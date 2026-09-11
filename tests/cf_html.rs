//! CF_HTML is the highest-risk correctness area in the project: the header
//! carries byte offsets into its own payload, so every multi-byte character is
//! a chance to be off by a few.

use unslop::cf_html::{build, parse};

/// A header with fixed-width offsets, so substituting values never changes its
/// length. Mirrors what `build` emits, minus the wrapper markup.
fn header(start: usize, end: usize) -> String {
    format!("StartFragment:{start:010}\r\nEndFragment:{end:010}\r\n")
}

const SAMPLES: &[&str] = &[
    "",
    "plain ascii",
    "<p>hello</p>",
    "costs \u{a3}4,500",                          // pound sign, 2 bytes
    "it\u{2019}s \u{201c}quoted\u{201d}",         // curly quotes, 3 bytes each
    "ship it \u{1f680}\u{1f525}",                 // emoji, 4 bytes each
    "Bj\u{f6}rn Dvo\u{159}\u{e1}k",               // accented Latin
    "\u{4f60}\u{597d}\u{4e16}\u{754c}",           // CJK
    "<b>\u{a3}1</b> \u{2014} \u{1f600} \u{4e16}", // mixed
    "<table><tr><td>\u{1f4a1}</td></tr></table>",
];

#[test]
fn round_trips_unicode() {
    for sample in SAMPLES {
        assert_eq!(
            parse(&build(sample)).as_deref(),
            Some(*sample),
            "sample: {sample:?}"
        );
    }
}

/// `parse(build(x)) == x` over multi-byte characters, since a fixed sample list
/// cannot cover offset arithmetic exhaustively.
#[test]
fn round_trips_random_unicode() {
    const POOL: &[char] = &[
        'a',
        'Z',
        '<',
        '>',
        '&',
        ' ',
        '\n',
        '\u{a3}',
        '\u{2019}',
        '\u{2014}',
        '\u{e9}',
        '\u{4e16}',
        '\u{1f680}',
        '\u{1f4a1}',
    ];
    // xorshift64, so the cases are varied but reproducible without a dependency.
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        POOL[(state % POOL.len() as u64) as usize]
    };

    for len in 0..200 {
        let fragment: String = (0..len).map(|_| next()).collect();
        assert_eq!(parse(&build(&fragment)).as_deref(), Some(fragment.as_str()));
    }
}

#[test]
fn a_large_fragment_does_not_widen_the_offset_fields() {
    // Variable-width offsets would shift StartHTML and corrupt every paste.
    let big = "x".repeat(100_000);
    let (small, large) = (build(""), build(&big));
    let prefix = |payload: &[u8]| {
        payload
            .windows(6)
            .position(|w| w == b"<html>")
            .expect("header precedes markup")
    };
    assert_eq!(prefix(&small), prefix(&large));
    assert_eq!(parse(&large).as_deref(), Some(big.as_str()));
}

#[test]
fn the_wrapper_stays_outside_the_fragment() {
    // Otherwise pasting nests a whole document inside the target.
    let fragment = parse(&build("<i>mid</i>")).unwrap();
    assert_eq!(fragment, "<i>mid</i>");
}

#[test]
fn fragment_markers_beat_untrustworthy_offsets() {
    let body = "<html><body><!--StartFragment--><p>\u{a3}9</p><!--EndFragment--></body></html>";
    // Zeroed, reversed, overrunning and non-numeric offsets all appear in the wild.
    for bogus in [
        header(0, 0),
        header(100, 10),
        header(900, 1500),
        "StartFragment:abc\r\n".into(),
    ] {
        let payload = format!("{bogus}{body}");
        assert_eq!(
            parse(payload.as_bytes()).as_deref(),
            Some("<p>\u{a3}9</p>"),
            "header: {bogus:?}"
        );
    }
}

#[test]
fn unicode_outside_the_fragment_does_not_shift_it() {
    // Producers put Unicode in the wrapper, which moves every following offset.
    let payload = format!(
        "{}<html><body>\u{4e16}\u{754c}<!--StartFragment--><p>\u{a3}9</p><!--EndFragment-->\u{1f680}</body></html>",
        header(0, 0)
    );
    assert_eq!(parse(payload.as_bytes()).as_deref(), Some("<p>\u{a3}9</p>"));
}

#[test]
fn offsets_are_used_when_the_markers_are_absent() {
    let body = "<html><body><p>\u{a3}9</p></body></html>";
    let start = header(0, 0).len() + body.find("<p>").unwrap();
    let payload = format!("{}{body}", header(start, start + "<p>\u{a3}9</p>".len()));
    assert_eq!(parse(payload.as_bytes()).as_deref(), Some("<p>\u{a3}9</p>"));
}

#[test]
fn offsets_splitting_a_character_fall_back_instead_of_panicking() {
    let body = "<html><body><p>\u{1f680}</p></body></html>";
    let emoji_at = header(0, 0).len() + body.find('\u{1f680}').unwrap();
    // Deliberately land inside the 4-byte emoji, with no markers to recover from.
    let payload = format!("{}{body}", header(emoji_at + 1, emoji_at + 3));

    let parsed = parse(payload.as_bytes()).expect("must not panic");
    assert!(
        parsed.contains('\u{1f680}'),
        "expected whole characters, got {parsed:?}"
    );
}

#[test]
fn garbage_never_panics() {
    for raw in [
        b"".as_slice(),
        b"\xff\xfe\x00",
        b"Version:0.9\r\n",
        b"StartFragment:1\r\n\xff\xff",
        b"StartFragment:99999999999999999999\r\nEndFragment:1\r\n<p>x</p>",
    ] {
        let _ = parse(raw);
    }
}

#[test]
fn a_trailing_nul_terminator_is_not_part_of_the_fragment() {
    let mut payload = build("<p>ok</p>");
    payload.extend_from_slice(b"\0\0");
    assert_eq!(parse(&payload).as_deref(), Some("<p>ok</p>"));
}

#[test]
fn an_ansi_payload_still_yields_its_fragment() {
    // Some writers emit the ANSI code page despite CF_HTML requiring UTF-8.
    // The fragment must still be extracted rather than the whole wrapper.
    let mut payload = b"<html><body><!--StartFragment--><p>".to_vec();
    payload.push(0xa3); // a lone Windows-1252 pound sign: invalid UTF-8
    payload.extend_from_slice(b"9</p><!--EndFragment--></body></html>");

    let parsed = parse(&payload).expect("must not panic");
    assert!(
        parsed.starts_with("<p>"),
        "expected just the fragment, got {parsed:?}"
    );
    assert!(
        parsed.ends_with("9</p>"),
        "expected just the fragment, got {parsed:?}"
    );
    assert!(
        !parsed.contains("<html>"),
        "wrapper leaked into the fragment: {parsed:?}"
    );
}

#[test]
fn ansi_payloads_keep_their_punctuation() {
    // Windows-1252 0x80-0x9F is exactly the em dash, en dash and curly quotes
    // this tool exists to find. Decoding them lossily would destroy the
    // characters we are meant to fix, and mangle currency along the way.
    let mut payload = b"<html><body><!--StartFragment--><p>".to_vec();
    payload.extend_from_slice(&[0x97, 0x96, 0x91, 0x92, 0x93, 0x94, 0x85, 0xa3]);
    payload.extend_from_slice(b"4,500</p><!--EndFragment--></body></html>");

    let parsed = parse(&payload).expect("must not panic");
    assert!(parsed.contains('\u{2014}'), "em dash lost: {parsed:?}");
    assert!(parsed.contains('\u{2013}'), "en dash lost: {parsed:?}");
    assert!(parsed.contains('\u{2019}'), "curly quote lost: {parsed:?}");
    assert!(
        parsed.contains("\u{a3}4,500"),
        "currency mangled: {parsed:?}"
    );
    assert!(
        !parsed.contains('\u{fffd}'),
        "characters were destroyed: {parsed:?}"
    );
}
