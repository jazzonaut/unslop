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
    config::{Config, Mode, Provider},
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

impl Rejected {
    /// Whether asking the model again could plausibly do better.
    ///
    /// Sampling is not deterministic, so most refusals are a bad draw rather
    /// than a text the model cannot handle.
    pub fn worth_another_draw(&self) -> bool {
        match self {
            // Nothing a second attempt can do about a text that will not fit
            // or a server that is not answering.
            Rejected::TooLong | Rejected::Model(_) => false,
            // The tells have their own repair pass, which has already run and
            // already declined to improve on this.
            Rejected::Style(_) => false,
            Rejected::Structure(_) | Rejected::Facts(_) => true,
        }
    }
}

impl fmt::Display for Rejected {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Rejected::TooLong => write!(f, "a paragraph too long to rewrite, rules only"),
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
    editing_prompt(&rules.detect(text), text, protected)
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
fn editing_prompt(findings: &[Finding], text: &str, protected: usize) -> String {
    let placeholders = placeholder_rule(protected, false);
    let shape = shape_rule(text);
    let issues = issue_list(findings, 24);

    format!(
        "You are an experienced editor. Rewrite the text so it reads as if a \
         knowledgeable professional wrote it for colleagues: direct, concrete, in \
         ordinary adult vocabulary, with no hype and no rhetoric. Keep the meaning \
         and the professional register. Do not make it sound childish or casual.\n\n\
         {HARD_BANS}\n\
         {issues}\n\
         Rules:\n\
         - Keep every fact and claim, including qualifiers such as \"about\", \"roughly\" and \"likely\". Add nothing.\n\
         - Cut empty words rather than replacing them. A shorter sentence is fine.\n\
         - Sentences with none of the listed words may stay as they are.\n\
         - Keep lists, tables, links and headings as they are.\n\
         {shape}\
         {placeholders}\
         - Return only the rewritten text, no preamble and no commentary."
    )
}

/// The tells a model reintroduces whatever it was asked to do, so every
/// prompt carries them.
const HARD_BANS: &str = "Hard bans:\n\
- Never use an em dash or an en dash. Use a comma, a full stop or a hyphen.\n\
- Never open with a scene-setting clause such as \"In today's ...\". Start with the point.\n\
- Never use the \"not just X, it's Y\" or \"not X, but Y\" contrast in any wording. State Y on its own.\n\
- Never begin a sentence with \"In conclusion\", \"Ultimately\" or \"Overall\". Drop the opener and keep the claim.\n";

/// The prompt for the two condensing modes. Public for the same reason as
/// `system_prompt`. `text` sets the length target: a 4B told "about a quarter"
/// guesses, told "about 60 words" it lands close.
pub fn condense_prompt(mode: Mode, text: &str, protected: usize) -> String {
    match mode {
        Mode::Unslop => unreachable!("unslop builds its prompt from the rule pack"),
        Mode::Simplify => simplify_prompt(text, protected),
        Mode::Tldr => summary_prompt(text.split_whitespace().count(), protected),
    }
}

fn simplify_prompt(text: &str, protected: usize) -> String {
    let placeholders = placeholder_rule(protected, false);
    let shape = shape_rule(text);
    format!(
        "You are an experienced editor. Rewrite the text in plain language for a busy, \
         intelligent reader who is not a specialist: short sentences, common words, one \
         idea per sentence. Replace or briefly explain jargon where the meaning allows, \
         and keep a technical term where there is no plain equivalent. Cut repetition, \
         asides and filler so the result is shorter, but keep every distinct point. \
         Keep the professional register: plain is not childish.\n\n\
         {HARD_BANS}\n\
         Rules:\n\
         - Keep every fact and claim, including qualifiers such as \"about\", \"roughly\" and \"likely\". Add nothing.\n\
         - Keep lists, tables, links and headings; simplify the words inside them.\n\
         {shape}\
         - Keep the paragraph breaks of the text. Do not put each sentence on its own line.\n\
         {placeholders}\
         - Return only the rewritten text, no preamble and no commentary."
    )
}

/// Deliberately the shortest prompt in the file. Measured on a status email
/// whose one sentence said three vendors were done and two were due by a date:
/// the editor-style prompt above, with its rules block, moved the date onto the
/// finished vendors in 12 of 12 runs, whatever rule was added about keeping
/// dates with their claims. This wording got it right in all but one of about
/// fifteen runs, so the failure is rare rather than gone: a 4B summariser can
/// still misattach a date, which is one reason the result is previewed before
/// it is copied. The register and hard-ban rules are not missed here: the rules
/// pass runs over the output.
///
/// The "two or three things" line is what makes it a summary rather than a
/// trim. Without it a fact-dense email came back at 88, 33 and 58 words from
/// a 91-word original, keeping every sentence that held a placeholder; with it,
/// 25 to 33 words, each run leading with the thing the reader has to do.
fn summary_prompt(words: usize, protected: usize) -> String {
    let placeholders = placeholder_rule(protected, true);
    let target = (words / 4).max(25).min(words);
    format!(
        "Summarise the text in about {target} words of plain prose for a colleague who has \
         not read it. Keep only what the text actually says; when a detail does not fit, \
         leave it out rather than reword it. Keep the two or three things the reader must \
         know or do and drop everything else. Write sentences, not headings or bullet \
         points, and keep the technical terms the text uses. Never use an em dash or an \
         en dash.\n\
         {placeholders}\
         Return only the summary."
    )
}

/// A 4B asked for short sentences likes to put each on its own line, in two
/// runs out of three on an ordinary email. When the text had no single line
/// breaks to begin with, none belong in the result: they are rejoined into the
/// paragraphs the model was told to keep. Lines that look like list items,
/// headings or table rows are left alone in case the model built one anyway.
pub fn keep_paragraphs(baseline: &str, edited: &str) -> String {
    let single_breaks = |text: &str| {
        text.split("\n\n")
            .any(|paragraph| paragraph.trim().contains('\n'))
    };
    if single_breaks(&baseline.replace("\r\n", "\n")) {
        return edited.to_owned();
    }
    let is_block = |line: &str| {
        let line = line.trim_start();
        line.starts_with(['-', '*', '+', '#', '|', '>'])
            || line
                .split_once(". ")
                .is_some_and(|(number, _)| number.chars().all(|c| c.is_ascii_digit()))
    };
    edited
        .split("\n\n")
        .map(|paragraph| {
            let lines: Vec<&str> = paragraph.lines().map(str::trim).collect();
            if lines.iter().any(|line| is_block(line)) {
                paragraph.to_owned()
            } else {
                lines
                    .into_iter()
                    .filter(|line| !line.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ")
            }
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// What must survive, stated from the same `Shape` the guard measures.
///
/// "Keep lists, tables and links" is the kind of abstract instruction this
/// model reads straight past: measured on two bulleted texts it flattened the
/// list into prose in 10 of 10 runs, and on a page with one link in it it
/// dropped the link in 7 of 8. Naming the count keeps them.
///
/// Built by destructuring `Shape` rather than written out a dimension at a
/// time, so a field the guard counts cannot go unmentioned here: that gap is
/// exactly what refused every rewrite of a page with a link in it while the
/// prompt said only "keep links". Adding a field to `Shape` stops compiling
/// until this says what to do with it.
fn shape_rule(text: &str) -> String {
    let html = markdown::to_html(text);
    let markdown::Shape {
        tables,
        links,
        list_items,
    } = markdown::Shape::of_html(&html);

    let mut rules = String::new();
    if list_items > 0 {
        // The guard counts items and does not care how they are marked, but
        // the model does exactly what this line says: told "- " it turned a
        // numbered list into dashes in 6 runs of 6, in both modes. The bullet
        // wording is the measured one and stays; a numbered list gets the
        // marker it already has.
        let marker = if html.contains("<ol") {
            "numbered or marked exactly as it is now"
        } else {
            "starting with \"- \""
        };
        rules += &format!(
            "- The text contains {list_items} bullet points. Return the same \
             {list_items} bullet points, each on its own line {marker}. \
             Shorten the words inside a bullet point, but never turn one into a \
             sentence of a paragraph, and never turn a heading, a lead-in line or a \
             paragraph into a bullet point.\n"
        );
    }
    if links > 0 {
        rules += &format!(
            "- The text contains {links} link{plural} written as [text](URL). Return \
             the same {links}, each still written that way. Shorten the words inside \
             the square brackets if they need it, and never turn a link into plain \
             text.\n",
            plural = if links == 1 { "" } else { "s" },
        );
    }
    if tables > 0 {
        rules += &format!(
            "- The text contains {tables} table{plural}. Return the same {tables}, \
             still as a markdown table with the same rows and columns. Shorten the \
             words inside a cell, and never flatten a table into prose.\n",
            plural = if tables == 1 { "" } else { "s" },
        );
    }
    rules
}

fn placeholder_rule(protected: usize, may_drop: bool) -> String {
    match (protected, may_drop) {
        (0, _) => String::new(),
        (_, false) => "- Tokens such as [[F00]] are protected values. Copy every one through exactly once, unchanged. Never drop, repeat, alter or invent one.\n".to_owned(),
        (_, true) => "- Tokens such as [[F00]] are protected values. Copy any you keep through exactly once, unchanged, and leave out the rest. Never alter, repeat or invent one.\n".to_owned(),
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
fn repair_prompt(findings: &[Finding], text: &str, protected: usize) -> String {
    let placeholders = placeholder_rule(protected, false);
    let shape = shape_rule(text);
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
         {shape}\
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
/// A text longer than `chunk_chars` is edited a paragraph group at a time, see
/// `chunks`. Every check the model pass makes is local to the text it was
/// given, so each group is validated on its own; only the structure check and
/// the rules re-clean need the whole document, and those run once over the
/// join. A group the model spoils keeps its own baseline rather than costing
/// the rest of the rewrite: at roughly 85% per call, all-or-nothing over five
/// groups would refuse more than half of long texts.
///
/// Blocking: call from a worker thread.
pub fn run(
    rules: &Rules,
    doc: &Doc,
    config: &Config,
    port: Option<u16>,
    mode: Mode,
) -> Result<Doc, Rejected> {
    let baseline_markdown = markdown_from_doc(doc)?;
    let (target, ceiling) = (config.rewrite.chunk_chars, config.rewrite.max_input_chars);

    if baseline_markdown.chars().count() <= target {
        return rewrite_one(rules, doc, &baseline_markdown, config, port, mode);
    }

    let (mut parts, mut passed, mut refused) = (Vec::new(), 0, None);
    for chunk in chunks(&baseline_markdown, target, ceiling)? {
        match rewrite_one(
            rules,
            &Doc::Plain(chunk.clone()),
            &chunk,
            config,
            port,
            mode,
        ) {
            Ok(edited) => {
                passed += 1;
                parts.push(edited.text().to_owned());
            }
            // A server that is not answering will not answer for the next group.
            Err(err @ Rejected::Model(_)) => return Err(err),
            Err(err) => {
                refused.get_or_insert(err);
                parts.push(chunk);
            }
        }
    }
    if let (0, Some(refused)) = (passed, refused) {
        return Err(refused);
    }

    let joined = candidate_doc(doc, parts.join("\n\n"));
    if mode == Mode::Tldr {
        // Summaries of the pieces are a digest, not a summary. Summarise that,
        // and chunk again should even the digest not fit.
        return run(rules, &joined, config, port, mode);
    }
    let final_doc = rules.clean_doc(&joined).doc;
    validate_structure(doc, &baseline_markdown, &final_doc)?;
    Ok(final_doc)
}

/// One model pass over a text that fits the context window.
fn rewrite_one(
    rules: &Rules,
    doc: &Doc,
    baseline_markdown: &str,
    config: &Config,
    port: Option<u16>,
    mode: Mode,
) -> Result<Doc, Rejected> {
    match mode {
        Mode::Unslop => unslop(rules, doc, baseline_markdown, config, port),
        Mode::Simplify | Mode::Tldr => condense(rules, doc, baseline_markdown, config, port, mode),
    }
}

/// Paragraph groups of about `target` characters, cut only at blank lines so
/// a list or a table is never split in half. A paragraph longer than the
/// target goes in a group of its own, up to the `ceiling` the context window
/// allows; beyond that it has nowhere to go and the text is refused as it
/// always was.
///
/// Public so the packing can be tested without a model on the other end.
pub fn chunks(text: &str, target: usize, ceiling: usize) -> Result<Vec<String>, Rejected> {
    let text = text.replace("\r\n", "\n");
    let mut out: Vec<(String, usize)> = Vec::new();
    for paragraph in text.split("\n\n").filter(|p| !p.trim().is_empty()) {
        let len = paragraph.chars().count();
        if len > ceiling {
            return Err(Rejected::TooLong);
        }
        match out.last_mut() {
            Some((chunk, used)) if *used + 2 + len <= target => {
                chunk.push_str("\n\n");
                chunk.push_str(paragraph);
                *used += 2 + len;
            }
            _ => out.push((paragraph.to_owned(), len)),
        }
    }
    Ok(out.into_iter().map(|(chunk, _)| chunk).collect())
}

/// The checklist-driven edit described at the top of the file.
fn unslop(
    rules: &Rules,
    doc: &Doc,
    baseline_markdown: &str,
    config: &Config,
    port: Option<u16>,
) -> Result<Doc, Rejected> {
    // Detected against the text the model will actually edit, which is what
    // makes the checklist worth more than a global word list: the pack's
    // fix-less phrases, its tier 2 and 3 vocabulary and all twenty of its
    // regex detectors become exact targets here, and none of them could be
    // stated usefully in advance.
    let baseline_findings = rules.detect(baseline_markdown);
    let baseline_score = gate_score(&baseline_findings);

    let protected = Protected::new(baseline_markdown);
    let prompt = editing_prompt(&baseline_findings, baseline_markdown, protected.count());

    with_second_draw(config, port, &prompt, &protected.text, |answer| {
        let mut edited = protected.restore(answer).map_err(Rejected::Facts)?;
        validate_text_answer(baseline_markdown, &edited)?;

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

        validate_structure(doc, baseline_markdown, &final_doc)?;

        let final_markdown = markdown_from_doc(&final_doc)?;
        check_length(baseline_markdown, &final_markdown)?;

        // A rewrite that left every structural tell standing did not do the job.
        // Vocabulary is not counted here, only shapes: see `gate_score`.
        if baseline_score > 0 && gate_score(&rules.detect(&final_markdown)) >= baseline_score {
            return Err(Rejected::Style(
                "the model left the structural tells in place",
            ));
        }

        Ok(final_doc)
    })
}

/// Simplify or summarise, with the checks that still make sense for a text
/// that is meant to come back shorter.
///
/// No checklist and no repair pass: the tells are not the job here, and the
/// rules pass over the output catches the ones the model brings along. A
/// summary is also excused the structure check, since dropping a table is what
/// it was asked to do. Simplify keeps that one: every point stays.
///
/// Neither is required to carry every protected value through. The prompts
/// still ask for all of them, and what matters is that a value which does
/// survive is the original bytes: a figure quietly rounded is the damage this
/// module exists to prevent, and a figure left out is visible in the preview.
/// Rejecting a whole good rewrite over one dropped value only taught the user
/// to press Retry until the model humoured us.
fn condense(
    rules: &Rules,
    doc: &Doc,
    baseline_markdown: &str,
    config: &Config,
    port: Option<u16>,
    mode: Mode,
) -> Result<Doc, Rejected> {
    let protected = Protected::new(baseline_markdown);
    let prompt = condense_prompt(mode, baseline_markdown, protected.count());

    with_second_draw(config, port, &prompt, &protected.text, |answer| {
        let edited = protected.restore_subset(answer).map_err(Rejected::Facts)?;
        let edited = keep_paragraphs(baseline_markdown, &edited);

        if is_commentary(baseline_markdown, &edited) {
            return Err(Rejected::Structure(
                "the rewrite talked about the task instead of doing it",
            ));
        }
        match mode {
            Mode::Tldr => check_shorter(baseline_markdown, &edited)?,
            _ => check_length(baseline_markdown, &edited)?,
        }

        let final_doc = rules.clean_doc(&candidate_doc(doc, edited)).doc;
        if mode == Mode::Simplify {
            validate_structure(doc, baseline_markdown, &final_doc)?;
        }
        Ok(final_doc)
    })
}

/// Ask once, and once more if the validators refused the answer. Never a loop.
///
/// Every guard here is a tripwire on the whole result: one dropped link or one
/// over-cut paragraph and an otherwise good rewrite is thrown away. That is
/// what made each new document shape find a fresh hole, answered one prompt
/// rule at a time.
///
/// What it is not is a repair. Handing the model its own rejected answer, the
/// original text and the guard's complaint rescued 0 of 4 refusals on the page
/// this was built for, where a plain second draw of the same prompt passes
/// about 7 times in 10: the model that has just dropped a link does not put it
/// back when asked, it writes the paragraph again without it. So the rejection
/// buys a second sample rather than a conversation.
///
/// The answer is validated by the same closure, so a second draw that fixes
/// the link but mangles a figure is discarded like any other bad answer, and
/// the first rejection is what the user is told about.
///
/// Measured on that page, fourteen simplify runs: 7 in 10 passed without this,
/// 12 in 14 with it. The cost falls on the refusals alone, and in unslop mode
/// the closure carries its own repair attempt, so a refused rewrite there can
/// reach four calls and took 57 seconds at its worst against 13 typical. That
/// is affordable here and nowhere else: the rules result is on screen from the
/// first moment, so the wait costs the user nothing they were using.
fn with_second_draw<F>(
    config: &Config,
    port: Option<u16>,
    prompt: &str,
    body: &str,
    finish: F,
) -> Result<Doc, Rejected>
where
    F: Fn(&str) -> Result<Doc, Rejected>,
{
    let answer = send(config, port, prompt, body)?;
    let refused = match finish(answer.trim()) {
        Ok(doc) => return Ok(doc),
        Err(refused) => refused,
    };
    if !refused.worth_another_draw() {
        return Err(refused);
    }
    match send(config, port, prompt, body) {
        Ok(second) => finish(second.trim()).map_err(|_| refused),
        Err(_) => Err(refused),
    }
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
        &repair_prompt(&findings, current, protected.count()),
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
const COMMENTARY: [&str; 24] = [
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
    "here is a summary",
    "here's a summary",
    "here is the summary",
    "here's the summary",
    "simplified text",
    "simplified version",
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

/// The one length rule a summary has to meet: it exists, and it is shorter.
///
/// No lower bound. A long, padded text can honestly come down to a sentence,
/// and a summary that is too short is visible on screen in a way a mangled
/// fact is not.
pub fn check_shorter(before: &str, after: &str) -> Result<(), Rejected> {
    if after.trim().is_empty() {
        return Err(Rejected::Structure("the summary came back empty"));
    }
    if after.chars().count() >= before.chars().count() {
        return Err(Rejected::Structure(
            "the summary is no shorter than the text",
        ));
    }
    Ok(())
}
