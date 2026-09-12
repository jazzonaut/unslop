//! Run the shipped pipeline against a running llama-server.
//!
//!     cargo run --release --example probe -- sample.txt 8127 [runs] [--verbose]
//!     cargo run --release --example probe -- bench/corpus.json 8127 [runs]
//!
//! Sums surviving tells the way the bench scorer does: tier-1 words, fix-less
//! phrases and regex detectors of weight 2 or more.

use std::{env, fs, time::Instant};

use unslop::{
    config::{Config, Mode},
    doc::Doc,
    rewrite,
    rules::{Finding, Rules},
};

fn score(findings: &[Finding]) -> usize {
    findings
        .iter()
        .filter(|f| f.tier == Some(1) || (f.tier.is_none() && f.weight >= 2))
        .count()
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let path = &args[0];
    let port: u16 = args[1].parse().expect("port");
    let runs: usize = args.get(2).and_then(|r| r.parse().ok()).unwrap_or(1);
    let verbose = args.iter().any(|a| a == "--verbose");
    let mode = if args.iter().any(|a| a == "--simplify") {
        Mode::Simplify
    } else if args.iter().any(|a| a == "--tldr") {
        Mode::Tldr
    } else {
        Mode::Unslop
    };
    let rules = Rules::load(include_str!("../rules/slop-rules.json")).expect("pack");
    let config = Config::default();

    let items: Vec<(String, String)> = if path.ends_with(".json") {
        let corpus: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        corpus
            .as_array()
            .unwrap()
            .iter()
            .map(|i| {
                (
                    i["id"].as_str().unwrap().to_owned(),
                    i["text"].as_str().unwrap().to_owned(),
                )
            })
            .collect()
    } else {
        vec![(
            "sample".to_owned(),
            fs::read_to_string(path).unwrap().trim().to_owned(),
        )]
    };

    let (mut total_before, mut total_after, mut rejected) = (0, 0, 0);
    for (id, text) in &items {
        let baseline = rules.clean(text).text;
        let before = rules.detect(&baseline);
        if verbose {
            println!("== {id} baseline ==\n{baseline}\n");
            for f in &before {
                println!(
                    "  w{} tier{:?} {} :: {:?}",
                    f.weight, f.tier, f.id, f.matched
                );
            }
            println!(
                "\n== prompt ==\n{}\n",
                rewrite::system_prompt(&rules, &baseline, 0)
            );
        }
        for run in 0..runs {
            let started = Instant::now();
            let outcome = rewrite::run(
                &rules,
                &Doc::Plain(baseline.clone()),
                &config,
                Some(port),
                mode,
            );
            let ms = started.elapsed().as_millis();
            total_before += score(&before);
            match outcome {
                Ok(doc) => {
                    let after = rules.detect(doc.text());
                    total_after += score(&after);
                    let left: Vec<&str> = after
                        .iter()
                        .filter(|f| f.tier == Some(1) || (f.tier.is_none() && f.weight >= 2))
                        .map(|f| f.matched.as_str())
                        .collect();
                    println!(
                        "## {id} run {run} OK {ms}ms tells {}->{} words {}->{} left {left:?}\n{}\n",
                        score(&before),
                        score(&after),
                        baseline.split_whitespace().count(),
                        doc.text().split_whitespace().count(),
                        doc.text()
                    );
                }
                Err(err) => {
                    rejected += 1;
                    total_after += score(&before);
                    println!(
                        "## {id} run {run} REJECTED {ms}ms tells {} :: {err}\n",
                        score(&before)
                    );
                }
            }
        }
    }
    println!(
        "TOTAL tells {total_before} -> {total_after}, rejected {rejected} of {}",
        items.len() * runs
    );
}
