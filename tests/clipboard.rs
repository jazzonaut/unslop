//! Real clipboard round-trips. Ignored by default because they take over the
//! machine's clipboard; run with `cargo test -- --ignored --test-threads=1`.

use unslop::{clip, doc::Doc};

fn round_trip(doc: Doc) {
    clip::write(&doc).expect("write");
    assert_eq!(clip::read().expect("read"), Some(doc));
}

#[test]
fn whitespace_is_not_content() {
    // The hotkey used to open no window at all for a blank clipboard, which
    // from the outside is indistinguishable from the app having died.
    for blank in ["", " ", "\n\t  \r\n"] {
        assert!(
            Doc::Plain(blank.into()).is_blank(),
            "{blank:?} passed as text"
        );
    }
    let empty_markup = Doc::Rich {
        html: "<p>&nbsp;</p>".into(),
        text: " ".into(),
    };
    assert!(empty_markup.is_blank(), "empty markup passed as content");
    assert!(!Doc::Plain("  x  ".into()).is_blank(), "real text refused");
}

#[test]
#[ignore = "uses the real clipboard"]
fn rich_html_survives_a_round_trip() {
    round_trip(Doc::Rich {
        html: "<p><b>\u{a3}4,500</b> by March 12 \u{1f680} \u{4e16}\u{754c}</p>".into(),
        text: "\u{a3}4,500 by March 12 \u{1f680} \u{4e16}\u{754c}".into(),
    });
}

#[test]
#[ignore = "uses the real clipboard"]
fn a_table_survives_a_round_trip() {
    round_trip(Doc::Rich {
        html: "<table><tr><td>a</td><td>\u{a3}1</td></tr></table>".into(),
        text: "a\t\u{a3}1".into(),
    });
}

#[test]
#[ignore = "uses the real clipboard"]
fn plain_text_stays_plain() {
    // A plain source must not gain HTML, or pasting into a terminal shows markup.
    clip::write(&Doc::Plain("cargo test \u{2014} \u{a3}9".into())).expect("write");
    let read = clip::read()
        .expect("read")
        .expect("something on the clipboard");
    assert!(!read.is_rich(), "plain source came back as rich: {read:?}");
    assert_eq!(read.text(), "cargo test \u{2014} \u{a3}9");
}
