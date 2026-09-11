//! The model pass.
//!
//! `App` hands this module the deterministic rules result, so `doc` is the
//! trusted baseline and it is already on the clipboard. The job here is to
//! improve on it without ever being allowed to make it worse: anything that
//! cannot be verified is rejected and the baseline stands.
//!
//! The model is given a checklist of the rule-pack hits that are actually
//! present in the text it is editing, and one focused repair attempt if the
//! strong ones survive. Concrete targets work where abstract instruction does
//! not: told to "remove AI cadence" the model swaps synonyms and keeps the
//! shape, but handed the exact words and phrases it removes them.

use std::fmt;

use crate::{
    config::{Config, Provider},
    doc::Doc,
    facts::{self, Protected},
    markdown, model, remote,
    rules::{Finding, Rules},
};

/// Why a rewrite was not published. The baseline stays in every case.
#[derive(Debug, Clone, PartialEq)]
pub enum Rejected {
    TooLong,
    Facts(facts::Violation),
    Structure(&'static str),
    /// The rewrite was well-formed and simply did not do the job, which reads
    /// differently in the status line from a rewrite that broke something.
    Style(&'static str),
    Model(String),
}

impl fmt::Display for Rejected {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Rejected::TooLong => write!(f, "too long to rewrite, rules only"),
            Rejected::Facts(violation) => write!(f, "rewrite not copied, {violation}"),
            Rejected::Structure(what) => write!(f, "rewrite not copied, {what}"),
            Rejected::Style(what) => write!(f, "rewrite not copied, {what}"),
            Rejected::Model(err) => write!(f, "rewrite unavailable, {err}"),
        }
    }
}

/// The prompt `text` will be edited under.
///
/// Public so the checklist can be inspected without a model on the other end.
/// It takes the text because the prompt depends on it: the pack's own hits in
/// this document become the edit targets.
pub fn system_prompt(rules: &Rules, text: &str, protected: usize) -> String {
    editing_prompt(&rules.detect(text), protected)
}

/// Instructions assembled from the same pack that drives the rules pass, so a
/// tell added there improves both passes at once.
///
/// The pack's own detector notes are deliberately left out. They are written
/// for a human, and measured over eighteen runs they made adherence markedly
/// worse: roughly four times as many tells survived with them than without.
/// More instruction is not more obedience in a small model, which is why the
/// standing rules below are short and the length goes on the checklist.
///
/// The register is named rather than described. Told to write "quickly and
/// plainly" a 4B writes for children ("making things people like"); told to
/// "preserve the professional register" and "make targeted edits" it freezes
/// and hands the text back with two words changed. Told whose voice to write
/// in, a knowledgeable professional writing for colleagues, it does neither.
/// Measured on a 118-word paragraph that is nothing but tells: the cautious
/// wording left 6 or 7 of 11 in place, this wording leaves 0 or 1.
///
/// No worked example, tempting as one is. Given a before/after pair the
/// same model lifts a sentence out of the "after" and puts it in the answer,
/// which is a fact it invented.
fn editing_prompt(findings: &[Finding], protected: usize) -> String {
    let placeholders = placeholder_rule(protected);
    let issues = issue_list(findings, 24);

    format!(
        "You are an experienced editor. Rewrite the text so it reads as if a \
         knowledgeable professional wrote it for colleagues: direct, concrete, in \
         ordinary adult vocabulary, with no hype and no rhetoric. Keep the meaning \
         and the professional register. Do not make it sound childish or casual.\n\n\
         Hard bans:\n\
         - Never use an em dash or an en dash. Use a comma, a full stop or a hyphen.\n\
         - Never open with a scene-setting clause such as \"In today's ...\". Start with the point.\n\
         - Never use the \"not just X, it's Y\" or \"not X, but Y\" contrast in any wording. State Y on its own.\n\
         - Never begin a sentence with \"In conclusion\", \"Ultimately\" or \"Overall\". Drop the opener and keep the claim.\n\n\
         {issues}\n\
         Rules:\n\
         - Keep every fact and claim, including qualifiers such as \"about\", \"roughly\" and \"likely\". Add nothing.\n\
         - Cut empty words rather than replacing them. A shorter sentence is fine.\n\
         - Sentences with none of the listed words may stay as they are.\n\
         - Keep lists, tables, links and headings as they are.\n\
         {placeholders}\
         - Return only the rewritten text, no preamble and no commentary."
    )
}

fn placeholder_rule(protected: usize) -> String {
    if protected == 0 {
        String::new()
    } else {
        "- Tokens such as [[F00]] are protected values. Copy every one through exactly once, unchanged. Never drop, repeat, alter or invent one.\n".to_owned()
    }
}

/// Turn rule-pack findings into the concrete checklist the model sees.
///
/// The pack's weight-1 rules are left out. They fire on ordinary writing, a
/// typographic quote or "red, green, and blue" among them, and the rules pass
/// deliberately leaves both alone: listing one asks the model for an edit we do
/// not want and spends a checklist line on it. So is anything the model could
/// only fix by reshaping structure, see `Finding::is_editable`.
///
/// Each line is the matched text and nothing else. The category names and
/// strong/soft labels were tried and are noise to a 4B: what it follows is
/// "rewrite every sentence that contains one".
fn issue_list(findings: &[Finding], limit: usize) -> String {
    let listed: Vec<&Finding> = findings
        .iter()
        .filter(|finding| finding.weight >= 2 && finding.is_editable())
        .take(limit)
        .collect();

    if listed.is_empty() {
        return "No specific rule-pack hit remains in this text. Return it as written, \
                changing a sentence only if it is clearly canned or inflated.\n"
            .to_owned();
    }

    let mut out = String::from(
        "These words and phrases are in the text and each must go. Rewrite every \
         sentence that contains one; do not swap the word for a synonym of the same kind:\n",
    );
    for finding in listed {
        out.push_str(&format!("- \"{}\"\n", finding.matched));
    }
    out
}

/// A deliberately tiny second-pass prompt, used only when strong hits survived
/// the first edit. It names exactly what was missed and nothing else.
fn repair_prompt(findings: &[Finding], protected: usize) -> String {
    let placeholders = placeholder_rule(protected);
    let targets: String = findings
        .iter()
        .take(16)
        .map(|finding| format!("- \"{}\"\n", finding.matched))
        .collect();

    format!(
        "The text still contains these words and phrases. Each must go. Rewrite every \
         sentence that contains one, and leave every other sentence exactly as it is:\n\
         {targets}\
         Do not swap a listed word for a synonym of the same kind. Say the plain thing, or cut it.\n\
         Keep the meaning, the professional register and the Markdown structure.\n\
         Never use an em dash or en dash.\n\
         {placeholders}\
         Return the complete corrected text only."
    )
}

/// Rewrite `doc`, or explain why the baseline should stand.
///
/// `doc` is the rules result, not the raw clipboard: `App::unslop` cleans it
/// before handing it over, so what arrives here is the trusted baseline and the
/// text the model is asked to edit.
///
/// Blocking: call from a worker thread.
pub fn run(rules: &Rules, doc: &Doc, config: &Config, port: Option<u16>) -> Result<Doc, Rejected> {
    let baseline_markdown = markdown_from_doc(doc)?;

    if baseline_markdown.chars().count() > config.rewrite.max_input_chars {
        return Err(Rejected::TooLong);
    }

    // Detected against the text the model will actually edit, which is what
    // makes the checklist worth more than a global word list: the pack's
    // fix-less phrases, its tier 2 and 3 vocabulary and all twenty of its
    // regex detectors become exact targets here, and none of them could be
    // stated usefully in advance.
    let baseline_findings = rules.detect(&baseline_markdown);
    let baseline_score = gate_score(&baseline_findings);

    let protected = Protected::new(&baseline_markdown);
    let answer = send(
        config,
        port,
        &editing_prompt(&baseline_findings, protected.count()),
        &protected.text,
    )?;

    let mut edited = protected.restore(answer.trim()).map_err(Rejected::Facts)?;
    validate_text_answer(&baseline_markdown, &edited)?;

    // One focused repair attempt, never a loop. A repair is kept only if it
    // removed something it was asked to remove, so a model that rewords at
    // random cannot spend the attempt making the text worse. Vocabulary counts
    // as progress here even though the gate below ignores it: dropping
    // "delve" is a win, it is only failing to drop it that is forgivable.
    let surviving = rules.detect(&edited);
    if let Some(repaired) = try_repair(config, port, &edited, &surviving)
        && repair_score(&rules.detect(&repaired)) < repair_score(&surviving)
    {
        edited = repaired;
    }

    let candidate = candidate_doc(doc, edited);

    // The deterministic pass runs again over the model's output. It reintroduces
    // the very tells the first pass removed, em dashes above all, and this is
    // free and the only way to be sure.
    let final_doc = rules.clean_doc(&candidate).doc;

    validate_structure(doc, &baseline_markdown, &final_doc)?;

    let final_markdown = markdown_from_doc(&final_doc)?;
    check_length(&baseline_markdown, &final_markdown)?;

    // A rewrite that left every structural tell standing did not do the job.
    // Vocabulary is not counted here, only shapes: see `gate_score`.
    if baseline_score > 0 && gate_score(&rules.detect(&final_markdown)) >= baseline_score {
        return Err(Rejected::Style(
            "the model left the structural tells in place",
        ));
    }

    Ok(final_doc)
}

/// Best-effort one-shot repair, or nothing when there is nothing to ask about.
///
/// A failure here does not throw away an otherwise valid first rewrite: the
/// normal validators still decide whether that rewrite may be returned.
fn try_repair(
    config: &Config,
    port: Option<u16>,
    current: &str,
    surviving: &[Finding],
) -> Option<String> {
    let findings = repair_targets(surviving);
    if findings.is_empty() {
        return None;
    }

    let protected = Protected::new(current);
    let answer = send(
        config,
        port,
        &repair_prompt(&findings, protected.count()),
        &protected.text,
    )
    .ok()?;

    let repaired = protected.restore(answer.trim()).ok()?;
    validate_text_answer(current, &repaired).ok()?;
    Some(repaired)
}

/// What the repair pass is worth asking about.
fn repair_targets(findings: &[Finding]) -> Vec<Finding> {
    findings
        .iter()
        .filter(|finding| finding.repair_required() && finding.is_editable())
        .cloned()
        .collect()
}

/// Whether a repair earned its extra round trip: every tell it was asked about
/// counts, vocabulary included, because removing one is real progress.
///
/// Public so the two policies can be told apart without a model on the other
/// end, as with `gate_score` and `is_commentary` below.
pub fn repair_score(findings: &[Finding]) -> u32 {
    weigh(findings, |finding| {
        finding.repair_required() && finding.is_editable()
    })
}

/// What the publish gate measures: structural tells only.
///
/// The gate may only count what the prompt actually asked for. It asks for
/// canned phrasing and formulaic shapes to go, so a phrase or regex hit that
/// survives is a refusal. It asks for precise vocabulary to be *kept*, and
/// "robust", "crucial" and "intricate" are ordinary technical English, so a
/// surviving word is often obedience. Counting those would reject a good
/// rewrite of a technical document for following its instructions.
pub fn gate_score(findings: &[Finding]) -> u32 {
    weigh(findings, |finding| {
        finding.is_structural() && finding.repair_required() && finding.is_editable()
    })
}

fn weigh(findings: &[Finding], counts: impl Fn(&Finding) -> bool) -> u32 {
    findings
        .iter()
        .filter(|finding| counts(finding))
        .map(|finding| u32::from(finding.weight.max(1)))
        .sum()
}

fn validate_text_answer(before: &str, after: &str) -> Result<(), Rejected> {
    check_length(before, after)?;
    if is_commentary(before, after) {
        return Err(Rejected::Structure(
            "the rewrite talked about the task instead of doing it",
        ));
    }
    Ok(())
}

/// Convert a document into the representation the model works with.
fn markdown_from_doc(doc: &Doc) -> Result<String, Rejected> {
    match doc {
        Doc::Plain(text) => Ok(text.clone()),
        Doc::Rich { html, .. } => {
            markdown::from_html(html).ok_or(Rejected::Structure("the markup could not be read"))
        }
    }
}

/// Turn a restored model answer back into the same family as the baseline.
fn candidate_doc(baseline: &Doc, restored: String) -> Doc {
    match baseline {
        Doc::Plain(_) => Doc::Plain(restored),
        Doc::Rich { .. } => Doc::Rich {
            html: markdown::to_html(&restored),
            // Markdown is the useful plain companion: a pipe table pasted into
            // a terminal remains intelligible as a table.
            text: restored,
        },
    }
}

/// Validate the final publishable document against the trusted baseline.
///
/// Rich text is compared as markup rather than as Markdown, or structure lost
/// in the conversion itself would go unnoticed. Plain text is compared through
/// Markdown because plain clipboard content is Markdown here too: a bullet list
/// copied out of a chat or a README arrives with no markup at all, and a model
/// that flattens it into prose has done exactly the damage the rich path
/// already refuses. Measured on Qwen3.5-4B, which flattens such a list in 14 of
/// 15 rewrites where the 8B never does, so the gap was invisible until the
/// model changed.
fn validate_structure(
    baseline: &Doc,
    baseline_markdown: &str,
    final_doc: &Doc,
) -> Result<(), Rejected> {
    let (before, after) = match (baseline, final_doc) {
        (
            Doc::Rich { html, .. },
            Doc::Rich {
                html: rewritten, ..
            },
        ) => (
            markdown::Shape::of_html(html),
            markdown::Shape::of_html(rewritten),
        ),
        _ => (
            markdown::Shape::of_html(&markdown::to_html(baseline_markdown)),
            markdown::Shape::of_html(&markdown::to_html(final_doc.text())),
        ),
    };

    match before.lost(after) {
        Some(lost) => Err(Rejected::Structure(lost)),
        None => Ok(()),
    }
}

/// Hand the text to whichever provider is configured.
fn send(config: &Config, port: Option<u16>, system: &str, text: &str) -> Result<String, Rejected> {
    let rewrite = &config.rewrite;
    let answer = match rewrite.provider {
        Provider::Off => Err("rewriting is switched off".to_owned()),
        Provider::Local => match port {
            Some(port) => model::rewrite(
                port,
                system,
                text,
                rewrite.temperature,
                rewrite.max_output_tokens,
            ),
            None => Err("no local model server".to_owned()),
        },
        Provider::Remote => remote::rewrite(
            &config.remote,
            system,
            text,
            rewrite.temperature,
            rewrite.max_output_tokens,
        ),
    };
    answer.map_err(Rejected::Model)
}

/// Ways a small model answers the instructions instead of following them.
const COMMENTARY: [&str; 18] = [
    "do not use",
    "don't use",
    "never use",
    "avoid using",
    "fancy word",
    "plain english",
    "keep it simple",
    "here is the rewritten",
    "here's the rewritten",
    "here is the edited",
    "here's the edited",
    "rewritten text",
    "rewritten version",
    "edited text",
    "edited version",
    "no commentary",
    "as an ai",
    "i cannot rewrite",
];

/// Whether the rewrite is talking about the task rather than doing it.
pub fn is_commentary(before: &str, after: &str) -> bool {
    let (before, after) = (before.to_lowercase(), after.to_lowercase());
    COMMENTARY
        .iter()
        .any(|marker| after.contains(marker) && !before.contains(marker))
}

/// Catch the ways a small model goes wrong that have nothing to do with
/// structure: refusing, or running away.
///
/// Public so the thresholds can be exercised without a model on the other end.
pub fn check_length(before: &str, after: &str) -> Result<(), Rejected> {
    if after.trim().is_empty() {
        return Err(Rejected::Structure("the rewrite came back empty"));
    }
    // Refusals and preambles are the common failure, and they are always much
    // shorter or much longer than the text they replaced.
    let (before, after) = (before.chars().count(), after.chars().count());
    if after * 3 < before {
        return Err(Rejected::Structure("the rewrite dropped most of the text"));
    }
    if after > before * 3 {
        return Err(Rejected::Structure("the rewrite ran away"));
    }
    Ok(())
}
