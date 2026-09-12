//! Run the real clipboard capture through simplify N times.
use unslop::{cf_html, config::{Config, Mode}, doc::Doc, rewrite, rules::Rules};
fn main() {
    let raw = std::fs::read(std::env::args().nth(1).unwrap()).unwrap();
    let runs: usize = std::env::args().nth(2).unwrap().parse().unwrap();
    let html = cf_html::parse(&raw).unwrap();
    let doc = Doc::Rich { html: html.clone(), text: unslop::markdown::from_html(&html).unwrap() };
    let rules = Rules::load(include_str!("../rules/slop-rules.json")).unwrap();
    let config = Config::default();
    let (mut ok, mut fail) = (0, 0);
    for i in 1..=runs {
        match rewrite::run(&rules, &doc, &config, Some(8127), Mode::Simplify) {
            Ok(_) => { ok += 1; println!("run {i}: ok"); }
            Err(e) => { fail += 1; println!("run {i}: REJECTED: {e}"); }
        }
    }
    println!("\n{ok} ok / {fail} rejected");
}
