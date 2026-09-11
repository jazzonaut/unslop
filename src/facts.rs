//! Protecting facts from the model.
//!
//! Detecting a mangled figure after the fact is not enough when the text is a
//! client email. Anything the model has no business rewording is swapped for a
//! placeholder first, so the model never sees it and cannot alter it, and the
//! original bytes are put back afterwards.
//!
//! If a placeholder does not survive intact, the rewrite is rejected rather
//! than patched up: a model that dropped one has almost certainly rearranged
//! the sentence around it too.

use std::{collections::HashMap, fmt, sync::LazyLock};

use regex::Regex;

/// Boring on purpose. Unusual bracket characters tokenise awkwardly and invite
/// a small model to "tidy" them.
const PLACEHOLDER: &str = "[[F";

/// Patterns are tried in order and may not overlap, so the most specific come
/// first: a URL contains things that look like numbers and dates.
static PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r#"https?://[^\s<>()\[\]"']+"#,
        r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}",
        // Currency, either side, with optional thousands separators.
        r"[\p{Sc}]\s?\d[\d,. ]*\d|\b\d[\d,.]*\s?(?:GBP|USD|EUR|p|pence)\b",
        r"\b\d{4}-\d{2}-\d{2}\b",
        r"\b\d{1,2}/\d{1,2}/\d{2,4}\b",
        r"(?i)\b(?:jan|feb|mar|apr|may|jun|jul|aug|sep|oct|nov|dec)[a-z]*\.?\s+\d{1,2}(?:st|nd|rd|th)?(?:,?\s+\d{4})?\b",
        r"(?i)\b\d{1,2}(?:st|nd|rd|th)?\s+(?:jan|feb|mar|apr|may|jun|jul|aug|sep|oct|nov|dec)[a-z]*\.?(?:,?\s+\d{4})?\b",
        r"\b\d{1,2}:\d{2}(?::\d{2})?\s?(?i:[ap]\.?m\.?)?",
        r"\d+(?:\.\d+)?\s?%",
        r"\+\d[\d\s().-]{7,}\d",
        // Reference numbers and anything long enough to be an identifier.
        r"\b[A-Z]{2,}[-/]?\d{3,}\b|\b\d{5,}\b",
    ]
    .iter()
    // Compiled rather than filtered: a pattern that will not build is a typo
    // in a constant, and quietly dropping it leaves that whole class of fact
    // exposed to the model with nothing to show for it.
    .map(|pattern| Regex::new(pattern).expect("a fact pattern must compile"))
    .collect()
});

/// Why a rewrite cannot be trusted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Violation {
    Missing(usize),
    Duplicated(usize),
    Unknown(String),
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Violation::Missing(n) => write!(f, "protected value {n} was dropped"),
            Violation::Duplicated(n) => write!(f, "protected value {n} was repeated"),
            Violation::Unknown(token) => write!(f, "the rewrite invented a placeholder: {token}"),
        }
    }
}

/// Text with its facts swapped out, and the values needed to put them back.
pub struct Protected {
    pub text: String,
    values: Vec<String>,
}

impl Protected {
    /// Replace every recognisable fact in `text` with a placeholder.
    pub fn new(text: &str) -> Self {
        let mut spans: Vec<(usize, usize)> = Vec::new();
        for pattern in PATTERNS.iter() {
            for found in pattern.find_iter(text) {
                // Patterns overlap by nature, and the earlier, more specific
                // one has already claimed the ground.
                let clashes = spans
                    .iter()
                    .any(|(start, end)| found.start() < *end && *start < found.end());
                if !clashes {
                    spans.push((found.start(), found.end()));
                }
            }
        }
        spans.sort_unstable();

        let mut out = String::with_capacity(text.len());
        let mut values = Vec::new();
        let mut cursor = 0;
        for (start, end) in spans {
            out.push_str(&text[cursor..start]);
            out.push_str(&format!("{PLACEHOLDER}{:02}]]", values.len()));
            values.push(text[start..end].to_owned());
            cursor = end;
        }
        out.push_str(&text[cursor..]);

        Self { text: out, values }
    }

    pub fn count(&self) -> usize {
        self.values.len()
    }

    /// Put the original values back, or say why the rewrite cannot be trusted.
    pub fn restore(&self, rewritten: &str) -> Result<String, Violation> {
        static TOKEN: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"\[\[F(\d+)\]\]").expect("a fixed pattern"));

        let mut seen: HashMap<usize, usize> = HashMap::new();
        for found in TOKEN.captures_iter(rewritten) {
            let index: usize = found[1]
                .parse()
                .map_err(|_| Violation::Unknown(found[0].to_owned()))?;
            if index >= self.values.len() {
                return Err(Violation::Unknown(found[0].to_owned()));
            }
            *seen.entry(index).or_default() += 1;
        }

        if let Some(index) = seen
            .iter()
            .find_map(|(index, count)| (*count > 1).then_some(*index))
        {
            return Err(Violation::Duplicated(index));
        }
        if let Some(index) = (0..self.values.len()).find(|i| !seen.contains_key(i)) {
            return Err(Violation::Missing(index));
        }

        Ok(TOKEN
            .replace_all(rewritten, |found: &regex::Captures| {
                let index: usize = found[1].parse().unwrap_or_default();
                self.values[index].clone()
            })
            .into_owned())
    }
}
