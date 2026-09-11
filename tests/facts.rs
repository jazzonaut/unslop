//! Detecting a mangled figure afterwards is not enough when the text is a
//! client email, so facts never reach the model at all.

use unslop::facts::{Protected, Violation};

#[test]
fn the_things_that_must_not_change_are_hidden() {
    let protected = Protected::new(
        "Send the \u{a3}4,500 invoice to billing@example.com by March 12, see https://example.com/x?a=b, \
         21% is due, ref INV-20394, call +44 20 7123 4567.",
    );
    for fact in [
        "\u{a3}4,500",
        "billing@example.com",
        "March 12",
        "https://example.com/x?a=b",
        "21%",
        "INV-20394",
    ] {
        assert!(
            !protected.text.contains(fact),
            "{fact} was left exposed: {}",
            protected.text
        );
    }
    assert!(
        protected.count() >= 6,
        "only protected {}",
        protected.count()
    );
}

#[test]
fn an_untouched_rewrite_restores_exactly() {
    let original = "Send \u{a3}4,500 to billing@example.com by March 12.";
    let protected = Protected::new(original);
    assert_eq!(protected.restore(&protected.text).unwrap(), original);
}

#[test]
fn rewording_around_the_placeholders_is_fine() {
    // The whole point: the model may rewrite freely between the facts.
    let protected = Protected::new("Please kindly send the \u{a3}4,500 invoice by March 12.");
    let rewritten = protected.text.replace("Please kindly send the", "Send");
    let restored = protected.restore(&rewritten).unwrap();
    assert_eq!(restored, "Send \u{a3}4,500 invoice by March 12.");
}

#[test]
fn a_dropped_fact_rejects_the_rewrite() {
    let protected = Protected::new("Send \u{a3}4,500 by March 12.");
    let lost = protected.text.replace("[[F00]]", "some money");
    assert!(matches!(
        protected.restore(&lost),
        Err(Violation::Missing(_))
    ));
}

#[test]
fn a_repeated_fact_rejects_the_rewrite() {
    let protected = Protected::new("Send \u{a3}4,500 by March 12.");
    let doubled = format!("{} And again by [[F01]].", protected.text);
    assert!(matches!(
        protected.restore(&doubled),
        Err(Violation::Duplicated(_))
    ));
}

#[test]
fn an_invented_placeholder_rejects_the_rewrite() {
    let protected = Protected::new("Send \u{a3}4,500 by March 12.");
    let invented = format!("{} Also [[F99]].", protected.text);
    assert!(matches!(
        protected.restore(&invented),
        Err(Violation::Unknown(_))
    ));
}

#[test]
fn facts_inside_a_url_are_not_protected_twice() {
    // A URL contains things that look like dates and numbers; nesting a
    // placeholder inside one would corrupt the link.
    let protected = Protected::new("See https://example.com/2024-01-30/invoice/99999 today.");
    assert_eq!(
        protected.count(),
        1,
        "the URL was split: {}",
        protected.text
    );
    assert_eq!(
        protected.restore(&protected.text).unwrap(),
        "See https://example.com/2024-01-30/invoice/99999 today."
    );
}

#[test]
fn prose_with_nothing_to_protect_is_untouched() {
    let plain = "We shipped the parser today and the tests pass.";
    let protected = Protected::new(plain);
    assert_eq!(protected.text, plain);
    assert_eq!(protected.count(), 0);
    assert_eq!(protected.restore(plain).unwrap(), plain);
}

#[test]
fn ordinary_small_numbers_are_left_readable() {
    // Protecting every digit would hand the model unreadable text and make the
    // rewrite worse, so only identifier-length runs are hidden.
    let protected = Protected::new("We cut 3 of the 12 sections.");
    assert_eq!(protected.count(), 0, "over-protected: {}", protected.text);
}

#[test]
fn times_are_protected() {
    // A pattern that will not compile used to be dropped in silence, which
    // left this whole class of fact standing in front of the model.
    let protected = Protected::new("Call at 3:30pm, again at 09:15.");
    assert_eq!(
        protected.count(),
        2,
        "times were exposed: {}",
        protected.text
    );
    assert_eq!(
        protected.restore(&protected.text).unwrap(),
        "Call at 3:30pm, again at 09:15."
    );
}
