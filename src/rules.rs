//! The deterministic pass: the safe half of unslopping.
//!
//! The vendored rule pack is a linter, not a rewriter. Only its `phrases`
//! carry a replacement; the regex detectors and vocabulary tiers describe what
//! is wrong without saying what to put instead, and ordinary words like
//! "leverage" must not be swapped blindly.
//! So this pass fixes what can be fixed safely and leaves the rest to the
//! model, which keeps the rules result trustworthy enough to be the baseline.

use serde::Deserialize;

use crate::{doc::Doc, html_text};

/// A rule pack that failed to parse, so the caller can fall back rather than
/// take down a resident application over an editable file.
pub type LoadError = serde_json::Error;

#[derive(Deserialize)]
struct Pack {
    phrases: Vec<PackPhrase>,
    vocabulary: Vocabulary,
}

#[derive(Deserialize)]
struct Vocabulary {
    /// The dead giveaways. The lower tiers are context-dependent and would
    /// only dilute a prompt.
    tier1: Tier,
}

#[derive(Deserialize)]
struct Tier {
    words: Vec<String>,
}

#[derive(Deserialize)]
struct PackPhrase {
    #[serde(rename = "match")]
    matches: String,
    /// Absent where the pack knows a phrase is slop but has no safe substitute.
    fix: Option<String>,
}

struct Phrase {
    matches: String,
    fix: String,
}

pub struct Rules {
    phrases: Vec<Phrase>,
    /// Carried through to the prompt: these are the tells the deterministic
    /// pass cannot fix, because the pack offers no safe substitute.
    banned_words: Vec<String>,
}

/// The result of a rules pass, with a count for the status line.
#[derive(Debug, Clone, PartialEq)]
pub struct Cleaned {
    pub text: String,
    pub fixes: usize,
}

/// The result of cleaning a whole clipboard document.
#[derive(Debug, Clone, PartialEq)]
pub struct CleanedDoc {
    pub doc: Doc,
    pub fixes: usize,
}

impl Rules {
    pub fn load(json: &str) -> Result<Self, LoadError> {
        let pack: Pack = serde_json::from_str(json)?;
        let mut phrases: Vec<Phrase> = pack
            .phrases
            .into_iter()
            .filter_map(|phrase| {
                Some(Phrase {
                    matches: phrase.matches,
                    fix: phrase.fix?,
                })
            })
            .collect();
        // Longest first, so "in light of the fact that" wins over any shorter
        // phrase that starts at the same place.
        phrases.sort_by_key(|phrase| std::cmp::Reverse(phrase.matches.len()));
        Ok(Self {
            phrases,
            banned_words: pack.vocabulary.tier1.words,
        })
    }

    pub fn phrase_count(&self) -> usize {
        self.phrases.len()
    }

    /// Words the model must not reach for. These are the tells the
    /// deterministic pass cannot fix, because there is no safe substitute.
    pub fn banned_words(&self) -> &[String] {
        &self.banned_words
    }

    /// Clean a clipboard document, keeping rich documents rich.
    ///
    /// Both halves of a rich document are cleaned, since the plain companion
    /// is what terminals and Notepad receive. The reported count is the one
    /// from the markup, so the two passes are not counted twice.
    pub fn clean_doc(&self, doc: &Doc) -> CleanedDoc {
        match doc {
            Doc::Plain(text) => {
                let cleaned = self.clean(text);
                CleanedDoc {
                    doc: Doc::Plain(cleaned.text),
                    fixes: cleaned.fixes,
                }
            }
            Doc::Rich { html, text } => {
                let mut fixes = 0;
                let html = html_text::map_text(html, |run| {
                    let cleaned = self.clean(run);
                    fixes += cleaned.fixes;
                    cleaned.text
                });
                let text = self.clean(text).text;
                CleanedDoc {
                    doc: Doc::Rich { html, text },
                    fixes,
                }
            }
        }
    }

    /// Apply every safe fix to a run of plain text.
    pub fn clean(&self, text: &str) -> Cleaned {
        let mut out = String::with_capacity(text.len());
        let mut fixes = 0;
        let mut rest = text;

        while !rest.is_empty() {
            match self.match_at_start(rest) {
                Some((phrase, len)) => {
                    push_fix(&mut out, &rest[..len], &phrase.fix);
                    fixes += 1;
                    rest = &rest[len..];
                    // A deleted phrase leaves the space that followed it, and
                    // the punctuation that set it off. Keeping that comma is
                    // worse than the phrase was: it strands against whatever
                    // came before, as ". ," or ", ,".
                    if phrase.fix.is_empty() {
                        rest = rest.trim_start_matches(' ');
                        if let Some(after) = rest.strip_prefix([',', ';', ':']) {
                            rest = after.trim_start_matches(' ');
                            unpair_comma(&mut out);
                        }
                    }
                }
                None => {
                    let advance = next_boundary(rest);
                    out.push_str(&rest[..advance]);
                    rest = &rest[advance..];
                }
            }
        }

        let (text, punctuation_fixes) = normalise_dashes(&out);
        Cleaned {
            text,
            fixes: fixes + punctuation_fixes,
        }
    }

    /// The phrase matching at the start of `rest`, if any, and its length.
    fn match_at_start(&self, rest: &str) -> Option<(&Phrase, usize)> {
        self.phrases.iter().find_map(|phrase| {
            let len = phrase.matches.len();
            let candidate = rest.as_bytes().get(..len)?;
            // The pack's phrases are ASCII, so a byte-wise comparison is both
            // correct and free of the index shifts that lowercasing can cause.
            (candidate.eq_ignore_ascii_case(phrase.matches.as_bytes())
                && !rest[len..].starts_with(is_word_char))
            .then_some((phrase, len))
        })
    }
}

/// Append `fix`, carrying over the capitalisation of the text it replaces.
fn push_fix(out: &mut String, matched: &str, fix: &str) {
    let capitalised = matched.starts_with(|c: char| c.is_uppercase());
    // A phrase deleted from the start of a sentence leaves the next word to
    // carry the capital, which is handled when that word is appended.
    if fix.is_empty() {
        if capitalised {
            out.push('\u{0}'); // marker: the next word needs a capital
        }
        return;
    }
    match (capitalised, fix.chars().next()) {
        (true, Some(first)) => {
            out.extend(first.to_uppercase());
            out.push_str(&fix[first.len_utf8()..]);
        }
        _ => out.push_str(fix),
    }
}

/// Drop the comma on the near side of a phrase that had one on both.
///
/// An aside set off by a pair of commas takes the pair with it; one left
/// behind reads as a typo. A phrase that opened a sentence has no comma before
/// it and nothing is removed.
fn unpair_comma(out: &mut String) {
    let kept = out.trim_end_matches(' ');
    if let Some(kept) = kept.strip_suffix([',', ';', ':']) {
        let kept = format!("{kept} ");
        out.clear();
        out.push_str(&kept);
    }
}

/// How far to copy before testing for a phrase again: to the next word start,
/// since every phrase in the pack begins on a word boundary.
fn next_boundary(rest: &str) -> usize {
    // Step over the current word, then the gap after it.
    let word_end = rest.find(|c| !is_word_char(c)).unwrap_or(rest.len());
    rest[word_end..]
        .find(is_word_char)
        .map_or(rest.len(), |gap| word_end + gap)
}

/// Whether a character is part of a word.
///
/// Deliberately character-wise rather than byte-wise. Treating every non-ASCII
/// byte as part of a word glues neighbours together across dashes and curly
/// quotes, so a phrase sitting immediately after an em dash never begins at a
/// word boundary and is never matched.
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '\''
}

/// Em dashes are the highest-signal tell there is, so they always go.
///
/// A hyphen reads as human even where it is not strictly correct, which is the
/// trade the user asked for. En dashes between digits stay a range.
fn normalise_dashes(text: &str) -> (String, usize) {
    let mut out = String::with_capacity(text.len());
    let mut fixes = 0;
    let mut chars = text.chars().peekable();
    let mut pending_capital = false;

    while let Some(c) = chars.next() {
        match c {
            // The marker left by a deleted phrase.
            '\u{0}' => {
                pending_capital = true;
                continue;
            }
            '\u{2014}' | '\u{2013}' => {
                let numeric_range = c == '\u{2013}'
                    && out.ends_with(|p: char| p.is_ascii_digit())
                    && chars.peek().is_some_and(char::is_ascii_digit);
                if numeric_range {
                    out.push('-');
                } else {
                    while out.ends_with(' ') {
                        out.pop();
                    }
                    // No leading space when the dash opened the text.
                    if !out.is_empty() {
                        out.push(' ');
                    }
                    out.push_str("- ");
                    while chars.peek() == Some(&' ') {
                        chars.next();
                    }
                }
                fixes += 1;
            }
            _ => {
                if pending_capital && c.is_alphabetic() {
                    out.extend(c.to_uppercase());
                    pending_capital = false;
                } else {
                    out.push(c);
                }
            }
        }
    }

    (out, fixes)
}
