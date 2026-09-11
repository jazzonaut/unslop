//! Applying a text transformation to HTML without disturbing its markup.
//!
//! Parsing and reserialising normalises the source, so this is
//! structure-preserving rather than byte-lossless: tags,
//! attributes, table structure, inline styles and links all survive, but the
//! bytes may not be identical.

use kuchikiki::{NodeRef, traits::*};

/// Apply `transform` to every text node in `html`.
///
/// A phrase split across elements, such as `not <b>just</b> speed`, spans two
/// text nodes and so cannot be matched here. That is left to the model rather
/// than reconstructed with DOM offsets.
pub fn map_text(html: &str, mut transform: impl FnMut(&str) -> String) -> String {
    let document = kuchikiki::parse_html().one(html);

    for node in document.inclusive_descendants() {
        let Some(text) = node.as_text() else {
            continue;
        };
        if is_code(&node) {
            continue;
        }
        // The borrow must end before the write, or this panics at runtime.
        let replaced = transform(&text.borrow());
        *text.borrow_mut() = replaced;
    }

    // Parsing a fragment as a document hoists `<style>` and `<link>` into the
    // head, so both halves are serialised or that markup would be dropped.
    let section = |name| {
        document
            .select_first(name)
            .ok()
            .map(|section| {
                section
                    .as_node()
                    .children()
                    .map(|child| child.to_string())
                    .collect::<String>()
            })
            .unwrap_or_default()
    };

    let reserialised = section("head") + &section("body");
    if reserialised.is_empty() && !html.trim().is_empty() {
        // Nothing recognisable came back; the original is safer than nothing.
        return html.to_owned();
    }
    reserialised
}

/// Whether a node's text is code rather than prose.
///
/// `code` and `pre` are included: a snippet is quoted verbatim, so swapping a
/// phrase or an em dash inside one changes what it says.
fn is_code(node: &NodeRef) -> bool {
    node.ancestors().any(|ancestor| {
        ancestor
            .as_element()
            .is_some_and(|el| matches!(el.name.local.as_ref(), "script" | "style" | "code" | "pre"))
    })
}
