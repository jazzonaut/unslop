//! The model pass.
//!
//! The deterministic result is already on the clipboard by the time this runs,
//! so the job here is to improve on it without ever being allowed to make it
//! worse. Anything that cannot be verified is rejected and the baseline stands.

use std::fmt;

use crate::{
    config::{Config, Provider},
    doc::Doc,
    facts::{self, Protected},
    markdown, model, remote,
    rules::Rules,
};

/// Why a rewrite was not published. The baseline stays in every case.
#[derive(Debug, Clone, PartialEq)]
pub enum Rejected {
    TooLong,
    Facts(facts::Violation),
    Structure(&'static str),
    Model(String),
}

impl fmt::Display for Rejected {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Rejected::TooLong => write!(f, "too long to rewrite, rules only"),
            Rejected::Facts(violation) => write!(f, "rewrite not copied, {violation}"),
            Rejected::Structure(what) => write!(f, "rewrite not copied, {what}"),
            Rejected::Model(err) => write!(f, "rewrite unavailable, {err}"),
        }
    }
}

/// Instructions assembled from the same pack that drives the rules pass, so a
/// tell added there improves both passes at once.
///
/// Concrete prohibitions work and abstract descriptions do not: told to
/// "remove AI cadence" the model swaps synonyms and keeps the shape, but given
/// a list of banned words and punctuation it removes every one.
///
/// The pack's own detector notes are deliberately left out. They are written
/// for a human, and measured over eighteen runs they made adherence markedly
/// worse: roughly four times as many tells survived with them than without.
/// More instruction is not more obedience in an 8B.
pub fn system_prompt(rules: &Rules) -> String {
    let banned = rules.banned_words().join(", ");

    format!(
        "You rewrite text so it reads like a person wrote it quickly and plainly.\n\n\
         Hard bans, these are not negotiable:\n\
         - Never use an em dash or an en dash. Use a comma, a full stop, or a hyphen.\n\
         - Never open with \"In today's ...\", \"In the world of ...\", or any other \
           scene-setting clause. Start with the point.\n\
         - Never use the \"not just X, it's Y\" shape in any wording, including \
           \"doesn't just X, it Y\" and \"more than X, it Y\". State Y on its own.\n\
         - Never use any of these words: {banned}\n\n\
         Rules:\n\
         - Do not swap a banned word for a fancier synonym. Say the plain thing, or drop the claim.\n\
         - Vary sentence length. Some short, some longer.\n\
         - Text in double square brackets such as [[F00]] is a protected value. \
           Copy each one through exactly once, unchanged. Never drop, repeat or invent one.\n\
         - Do not add facts and do not remove facts.\n\
         - Leave wording that already sounds natural alone.\n\
         - Return only the rewritten text, with no preamble and no commentary."
    )
}

/// Rewrite `doc`, or explain why the baseline should stand.
///
/// Blocking: call from a worker thread.
pub fn run(rules: &Rules, doc: &Doc, config: &Config, port: Option<u16>) -> Result<Doc, Rejected> {
    // Rich text goes through Markdown, which the model handles well and raw
    // HTML it does not. A rich document that cannot be converted is left to
    // the baseline rather than silently flattened into plain text.
    let markdown = match doc {
        Doc::Plain(text) => text.clone(),
        Doc::Rich { html, .. } => {
            markdown::from_html(html).ok_or(Rejected::Structure("the markup could not be read"))?
        }
    };

    if markdown.chars().count() > config.rewrite.max_input_chars {
        return Err(Rejected::TooLong);
    }

    // The model never sees a price, a date or an address, so it cannot alter
    // one. This is prevention; the checks below are the second line.
    let protected = Protected::new(&markdown);
    let answer = send(config, port, &system_prompt(rules), &protected.text)?;
    let restored = protected.restore(answer.trim()).map_err(Rejected::Facts)?;

    check_length(&markdown, &restored)?;
    if is_commentary(&markdown, &restored) {
        return Err(Rejected::Structure(
            "the rewrite talked about the task instead of doing it",
        ));
    }

    // The model reintroduces the very tells the first pass removed, em dashes
    // above all, so the deterministic pass runs again over its output. It is
    // free and it is the only way to be sure.
    match doc {
        Doc::Plain(_) => {
            // Plain text is Markdown here too. A bullet list copied out of a
            // document, a chat or a README arrives with no markup at all, and
            // a model that flattens it into prose has done exactly the damage
            // the rich path already refuses. Measured on Qwen3.5-4B, which
            // flattens such a list in 14 of 15 rewrites where the 8B never
            // does, so the gap was invisible until the model changed.
            let before = markdown::Shape::of_html(&markdown::to_html(&markdown));
            if let Some(lost) = before.lost(markdown::Shape::of_html(&markdown::to_html(&restored)))
            {
                return Err(Rejected::Structure(lost));
            }
            Ok(Doc::Plain(rules.clean(&restored).text))
        }
        Doc::Rich { html, .. } => {
            let rewritten = markdown::to_html(&restored);
            // Compared against the original markup rather than the Markdown,
            // or structure lost in the conversion itself would go unnoticed.
            if let Some(lost) =
                markdown::Shape::of_html(html).lost(markdown::Shape::of_html(&rewritten))
            {
                return Err(Rejected::Structure(lost));
            }
            // The plain companion stays as Markdown: pasted into a terminal a
            // pipe table still reads as a table, where stripped text does not.
            Ok(rules
                .clean_doc(&Doc::Rich {
                    html: rewritten,
                    text: restored,
                })
                .doc)
        }
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

/// Ways an 8B answers the instructions instead of following them. Paraphrase
/// is the usual shape, so these are the words the paraphrase reaches for
/// rather than anything quoted from the prompt.
const COMMENTARY: [&str; 14] = [
    "do not use",
    "don't use",
    "never use",
    "avoid using",
    "fancy word",
    "plain english",
    "keep it simple",
    "here is the rewritten",
    "here's the rewritten",
    "rewritten text",
    "rewritten version",
    "no commentary",
    "as an ai",
    "i cannot rewrite",
];

/// Whether the rewrite is talking about the task rather than doing it.
///
/// Each marker has to be absent from the input as well: a style guide is
/// entitled to say "do not use", and only a phrase the model brought with it
/// is a leak. That qualifier is what makes a crude list safe to act on.
///
/// Public so the list can be exercised without a model on the other end.
pub fn is_commentary(before: &str, after: &str) -> bool {
    let (before, after) = (before.to_lowercase(), after.to_lowercase());
    COMMENTARY
        .iter()
        .any(|marker| after.contains(marker) && !before.contains(marker))
}

/// Catch the ways a small model goes wrong that have nothing to do with
/// structure: refusing, or running away.
fn check_length(before: &str, after: &str) -> Result<(), Rejected> {
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
