//! The model pass must improve on the deterministic result or get out of the
//! way. It is never allowed to make things worse.

use unslop::{config::Config, doc::Doc, rewrite, rules::Rules};

const PORT: u16 = 8127;

fn rules() -> Rules {
    Rules::load(include_str!("../rules/slop-rules.json")).expect("vendored pack")
}

#[test]
fn the_prompt_carries_the_packs_own_tells() {
    // One file drives both passes, so a tell added to the pack reaches the
    // model without anyone editing a prompt string.
    let prompt = rewrite::system_prompt(&rules());
    for expected in ["delve", "tapestry", "em dash", "[[F00]]"] {
        assert!(
            prompt.contains(expected),
            "{expected:?} missing from the prompt"
        );
    }
    // Short on purpose: the pack's detector notes measurably hurt adherence.
    assert!(prompt.len() < 2200, "the prompt is drifting long again");
}

#[test]
fn text_too_long_for_the_context_window_is_refused() {
    let long = Doc::Plain("word ".repeat(4000));
    // No port at all proves it never reached the network.
    assert_eq!(
        rewrite::run(&rules(), &long, &Config::default(), None),
        Err(rewrite::Rejected::TooLong)
    );
}

#[test]
fn an_unreachable_model_leaves_the_baseline_alone() {
    let doc = Doc::Plain("In order to ship we cut scope.".into());
    // Nothing is listening on this port.
    assert!(matches!(
        rewrite::run(&rules(), &doc, &Config::default(), Some(9)),
        Err(rewrite::Rejected::Model(_))
    ));
}

#[test]
#[ignore = "needs llama-server running"]
fn a_real_rewrite_removes_the_tells_and_keeps_the_facts() {
    let slop = "In today's rapidly evolving digital landscape, it is important to note that our \
                robust and seamless platform doesn't just streamline your workflow, it \
                fundamentally transforms it. By leveraging cutting-edge technology, we foster a \
                vibrant ecosystem that empowers teams to delve into their data. Please send the \
                \u{a3}4,500 invoice to billing@example.com by March 12.";

    let out = match rewrite::run(
        &rules(),
        &Doc::Plain(slop.into()),
        &Config::default(),
        Some(PORT),
    ) {
        Ok(doc) => doc.text().to_owned(),
        Err(err) => panic!("rewrite rejected: {err}"),
    };
    println!("\n--- REWRITE ---\n{out}\n");

    // The facts never reached the model, so these are guaranteed, not hoped for.
    for fact in ["\u{a3}4,500", "billing@example.com", "March 12"] {
        assert!(out.contains(fact), "lost {fact}: {out}");
    }
    // The second deterministic pass guarantees this one even if the model slips.
    assert!(!out.contains('\u{2014}'), "an em dash came back: {out}");
    for banned in [
        "delve",
        "leverag",
        "robust",
        "seamless",
        "vibrant",
        "foster",
        "ecosystem",
    ] {
        assert!(
            !out.to_lowercase().contains(banned),
            "{banned} survived: {out}"
        );
    }
    // Structural tells need naming explicitly; the pack's notes are written
    // for a human and are too abstract for an 8B to act on.
    let lower = out.to_lowercase();
    assert!(
        !lower.contains("in today"),
        "scene-setting opener survived: {out}"
    );
    assert!(
        !lower.contains("n't just"),
        "negative parallelism survived: {out}"
    );
}

/// Not a test: writes the live prompt out so it can be A/B tested against the
/// model without a rebuild for every change.
#[test]
#[ignore = "writes the prompt for tuning"]
fn dump_prompt() {
    let path = std::env::temp_dir().join("unslop-prompt.txt");
    std::fs::write(&path, rewrite::system_prompt(&rules())).unwrap();
    println!("{}", path.display());
}

#[test]
#[ignore = "needs llama-server running"]
fn rich_text_keeps_its_table_through_a_rewrite() {
    // Without the Markdown round trip the rewrite returns plain text, which is
    // worse than not rewriting at all.
    let doc = Doc::Rich {
        html: "<p>In order to ship, it is important to note that the costs below are final.</p>\
               <table><tr><td>Item</td><td>Cost</td></tr><tr><td>Build</td><td>\u{a3}4,500</td></tr></table>"
            .into(),
        text: "In order to ship, the costs below are final. Item Cost Build \u{a3}4,500".into(),
    };

    match rewrite::run(&rules(), &doc, &Config::default(), Some(PORT)) {
        Ok(out @ Doc::Rich { .. }) => {
            let Doc::Rich { html, .. } = &out else {
                unreachable!()
            };
            println!("\n--- REWRITTEN HTML ---\n{html}\n");
            assert!(html.contains("<table"), "the table was lost: {html}");
            assert!(html.contains("\u{a3}4,500"), "the price was lost: {html}");
        }
        Ok(other) => panic!("rich text was flattened to {other:?}"),
        // A rejection is acceptable; silently losing the table is not.
        Err(err) => println!("rejected, baseline stands: {err}"),
    }
}

#[test]
#[ignore = "diagnostic"]
fn show_markdown_conversion() {
    for html in [
        "<table><tr><td>Item</td><td>Cost</td></tr><tr><td>Build</td><td>\u{a3}4,500</td></tr></table>",
        "<table><thead><tr><th>Item</th><th>Cost</th></tr></thead><tbody><tr><td>Build</td><td>\u{a3}4,500</td></tr></tbody></table>",
        "<ul><li>one</li><li>two</li></ul>",
        "<p>text with <a href=\"https://example.com\">a link</a></p>",
    ] {
        let md = unslop::markdown::from_html(html).unwrap_or_default();
        println!(
            "\nHTML: {html}\n  MD: {md:?}\n  back: {}",
            unslop::markdown::to_html(&md).replace('\n', " ")
        );
    }
}

#[test]
fn a_flattened_list_is_damage_even_with_no_markup_to_lose() {
    // What the plain path now checks. Measured on Qwen3.5-4B: it turns a
    // bullet list into a paragraph in 14 of 15 rewrites, where the 8B never
    // does, and nothing else in the pipeline objects because the text is the
    // right length and every fact survives.
    use unslop::markdown::{Shape, to_html};

    let list = "Key takeaways:\n\n- Growth: revenue rose.\n- Retention: churn fell.\n";
    let flattened = "Key takeaways: revenue rose and churn fell.";
    assert_eq!(
        Shape::of_html(&to_html(list)).lost(Shape::of_html(&to_html(flattened))),
        Some("the list was lost")
    );

    // Ordinary prose has no shape to lose, so an ordinary rewrite of it must
    // not be rejected. Autolinks are off, so a bare URL stays text.
    let prose = "We cut the export step because nobody used it, see example.com.";
    let rewritten = "The export step is gone. Nobody used it, see example.com.";
    assert_eq!(
        Shape::of_html(&to_html(prose)).lost(Shape::of_html(&to_html(rewritten))),
        None
    );

    // Editing a list is not vandalising it: only a count falling to zero is.
    let merged = "Key takeaways:\n\n- Revenue rose and churn fell.\n";
    assert_eq!(
        Shape::of_html(&to_html(list)).lost(Shape::of_html(&to_html(merged))),
        None
    );
}

#[test]
fn a_backend_is_found_on_this_machine() {
    // No backend means no model pass at all, which is a silent loss of half
    // the product, so it is worth failing loudly in the suite.
    let chosen = unslop::model::choose_backend();
    match &chosen {
        Some(exe) => println!("backend: {}", exe.display()),
        None => println!("no GPU backend available, rules only"),
    }
    // Only assert when the bundled runtime is actually present.
    if std::path::Path::new("runtime/llama-server.exe").exists() {
        assert!(
            chosen.is_some(),
            "a bundled backend exists but none was chosen"
        );
    }
}

#[test]
fn instructions_echoed_back_are_not_a_rewrite() {
    // Observed from the 8B: handed slop, it explained how to fix slop instead
    // of fixing it. Same length and no protected values, so nothing else in
    // the pipeline had a reason to object.
    let before = "Let's take a deep dive into the intricate tapestry of controls, \
                  due to the fact that the seamless interplay is paramount.";
    let after = "Controls are important. The way they work together matters a lot. \
                 Don't use fancy words like \"tapestry\" or \"seamless.\" Just say \
                 what's needed.";
    assert!(rewrite::is_commentary(before, after));

    // A genuine rewrite of the same text must still get through.
    assert!(!rewrite::is_commentary(
        before,
        "Controls matter, and how they work together matters more."
    ));
}

#[test]
fn a_document_may_keep_saying_what_it_already_said() {
    // The marker is only a leak when the model brought it along. A style
    // guide that says "do not use" keeps the right to say so.
    let before = "Our house style: do not use the passive voice in headlines.";
    let after = "House style: do not use the passive voice in headlines.";
    assert!(!rewrite::is_commentary(before, after));
}
