//! The rules pass is the trusted baseline: whatever it produces goes on the
//! clipboard before the model has said anything, so it has to be conservative
//! and it has to be right.

use unslop::rules::Rules;

fn rules() -> Rules {
    Rules::load(include_str!("../rules/slop-rules.json")).expect("vendored pack must parse")
}

fn clean(text: &str) -> String {
    rules().clean(text).text
}

#[test]
fn the_vendored_pack_yields_usable_fixes() {
    // Most of the pack is detection-only; this guards against a future pack
    // silently leaving us with nothing to apply.
    assert!(rules().phrase_count() >= 15, "too few usable phrases");
}

#[test]
fn filler_phrases_are_replaced() {
    assert_eq!(clean("We did it in order to ship."), "We did it to ship.");
    assert_eq!(
        clean("Slow due to the fact that it blocks."),
        "Slow because it blocks."
    );
    assert_eq!(clean("It has the ability to scale."), "It can scale.");
}

#[test]
fn capitalisation_survives_a_replacement() {
    assert_eq!(
        clean("In order to ship, we cut scope."),
        "To ship, we cut scope."
    );
    assert_eq!(clean("we did it in order to ship"), "we did it to ship");
}

#[test]
fn deleted_phrases_hand_the_capital_to_the_next_word() {
    assert_eq!(
        clean("It is important to note that the build is green."),
        "The build is green."
    );
    assert_eq!(
        clean("Also, it is worth noting that tests pass."),
        "Also, tests pass."
    );
}

#[test]
fn em_dashes_become_hyphens() {
    // The highest-signal tell of the lot, so it always goes.
    assert_eq!(clean("It works\u{2014}mostly."), "It works - mostly.");
    assert_eq!(clean("It works \u{2014} mostly."), "It works - mostly.");
    assert_eq!(clean("It works -- mostly."), "It works -- mostly.");
}

#[test]
fn numeric_ranges_keep_their_dash() {
    assert_eq!(clean("See pages 10\u{2013}12."), "See pages 10-12.");
    // An en dash between words is doing the job of an em dash.
    assert_eq!(clean("Ship it \u{2013} today."), "Ship it - today.");
}

#[test]
fn phrases_only_match_whole_words() {
    // "in order to" must not fire inside a longer word.
    let input = "The reorder took time and the disorder toppled it.";
    assert_eq!(clean(input), input);
}

#[test]
fn already_natural_text_is_left_alone() {
    // The baseline must not mangle prose that was fine to begin with.
    for text in [
        "Shipped the parser today. Tests pass, so I merged it.",
        "Can you look at the clipboard bug before standup?",
        "We cut the release because the offsets were wrong.",
    ] {
        assert_eq!(clean(text), text, "rewrote clean prose");
    }
}

#[test]
fn the_pass_is_idempotent() {
    let slop = "In order to ship, it is important to note that we delve \u{2014} deeply \u{2014} \
                into the tapestry due to the fact that it has the ability to scale.";
    let once = clean(slop);
    assert_eq!(clean(&once), once, "a second pass changed the text again");
}

#[test]
fn unicode_is_not_corrupted() {
    for text in [
        "Costs \u{a3}4,500 \u{1f680}",
        "Bj\u{f6}rn wrote \u{4e16}\u{754c}",
        "caf\u{e9} in order to relax",
    ] {
        let out = clean(text);
        assert!(
            out.chars().all(|c| c != '\u{fffd}' && c != '\u{0}'),
            "corrupted: {out:?}"
        );
    }
    assert_eq!(clean("caf\u{e9} in order to relax"), "caf\u{e9} to relax");
}

#[test]
fn counts_what_it_fixed() {
    let result =
        rules().clean("In order to ship \u{2014} we cut scope due to the fact that it slipped.");
    assert_eq!(result.fixes, 3, "got {:?}", result.text);
}

#[test]
fn a_malformed_pack_is_an_error_not_a_panic() {
    // The pack is user-editable, so it must never take down the application.
    for bad in ["", "{", "{}", r#"{"phrases": "nonsense"}"#] {
        assert!(
            Rules::load(bad).is_err(),
            "accepted malformed pack: {bad:?}"
        );
    }
}

// --- rich text ---------------------------------------------------------

use unslop::doc::Doc;

fn clean_rich(html: &str) -> String {
    match rules()
        .clean_doc(&Doc::Rich {
            html: html.into(),
            text: String::new(),
        })
        .doc
    {
        Doc::Rich { html, .. } => html,
        other => panic!("a rich document must stay rich, got {other:?}"),
    }
}

#[test]
fn markup_is_left_alone_while_its_text_is_cleaned() {
    let out = clean_rich("<p>We shipped it <b>in order to</b> land the release.</p>");
    assert!(
        out.contains("<b>to</b>"),
        "text inside markup was not cleaned: {out:?}"
    );
    assert!(
        out.contains("<p>") && out.contains("</p>"),
        "markup was lost: {out:?}"
    );
}

#[test]
fn table_structure_survives() {
    // Tables are the structure most likely to be destroyed, and the one users
    // will notice immediately when pasting into Word.
    let out = clean_rich(
        "<table><tr><td>Cost</td><td>\u{a3}4,500</td></tr><tr><td>Due</td><td>March 12</td></tr></table>",
    );
    for kept in ["<table>", "<tr>", "<td>", "\u{a3}4,500", "March 12"] {
        assert!(out.contains(kept), "{kept} was lost: {out:?}");
    }
    assert_eq!(out.matches("<td>").count(), 4, "cells changed: {out:?}");
}

#[test]
fn attributes_and_links_are_untouched() {
    let out = clean_rich(r#"<a href="https://example.com/x?a=b" title="in order to">link</a>"#);
    assert!(
        out.contains("https://example.com/x?a=b"),
        "href was altered: {out:?}"
    );
    // Attribute values are not prose and must not be rewritten.
    assert!(
        out.contains(r#"title="in order to""#),
        "an attribute was cleaned: {out:?}"
    );
}

#[test]
fn em_dashes_inside_markup_are_normalised() {
    let out = clean_rich("<p>It works\u{2014}mostly.</p>");
    assert!(
        out.contains("It works - mostly."),
        "em dash survived: {out:?}"
    );
}

#[test]
fn code_samples_are_not_prose() {
    // A snippet is quoted verbatim: swapping a phrase or an em dash inside one
    // changes what it says.
    let out = clean_rich("<p>Run <code>git log \u{2014}oneline</code> in order to check.</p>");
    assert!(
        out.contains("log \u{2014}oneline"),
        "a code sample was rewritten: {out:?}"
    );
    assert!(out.contains("to check"), "prose was not cleaned: {out:?}");
}

#[test]
fn style_and_script_contents_are_not_prose() {
    let out = clean_rich("<style>.a{content:'in order to'}</style><p>in order to ship</p>");
    assert!(
        out.contains("content:'in order to'"),
        "stylesheet was rewritten: {out:?}"
    );
    assert!(
        out.contains("<p>to ship</p>"),
        "prose was not cleaned: {out:?}"
    );
}

#[test]
fn the_plain_companion_is_cleaned_too() {
    // Terminals and Notepad receive this half, so it cannot be left sloppy.
    let cleaned = rules().clean_doc(&Doc::Rich {
        html: "<p>in order to ship</p>".into(),
        text: "in order to ship".into(),
    });
    assert_eq!(cleaned.doc.text(), "to ship");
}

#[test]
fn plain_documents_stay_plain() {
    let cleaned = rules().clean_doc(&Doc::Plain("in order to ship".into()));
    assert_eq!(cleaned.doc, Doc::Plain("to ship".into()));
}

#[test]
fn a_phrase_after_an_em_dash_is_still_found() {
    // Byte-wise word detection glues "table" to "it" across the dash, and the
    // phrase after it is silently never matched.
    assert_eq!(
        clean("In order to ship\u{2014}it is important to note that the table is final."),
        "To ship - the table is final."
    );
}

#[test]
fn a_deleted_phrase_takes_the_punctuation_that_set_it_off() {
    // Deleting the words but leaving their comma is worse than the phrase
    // was: it strands a comma against the previous full stop.
    assert_eq!(
        clean("We shipped. At the end of the day, this is fine."),
        "We shipped. This is fine."
    );
    // Set off by a comma on each side, both belong to the phrase.
    assert_eq!(
        clean("The plan, at the end of the day, was vague."),
        "The plan was vague."
    );
    // A phrase with nothing around it still reads the same as before.
    assert_eq!(clean("It is important to note that data wins."), "Data wins.");
}

#[test]
fn a_phrase_with_no_safe_substitute_is_left_for_the_model() {
    // "take a deep dive into" has no one-word noun that survives both the
    // article and the preposition, so the pack must not claim one.
    let out = clean("Let's take a deep dive into the data.");
    assert!(!out.contains("a analysis"), "left a broken article: {out}");
    assert_eq!(out, "Let's take a deep dive into the data.");
}
