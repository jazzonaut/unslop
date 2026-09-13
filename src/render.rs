//! Turning clipboard content into markup that is safe to show in the preview.
//!
//! Sanitising only affects what is *rendered*. The document we copy back keeps
//! the original markup, so nothing here costs the user any paste fidelity.

use std::{sync::LazyLock, time::Duration};

use similar::{ChangeTag, TextDiff};

use crate::doc::Doc;

static SANITISER: LazyLock<ammonia::Builder<'static>> = LazyLock::new(|| {
    let mut builder = ammonia::Builder::default();
    // Images are the one remaining way for a preview to reach the network, and
    // a tracking pixel would quietly break the local-only guarantee.
    builder.rm_tags(["img"]);
    builder
});

/// Markup safe to inject into the preview document.
pub fn preview(doc: &Doc) -> String {
    match doc {
        Doc::Rich { html, .. } => SANITISER.clean(html).to_string(),
        // Plain text is shown as-is, escaped, so terminal output keeps its
        // line breaks instead of collapsing into a paragraph.
        Doc::Plain(text) => format!("<pre>{}</pre>", ammonia::clean_text(text)),
    }
}

/// A word-level diff of the original clipboard text against what was published.
///
/// The plain-text view of both documents is compared rather than the markup: an
/// HTML diff is mostly tag noise, and the wording is what the user is deciding
/// about. Every piece of either side is escaped before it is wrapped, so the
/// diff cannot smuggle markup past the sanitiser.
pub fn diff(original: &str, published: &str) -> String {
    // Bounded: this runs on the UI thread for every update, and Myers over two
    // long texts that share little can take seconds. Past the deadline
    // `similar` finishes with a coarser diff that is still correct.
    let diff = TextDiff::configure()
        .timeout(Duration::from_millis(300))
        .diff_words(original, published);
    let mut out = String::from("<pre>");
    for change in diff.iter_all_changes() {
        let text = ammonia::clean_text(change.value());
        out.push_str(&match change.tag() {
            ChangeTag::Delete => format!("<del>{text}</del>"),
            ChangeTag::Insert => format!("<ins>{text}</ins>"),
            ChangeTag::Equal => text,
        });
    }
    out.push_str("</pre>");
    out
}
