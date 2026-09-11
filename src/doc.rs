//! What was on the clipboard, and therefore what we put back.

/// A plain source stays plain: we never synthesise HTML for terminal text.
#[derive(Debug, Clone, PartialEq)]
pub enum Doc {
    Plain(String),
    /// `html` is the CF_HTML fragment. `text` is the source application's own
    /// plain rendering, which is better than anything we would derive from the
    /// markup ourselves.
    Rich {
        html: String,
        text: String,
    },
}

impl Doc {
    /// The plain-text view, used for the rules pass and for CF_UNICODETEXT.
    pub fn text(&self) -> &str {
        match self {
            Doc::Plain(text) | Doc::Rich { text, .. } => text,
        }
    }

    /// Whether there is anything here worth working on. Whitespace is not
    /// content, and markup with no words in it is an empty paragraph rather
    /// than something to rewrite.
    pub fn is_blank(&self) -> bool {
        self.text().trim().is_empty()
    }

    pub fn is_rich(&self) -> bool {
        matches!(self, Doc::Rich { .. })
    }
}
