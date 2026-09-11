//! Clipboard HTML is untrusted: it can carry scripts, event handlers, frames
//! and remote images that would each break the promise that nothing leaves the
//! machine.

use unslop::{doc::Doc, render::preview};

fn rich(html: &str) -> String {
    preview(&Doc::Rich {
        html: html.into(),
        text: String::new(),
    })
}

#[test]
fn active_content_is_stripped() {
    let cases = [
        (
            "<script>fetch('https://evil.example.com')</script><p>hi</p>",
            "fetch",
        ),
        ("<p onclick=\"steal()\">hi</p>", "onclick"),
        ("<img src=x onerror=\"steal()\">", "onerror"),
        (
            "<iframe src=\"https://evil.example.com\"></iframe><p>hi</p>",
            "iframe",
        ),
        ("<object data=\"evil.swf\"></object><p>hi</p>", "object"),
        ("<embed src=\"evil.swf\"><p>hi</p>", "embed"),
    ];
    for (input, forbidden) in cases {
        let out = rich(input);
        assert!(!out.contains(forbidden), "{forbidden:?} survived: {out:?}");
    }
}

#[test]
fn remote_resources_cannot_be_fetched() {
    // A tracking pixel would phone home the moment the preview rendered.
    let out = rich("<p>hi</p><img src=\"https://evil.example.com/pixel.gif\">");
    assert!(
        !out.contains("evil.example.com"),
        "remote image survived: {out:?}"
    );
}

#[test]
fn executable_urls_are_neutralised() {
    let out = rich("<a href=\"javascript:steal()\">click</a>");
    assert!(
        !out.contains("javascript:"),
        "javascript URL survived: {out:?}"
    );
}

#[test]
fn formatting_worth_previewing_is_kept() {
    let out = rich("<p>It's <b>bold</b> and <i>italic</i></p><table><tr><td>a</td></tr></table>");
    for kept in ["<b>", "<i>", "<table>", "<td>"] {
        assert!(out.contains(kept), "{kept} was stripped: {out:?}");
    }
}

#[test]
fn plain_text_is_escaped_not_interpreted() {
    let out = preview(&Doc::Plain("<script>steal()</script>".into()));
    assert!(
        !out.contains("<script>"),
        "plain text was treated as markup: {out:?}"
    );
    assert!(
        out.contains("&lt;script&gt;"),
        "expected escaped text, got {out:?}"
    );
}

#[test]
fn plain_text_keeps_its_line_breaks() {
    // Terminal output collapses into one paragraph without a preformatted block.
    assert!(preview(&Doc::Plain("one\ntwo".into())).starts_with("<pre>"));
}

// The diff shows both the original clipboard text and the result, so it is a
// second place untrusted content reaches the page.

#[test]
fn diff_marks_what_changed() {
    let out = unslop::render::diff("it is a delve into this", "it is a look into this");
    assert!(
        out.contains("<del>delve</del>"),
        "no deletion marked: {out:?}"
    );
    assert!(
        out.contains("<ins>look</ins>"),
        "no insertion marked: {out:?}"
    );
    // `clean_text` escapes spaces, so unchanged words are checked one by one.
    for kept in ["it", "into", "this"] {
        assert!(
            out.contains(kept),
            "unchanged word {kept:?} was lost: {out:?}"
        );
    }
}

#[test]
fn identical_text_diffs_to_nothing_marked() {
    let out = unslop::render::diff("same words", "same words");
    assert!(!out.contains("<del>") && !out.contains("<ins>"), "{out:?}");
}

#[test]
fn diff_escapes_both_sides() {
    let out = unslop::render::diff("<script>a()</script>", "<script>b()</script>");
    assert!(
        !out.contains("<script>"),
        "markup survived the diff: {out:?}"
    );
    assert!(
        out.contains("&lt;script&gt;"),
        "expected escaped text: {out:?}"
    );
}
