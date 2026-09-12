//! The model pass must improve on the deterministic result or get out of the
//! way. It is never allowed to make things worse.

use unslop::{
    config::{Config, Mode},
    doc::Doc,
    rewrite,
    rules::{Finding, Rules},
};

const PORT: u16 = 8127;

fn rules() -> Rules {
    Rules::load(include_str!("../rules/slop-rules.json")).expect("vendored pack")
}

/// Slop-heavy on purpose, covering vocabulary, a fix-less phrase and a regex
/// detector so one text exercises all three sources of an edit target.
const SLOPPY: &str = "In today's rapidly evolving market we delve into the intricate \
                      tapestry of controls. The stakes are significant.";

#[test]
fn the_prompt_carries_the_packs_own_tells() {
    // One file drives both passes, so a tell added to the pack reaches the
    // model without anyone editing a prompt string.
    let prompt = rewrite::system_prompt(&rules(), SLOPPY, 2);
    for expected in [
        "delve",
        "tapestry",
        "in today's rapidly evolving",
        "The stakes are significant",
        "em dash",
        "[[F00]]",
    ] {
        assert!(
            prompt.contains(expected),
            "{expected:?} missing from the prompt"
        );
    }
    // Short on purpose: the pack's detector notes measurably hurt adherence,
    // and the checklist is the only part allowed to grow with the document.
    assert!(prompt.len() < 2600, "the prompt is drifting long again");
}

#[test]
fn the_prompt_names_only_what_is_present() {
    // The point of the checklist over a global word list. Text that never said
    // "delve" must not be told to remove it: a 4B given a word to avoid will
    // find somewhere to avoid it, and the edit it makes is not an improvement.
    let prompt = rewrite::system_prompt(&rules(), "We cut the export step. Nobody used it.", 0);
    assert!(
        !prompt.contains("delve"),
        "a word from another document leaked"
    );
    assert!(
        prompt.contains("No specific rule-pack hit"),
        "clean text should be told there is nothing to chase"
    );
}

#[test]
fn the_placeholder_rule_is_absent_when_nothing_is_protected() {
    // Prose with no price or date in it protects nothing, and a 4B shown the
    // example anyway copies [[F00]] into its answer. Restore then rejects a
    // rewrite that was fine, so the rule is only stated when it applies.
    let prompt = rewrite::system_prompt(&rules(), SLOPPY, 0);
    assert!(
        !prompt.contains("[[F"),
        "the example placeholder is still there to be copied"
    );
    assert!(
        prompt.contains("delve"),
        "the rest of the prompt went with it"
    );
}

#[test]
fn text_too_long_for_the_context_window_is_refused() {
    let long = Doc::Plain("word ".repeat(4000));
    // No port at all proves it never reached the network.
    assert_eq!(
        rewrite::run(&rules(), &long, &Config::default(), None, Mode::Unslop),
        Err(rewrite::Rejected::TooLong)
    );
}

#[test]
fn an_unreachable_model_leaves_the_baseline_alone() {
    let doc = Doc::Plain("In order to ship we cut scope.".into());
    // Nothing is listening on this port.
    assert!(matches!(
        rewrite::run(&rules(), &doc, &Config::default(), Some(9), Mode::Unslop),
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
        Mode::Unslop,
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
    std::fs::write(&path, rewrite::system_prompt(&rules(), SLOPPY, 2)).unwrap();
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

    match rewrite::run(&rules(), &doc, &Config::default(), Some(PORT), Mode::Unslop) {
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

/// A finding as the pack would have produced it, so the two scoring policies
/// can be pinned without depending on which words the vendored pack lists.
fn finding(matched: &str, weight: u8, tier: Option<u8>) -> Finding {
    Finding {
        id: matched.to_owned(),
        matched: matched.to_owned(),
        category: "test".to_owned(),
        weight,
        tier,
    }
}

#[test]
fn the_gate_counts_shapes_and_the_repair_pass_counts_words() {
    let hits = vec![
        finding("testament", 5, Some(1)),
        finding("negative contrast", 3, None),
        finding("valuable", 1, Some(3)),
    ];
    // Both strong hits are worth spending a repair call on.
    assert_eq!(rewrite::repair_score(&hits), 8);
    // Only the shape is worth rejecting the whole rewrite over. The prompt asks
    // for precise vocabulary to be kept, so a model that keeps "testament"
    // where it belongs must not lose its other edits for it.
    assert_eq!(rewrite::gate_score(&hits), 3);
}

#[test]
fn vocabulary_alone_never_blocks_publication() {
    // "Robust statistics" and "crucial for p99" are ordinary technical English.
    // Counting them at the gate rejects a good rewrite of a technical document.
    let words = vec![
        finding("robust", 5, Some(1)),
        finding("crucial", 5, Some(1)),
    ];
    assert_eq!(rewrite::gate_score(&words), 0);
}

#[test]
fn the_checklist_leaves_out_what_the_rules_pass_tolerates() {
    // oxford-triple and curly quotes are weight 1 and fire on ordinary writing,
    // and the rules pass leaves both alone on purpose. Listing one asks the
    // model for an edit nobody wants and spends a checklist line doing it.
    let prompt = rewrite::system_prompt(&rules(), "The flag takes red, green, and blue.", 0);
    assert!(
        prompt.contains("No specific rule-pack hit"),
        "a weight-one hit reached the checklist: {prompt}"
    );
}

#[test]
fn an_empty_or_runaway_answer_is_not_a_rewrite() {
    assert!(rewrite::check_length("hello world", "").is_err());
    assert!(rewrite::check_length("a short line", &"word ".repeat(50)).is_err());
    assert!(
        rewrite::check_length(
            "This is a moderately long sentence that needs editing.",
            "This sentence needs editing."
        )
        .is_ok()
    );
}

#[test]
fn a_summary_is_told_how_long_and_that_it_may_drop_facts() {
    // "About a quarter" is a guess to a 4B; a word count is a target. And the
    // placeholder rule has to change with the job, or the model is told to
    // keep every fact in a text it was told to cut to a quarter.
    let text = "word ".repeat(400);
    let prompt = rewrite::condense_prompt(Mode::Tldr, &text, 2);
    assert!(prompt.contains("about 100 words"), "{prompt}");
    assert!(prompt.contains("leave out the rest"), "{prompt}");
    assert!(prompt.contains("em dash"), "the hard bans went missing");

    // Very short text is not asked to shrink below what it can say.
    let short = rewrite::condense_prompt(Mode::Tldr, "Ship it on Monday.", 0);
    assert!(short.contains("about 4 words"), "{short}");
    assert!(!short.contains("[[F"), "no protected values, no placeholder rule");

    // Simplify keeps every point, so it keeps the strict rule.
    let simplify = rewrite::condense_prompt(Mode::Simplify, &text, 1);
    assert!(simplify.contains("Never drop"), "{simplify}");
    assert!(simplify.contains("Keep lists, tables"), "{simplify}");
}

#[test]
fn both_prompts_are_told_how_many_bullet_points_to_return() {
    // "Keep lists" is the kind of abstract instruction this model ignores:
    // measured on two bulleted texts it flattened the list into prose in 10 of
    // 10 runs and every one was refused, where naming the count kept it in 10
    // of 10. The count must match what `validate_structure` counts, or the
    // prompt asks for something the guard will reject.
    let list = "Key takeaways:

- Growth: revenue rose.
- Retention: churn fell.

The board meets soon.";
    let prompt = rewrite::condense_prompt(Mode::Simplify, list, 0);
    assert!(prompt.contains("contains 2 bullet points"), "{prompt}");
    assert!(prompt.contains("the same 2 bullet points"), "{prompt}");

    // The prose around a list must not be swept into it. An earlier wording
    // said only "return exactly N bullet points" and the model turned the
    // lead-in and the closing paragraph into bullets too.
    assert!(prompt.contains("never turn a heading"), "{prompt}");

    // Unslop lost the same lists for the same reason, so it carries the same
    // rule. Said only once, in one prompt, the other keeps the bug.
    let unslop = rewrite::system_prompt(&rules(), list, 0);
    assert!(unslop.contains("contains 2 bullet points"), "{unslop}");

    // Text with no list is left exactly as it was, so the ordinary prompts are
    // unchanged for everything that has no list to lose.
    let prose = rewrite::condense_prompt(Mode::Simplify, "Just a sentence.", 0);
    assert!(!prose.contains("bullet point"), "{prose}");
    let plain = rewrite::system_prompt(&rules(), "Just a sentence.", 0);
    assert!(!plain.contains("bullet point"), "{plain}");
}

#[test]
fn a_paragraph_stays_a_paragraph_but_a_list_is_left_alone() {
    // Observed on Qwen3.5-4B in Simplify: an email arrives as one paragraph
    // and comes back with every sentence on its own line.
    let email = "Three vendors are done. Two are due by Friday. Budget is fine.";
    let lined = "Three vendors are done.\nTwo are due by Friday.\nBudget is fine.";
    assert_eq!(rewrite::keep_paragraphs(email, lined), email);

    // Two paragraphs in, two paragraphs out; only the breaks inside go.
    let two = "First point. More on it.\n\nSecond point.";
    let lined = "First point.\nMore on it.\n\nSecond point.";
    assert_eq!(rewrite::keep_paragraphs(two, lined), two);

    // Text that already had single line breaks is the author's business.
    let poem = "Roses are red\nViolets are blue";
    assert_eq!(rewrite::keep_paragraphs(poem, poem), poem);
    let crlf = "Roses are red\r\nViolets are blue";
    assert_eq!(rewrite::keep_paragraphs(crlf, poem), poem);

    // A list the model built is not flattened into "- a - b".
    let list = "Points:\n\n- one\n- two";
    assert_eq!(rewrite::keep_paragraphs(email, list), list);
    let numbered = "1. one\n2. two";
    assert_eq!(rewrite::keep_paragraphs(email, numbered), numbered);
}

#[test]
fn a_summary_only_has_to_be_shorter() {
    // check_length would reject a summary for dropping most of the text,
    // which is the job. The one thing a summary may not be is longer.
    let text = "This is a moderately long paragraph that says several things at length.";
    assert!(rewrite::check_shorter(text, "Several things.").is_ok());
    assert!(rewrite::check_shorter(text, "").is_err());
    assert!(rewrite::check_shorter(text, text).is_err());
    assert!(rewrite::check_shorter(text, &format!("{text} And more.")).is_err());
}

#[test]
#[ignore = "needs llama-server running"]
fn a_real_summary_is_short_and_keeps_the_facts_it_mentions() {
    let long = "The migration to the new billing system is scheduled for March 12. Before then \
                every team must export its open invoices, because the old system will be read \
                only from that date. Finance has confirmed the \u{a3}4,500 licence fee is paid. \
                Support will run both systems in parallel for two weeks so that customers who \
                phone in can still be looked up in either. Questions go to billing@example.com. \
                Teams that miss the export window will have their invoices migrated manually, \
                which takes about a day per team and delays their first statement.";
    let out = match rewrite::run(
        &rules(),
        &Doc::Plain(long.into()),
        &Config::default(),
        Some(PORT),
        Mode::Tldr,
    ) {
        Ok(doc) => doc.text().to_owned(),
        Err(err) => panic!("summary rejected: {err}"),
    };
    println!("\n--- SUMMARY ---\n{out}\n");
    assert!(out.chars().count() < long.chars().count() / 2, "not much of a summary: {out}");
    // Whatever it kept, it kept exactly. Restore guarantees this.
    for fact in ["March 12", "\u{a3}4,500", "billing@example.com"] {
        if out.contains(&fact[..3]) {
            assert!(out.contains(fact), "a fact was altered: {out}");
        }
    }
}

#[test]
fn a_bold_bullet_is_a_finding_but_never_an_edit_target() {
    // The pack's inline-header-list detector fires on `- **Label**:` bullets.
    // The prompt tells the model to keep lists as they are, so listing the
    // bullet asks for two contradictory things, and counting it at the gate
    // would reject a rewrite that fixed everything else because the bullets it
    // was told to keep are still there.
    let text = "- **Growth**: revenue rose.\n- **Retention**: churn fell.\n";
    let prompt = rewrite::system_prompt(&rules(), text, 0);
    assert!(
        !prompt.contains("**Growth**"),
        "the bullet reached the checklist: {prompt}"
    );

    let hits = rules().detect(text);
    assert!(
        hits.iter().any(|hit| hit.id == "inline-header-list"),
        "the detector still has to fire for scoring"
    );
    assert_eq!(rewrite::gate_score(&hits), 0);
    assert_eq!(rewrite::repair_score(&hits), 0);
}

/// Not a test: prints what the two condensing modes make of a technical and a
/// business text, so the prompts can be judged by eye against the live model.
#[test]
#[ignore = "needs llama-server running"]
fn show_condensed_outputs() {
    let technical = "The scheduler assigns each job to a worker using a weighted round-robin over \
        the healthy pool. Health is determined by a heartbeat every 5 seconds; a worker that \
        misses three consecutive heartbeats is marked unhealthy and its in-flight jobs are \
        requeued with their original priority. Because requeued jobs keep their priority, a \
        flapping worker can cause head-of-line blocking for lower-priority work, which is why \
        the pool now applies exponential backoff (base 2s, cap 60s) before readmitting a \
        worker that has flapped more than twice in 10 minutes. Note that backoff state is \
        held in memory on the scheduler, so a scheduler restart resets it; persisting it to \
        the coordination store is tracked in ticket SCHED-4412 and is planned for Q3.";
    let business = "Following our conversation last week, I wanted to circle back and provide a \
        comprehensive update on where things stand with the vendor onboarding initiative. \
        As you know, we have been working diligently to streamline the process and ensure \
        alignment across all stakeholders. At this point in time, three of the five vendors \
        have completed the security questionnaire, and we anticipate the remaining two will \
        do so by 30 September. The legal team has flagged that the standard MSA will need \
        amendments for the two EU-based vendors due to data residency requirements, which \
        may push their go-live to mid-October. Budget remains within the approved envelope \
        of \u{a3}120,000. Please let me know if you have any questions or concerns.";

    for (name, text) in [("TECHNICAL", technical), ("BUSINESS", business)] {
        for mode in [Mode::Simplify, Mode::Tldr] {
            let doc = Doc::Plain(text.to_owned());
            let start = std::time::Instant::now();
            let out = rewrite::run(&rules(), &doc, &Config::default(), Some(PORT), mode);
            println!(
                "\n--- {name} / {mode:?} ({} words -> {}, {:.1}s) ---",
                text.split_whitespace().count(),
                out.as_ref()
                    .map(|doc| doc.text().split_whitespace().count().to_string())
                    .unwrap_or_else(|_| "rejected".to_owned()),
                start.elapsed().as_secs_f32()
            );
            match out {
                Ok(doc) => println!("{}", doc.text()),
                Err(err) => println!("REJECTED: {err}"),
            }
        }
    }
}

#[test]
fn a_headings_invisible_permalink_is_not_a_fact_to_protect() {
    // Copying a rendered README out of GitHub brings one of these along beside
    // every heading. It shows nothing, but it used to reach `facts` as a URL,
    // and the model was then asked to copy a token it had every reason to drop
    // after rewriting the heading. Every simplify of a GitHub page failed with
    // "protected value 0 was dropped".
    use unslop::{facts::Protected, markdown::{Shape, from_html}};

    let html = "<h1>Unslop</h1>                <a id=\"user-content-unslop\" class=\"anchor\"                 href=\"https://github.com/jazzonaut/unslop#unslop\"></a>                <p>Copy some text, press a hotkey.</p>";
    let markdown = from_html(html).expect("markdown");
    assert!(!markdown.contains("github.com"), "got {markdown:?}");
    assert_eq!(Protected::new(&markdown).count(), 0);

    // And the shape check must agree, or the rewrite is rejected for losing
    // links that were never visible in the first place.
    assert_eq!(Shape::of_html(html).links, 0);

    // A link with something to click on is still a link, in both.
    let real = "<p>see <a href=\"https://example.com\">the docs</a></p>";
    assert_eq!(Shape::of_html(real).links, 1);
    assert_eq!(Protected::new(&from_html(real).unwrap()).count(), 1);
}
