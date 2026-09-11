//! The deterministic pass and rule-pack detector.
//!
//! The vendored soundshuman pack does two different jobs:
//! - phrases with a `fix` are safe enough to rewrite mechanically;
//! - vocabulary, phrases without a fix, and regex rules are signals for the
//!   model/editor, not automatic substitutions.
//!
//! Keeping those jobs separate makes the rules-only result trustworthy while
//! still giving the model concrete, source-specific targets.

use std::collections::HashSet;

use regex::Regex;
use serde::Deserialize;

use crate::{doc::Doc, html_text};

/// A rule pack that failed to parse, so the caller can fall back rather than
/// take down a resident application over an editable file.
pub type LoadError = serde_json::Error;

#[derive(Deserialize)]
struct Pack {
    phrases: Vec<PackPhrase>,
    vocabulary: Vocabulary,
    #[serde(default)]
    regex: Vec<PackRegex>,
}

#[derive(Deserialize)]
struct Vocabulary {
    tier1: Tier,
    #[serde(default)]
    tier2: Tier,
    #[serde(default)]
    tier3: Tier,
}

#[derive(Deserialize, Default)]
struct Tier {
    #[serde(default)]
    weight: u8,
    #[serde(default)]
    words: Vec<String>,
    #[serde(rename = "minDistinct", default)]
    min_distinct: Option<usize>,
    #[serde(rename = "maxDensityPct", default)]
    max_density_pct: Option<f32>,
}

#[derive(Deserialize)]
struct PackPhrase {
    #[serde(rename = "match")]
    matches: String,
    fix: Option<String>,
    #[serde(default)]
    category: String,
    #[serde(default)]
    weight: u8,
}

#[derive(Deserialize)]
struct PackRegex {
    id: String,
    pattern: String,
    #[serde(default)]
    flags: Option<String>,
    #[serde(default)]
    category: String,
    #[serde(default)]
    weight: u8,
}

struct Phrase {
    matches: String,
    fix: Option<String>,
    category: String,
    weight: u8,
}

struct Detector {
    id: String,
    category: String,
    weight: u8,
    kind: DetectorKind,
}

enum DetectorKind {
    Regex(Regex),
    Emoji,
}

pub struct Rules {
    phrases: Vec<Phrase>,
    tier1: TierRules,
    tier2: TierRules,
    tier3: TierRules,
    detectors: Vec<Detector>,
}

struct TierRules {
    weight: u8,
    words: Vec<String>,
    min_distinct: Option<usize>,
    max_density_pct: Option<f32>,
}

impl From<Tier> for TierRules {
    fn from(value: Tier) -> Self {
        Self {
            weight: value.weight,
            words: value.words,
            min_distinct: value.min_distinct,
            max_density_pct: value.max_density_pct,
        }
    }
}

/// One concrete detector hit in a piece of text.
///
/// `matched` is deliberately suitable for putting straight into a model prompt:
/// the 4B model follows "rewrite this exact phrase" far more reliably than an
/// abstract explanation of the detector category.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub id: String,
    pub matched: String,
    pub category: String,
    pub weight: u8,
    pub tier: Option<u8>,
}

impl Finding {
    /// Whether a phrase or regex detector found this, rather than the
    /// vocabulary tiers. Only vocabulary carries a tier.
    ///
    /// The distinction is what separates a shape from a word. "In today's
    /// rapidly evolving" and "the stakes are significant" are habits of
    /// arrangement and always worth removing; "robust" and "crucial" are
    /// ordinary technical English that a rewrite is right to leave alone.
    pub fn is_structural(&self) -> bool {
        self.tier.is_none()
    }

    /// Strong enough to justify one targeted repair call if it survives the
    /// initial rewrite. Tier 2 already has its density gate applied before a
    /// finding exists, so it is meaningful when it appears here.
    pub fn repair_required(&self) -> bool {
        matches!(self.tier, Some(1) | Some(2)) || self.weight >= 3
    }

    /// Whether a rewrite can act on this without reshaping the document.
    ///
    /// The pack's inline-header-list detector fires on `- **Label**:` bullets,
    /// and the only way to satisfy it is to restructure a list the prompt has
    /// just been told to keep as it is. Listing it asks the model for two
    /// contradictory things, and counting it at the gate can reject a rewrite
    /// that fixed everything else because the bullets it was told to keep are
    /// still there. It stays a finding for scoring; it is not an edit target.
    pub fn is_editable(&self) -> bool {
        self.id != "inline-header-list"
    }
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

        let Vocabulary {
            tier1,
            tier2,
            tier3,
        } = pack.vocabulary;
        let mut phrases: Vec<Phrase> = pack
            .phrases
            .into_iter()
            .map(|phrase| Phrase {
                matches: phrase.matches,
                fix: phrase.fix,
                category: phrase.category,
                weight: phrase.weight,
            })
            .collect();

        // Longest first, so "in light of the fact that" wins over a shorter
        // phrase beginning at the same byte when applying mechanical fixes.
        phrases.sort_by_key(|phrase| std::cmp::Reverse(phrase.matches.len()));

        let detectors = pack
            .regex
            .into_iter()
            .filter_map(compile_detector)
            .collect();

        Ok(Self {
            phrases,
            tier1: tier1.into(),
            tier2: tier2.into(),
            tier3: tier3.into(),
            detectors,
        })
    }

    /// How many phrases this pass can apply on its own. Most of the pack is
    /// detection-only, so this is a small fraction of the phrases it holds.
    pub fn phrase_count(&self) -> usize {
        self.phrases
            .iter()
            .filter(|phrase| phrase.fix.is_some())
            .count()
    }

    /// Find concrete rule-pack violations in `text`.
    ///
    /// This mirrors the useful parts of soundshuman's `sloplint` semantics:
    /// tier 1 always flags; tier 2 flags only when enough distinct terms are
    /// present; tier 3 flags only above its density threshold; every phrase and
    /// compiled regex detector is considered. Results are deduplicated for use
    /// as a compact editing checklist rather than as a full scoring report.
    pub fn detect(&self, text: &str) -> Vec<Finding> {
        let mut findings = Vec::new();
        let mut seen = HashSet::new();
        let word_count = word_count(text).max(1);
        // Folded once rather than once per term. There are 131 vocabulary
        // words and 38 phrases, so folding per term is 169 copies of the whole
        // document for every call, and a rewrite makes several calls. Curly
        // apostrophes go too, so the pack's "in today's" phrases match
        // whichever apostrophe the source typed.
        let folded = fold(text);

        add_tier_hits(
            &mut findings,
            &mut seen,
            &folded,
            &self.tier1,
            1,
            TierGate::Always,
            word_count,
        );
        add_tier_hits(
            &mut findings,
            &mut seen,
            &folded,
            &self.tier2,
            2,
            TierGate::Distinct(self.tier2.min_distinct.unwrap_or(2)),
            word_count,
        );
        add_tier_hits(
            &mut findings,
            &mut seen,
            &folded,
            &self.tier3,
            3,
            TierGate::Density(self.tier3.max_density_pct.unwrap_or(1.5)),
            word_count,
        );

        for phrase in &self.phrases {
            if folded.contains(&fold(&phrase.matches)) {
                push_finding(
                    &mut findings,
                    &mut seen,
                    Finding {
                        id: format!("phrase:{}", phrase.matches),
                        matched: phrase.matches.clone(),
                        category: phrase.category.clone(),
                        weight: phrase.weight,
                        tier: None,
                    },
                );
            }
        }

        for detector in &self.detectors {
            match &detector.kind {
                DetectorKind::Regex(regex) => {
                    for found in regex.find_iter(text) {
                        push_finding(
                            &mut findings,
                            &mut seen,
                            Finding {
                                id: detector.id.clone(),
                                matched: found.as_str().trim().to_owned(),
                                category: detector.category.clone(),
                                weight: detector.weight,
                                tier: None,
                            },
                        );
                    }
                }
                DetectorKind::Emoji => {
                    if let Some(found) = text.chars().find(|c| is_emoji(*c)) {
                        push_finding(
                            &mut findings,
                            &mut seen,
                            Finding {
                                id: detector.id.clone(),
                                matched: found.to_string(),
                                category: detector.category.clone(),
                                weight: detector.weight,
                                tier: None,
                            },
                        );
                    }
                }
            }
        }

        // Put the strongest, most concrete instructions first in the prompt.
        findings.sort_by(|a, b| {
            b.repair_required()
                .cmp(&a.repair_required())
                .then_with(|| b.weight.cmp(&a.weight))
                .then_with(|| a.matched.cmp(&b.matched))
        });
        findings
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
                Some((fix, len)) => {
                    push_fix(&mut out, &rest[..len], fix);
                    fixes += 1;
                    rest = &rest[len..];
                    // A deleted phrase leaves the space that followed it, and
                    // the punctuation that set it off. Keeping that comma is
                    // worse than the phrase was: it strands against whatever
                    // came before, as ". ," or ", ,".
                    if fix.is_empty() {
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

    /// The replacement for the mechanically fixable phrase matching at the
    /// start of `rest`, and how much of `rest` it covers.
    fn match_at_start(&self, rest: &str) -> Option<(&str, usize)> {
        self.phrases.iter().find_map(|phrase| {
            let fix = phrase.fix.as_deref()?;
            let len = phrase.matches.len();
            let candidate = rest.as_bytes().get(..len)?;
            // The pack's phrases are ASCII, so a byte-wise comparison is both
            // correct and free of the index shifts that lowercasing can cause.
            (candidate.eq_ignore_ascii_case(phrase.matches.as_bytes())
                && !rest[len..].starts_with(is_word_char))
            .then_some((fix, len))
        })
    }
}

#[derive(Clone, Copy)]
enum TierGate {
    Always,
    Distinct(usize),
    Density(f32),
}

/// `folded` must already have come through `fold`, as the needles do here.
fn add_tier_hits(
    out: &mut Vec<Finding>,
    seen: &mut HashSet<String>,
    folded: &str,
    tier: &TierRules,
    number: u8,
    gate: TierGate,
    word_count: usize,
) {
    let present: Vec<(&String, usize)> = tier
        .words
        .iter()
        .filter_map(|word| {
            let count = word_occurrences(folded, &fold(word));
            (count > 0).then_some((word, count))
        })
        .collect();

    let enabled = match gate {
        TierGate::Always => true,
        TierGate::Distinct(minimum) => present.len() >= minimum,
        TierGate::Density(max_pct) => {
            let occurrences: usize = present.iter().map(|(_, count)| *count).sum();
            (occurrences as f32 / word_count as f32) * 100.0 > max_pct
        }
    };

    if !enabled {
        return;
    }

    for (word, _) in present {
        push_finding(
            out,
            seen,
            Finding {
                id: format!("vocab:{word}"),
                matched: word.clone(),
                category: "vocabulary".to_owned(),
                weight: tier.weight,
                tier: Some(number),
            },
        );
    }
}

fn push_finding(out: &mut Vec<Finding>, seen: &mut HashSet<String>, finding: Finding) {
    // Same detector/match appearing twice in a paragraph is useful for scoring,
    // but not useful twice in a 4B model's editing checklist.
    let key = format!("{}\0{}", finding.id, finding.matched.to_ascii_lowercase());
    if seen.insert(key) {
        out.push(finding);
    }
}

/// Occurrences of `needle` on whole-word boundaries. Both sides must already
/// have come through `fold`.
fn word_occurrences(haystack: &str, needle: &str) -> usize {
    if needle.is_empty() {
        return 0;
    }

    let bytes = haystack.as_bytes();
    let mut count = 0;
    let mut from = 0;

    while let Some(relative) = haystack[from..].find(needle) {
        let start = from + relative;
        let end = start + needle.len();
        let before_ok = start == 0 || !is_regex_word_byte(bytes[start - 1]);
        let after_ok = end == bytes.len() || !is_regex_word_byte(bytes[end]);
        if before_ok && after_ok {
            count += 1;
        }
        from = end.max(start + 1);
    }

    count
}

fn is_regex_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// Lowercase, with curly apostrophes straightened.
///
/// The upstream scanner treats the two apostrophes as equivalent, and matching
/// here reports whole terms rather than byte offsets, so folding both sides of
/// every comparison is safe.
fn fold(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '\u{2018}' | '\u{2019}' => '\'',
            c => c.to_ascii_lowercase(),
        })
        .collect()
}

fn word_count(text: &str) -> usize {
    let mut count = 0;
    let mut in_word = false;
    for c in text.chars() {
        let word = c.is_alphanumeric() || matches!(c, '\'' | '\u{2019}' | '-' | '\u{2013}');
        if word && !in_word {
            count += 1;
        }
        in_word = word;
    }
    count
}

fn compile_detector(rule: PackRegex) -> Option<Detector> {
    if rule.id == "emoji" {
        return Some(Detector {
            id: rule.id,
            category: rule.category,
            weight: rule.weight,
            kind: DetectorKind::Emoji,
        });
    }

    let Some(pattern) = js_regex_to_rust(&rule.pattern) else {
        eprintln!(
            "slop detector {:?} uses an escape with no Rust equivalent and was skipped",
            rule.id
        );
        return None;
    };
    let flags = rule.flags.as_deref().unwrap_or("gi");
    let mut prefix = String::new();
    if flags.contains('i') {
        prefix.push_str("(?i)");
    }
    if flags.contains('m') {
        prefix.push_str("(?m)");
    }

    match Regex::new(&format!("{prefix}{pattern}")) {
        Ok(regex) => Some(Detector {
            id: rule.id,
            category: rule.category,
            weight: rule.weight,
            kind: DetectorKind::Regex(regex),
        }),
        Err(err) => {
            eprintln!(
                "slop detector {:?} did not compile and was skipped: {err}",
                rule.id
            );
            None
        }
    }
}

/// Convert JavaScript-style `\\u2014` escapes from the vendored pack to the
/// `regex` crate's `\\u{2014}` form.
///
/// UTF-16 surrogate escapes are not scalar values and have no equivalent, so a
/// pattern using one cannot be translated at all. The only upstream detector
/// that does is `emoji`, which is matched by codepoint range instead.
fn js_regex_to_rust(pattern: &str) -> Option<String> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut out = String::with_capacity(pattern.len());
    let mut i = 0;

    while i < chars.len() {
        if chars[i] == '\\' && i + 5 < chars.len() && chars[i + 1] == 'u' {
            let hex: String = chars[i + 2..i + 6].iter().collect();
            if hex.chars().all(|c| c.is_ascii_hexdigit()) {
                let value = u32::from_str_radix(&hex, 16).ok()?;
                if (0xD800..=0xDFFF).contains(&value) {
                    return None;
                }
                out.push_str("\\u{");
                out.push_str(&hex);
                out.push('}');
                i += 6;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }

    Some(out)
}

fn is_emoji(c: char) -> bool {
    let n = c as u32;
    (0x2600..=0x27BF).contains(&n)
        || (0x2B00..=0x2BFF).contains(&n)
        || (0x1F300..=0x1FAFF).contains(&n)
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
    let word_end = rest.find(|c| !is_word_char(c)).unwrap_or(rest.len());
    rest[word_end..]
        .find(is_word_char)
        .map_or(rest.len(), |gap| word_end + gap)
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '\''
}

/// Em dashes are the highest-signal tell there is, so they always go.
/// A hyphen reads as human even where it is not strictly correct, which is the
/// trade the user asked for. En dashes between digits stay a range.
fn normalise_dashes(text: &str) -> (String, usize) {
    let mut out = String::with_capacity(text.len());
    let mut fixes = 0;
    let mut chars = text.chars().peekable();
    let mut pending_capital = false;

    while let Some(c) = chars.next() {
        match c {
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
