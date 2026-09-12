//! Measure a model against the corpus: how many tells survive, how fast, and
//! how often the rewrite is rejected.
//!
//! Runs the real pipeline rather than talking to llama-server directly, so the
//! prompt, the fact placeholders, the rejection checks and the second rules
//! pass are all the ones that ship. Scoring happens outside: this writes one
//! JSON line per rewrite and nothing else.
//!
//!     cargo run --release --example bench -- corpus.json 5 models/a.gguf models/b.gguf

use std::{
    env, fs,
    path::Path,
    time::{Duration, Instant},
};

use unslop::{
    config::{Config, Mode},
    doc::Doc,
    model::{self, Model},
    rewrite,
    rules::Rules,
};

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let [corpus, runs, weights @ ..] = args.as_slice() else {
        eprintln!("usage: bench <corpus.json> <runs> <weights.gguf>...");
        std::process::exit(2);
    };
    let runs: usize = runs.parse().expect("runs must be a number");
    let corpus: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(corpus).expect("corpus")).expect("corpus json");
    let corpus = corpus.as_array().expect("corpus is an array");

    let rules = Rules::load(include_str!("../rules/slop-rules.json")).expect("vendored pack");
    let config = Config::default();
    let exe = model::choose_backend().expect("no llama-server backend sees a device");

    for weights in weights {
        let name = Path::new(weights)
            .file_stem()
            .unwrap()
            .to_string_lossy()
            .into_owned();

        // VRAM is a delta: Windows does not report it per process, so the
        // reading before the server starts is the only baseline available.
        let before = vram_used();
        // Longer than the app's own timeout: a cold read off a spinning disk
        // is not what is being measured here.
        let mut engine = Model::new(&exe, weights, Duration::from_secs(3600), &config.local);
        let load = Instant::now();
        let port = engine.port().expect("llama-server did not start");
        if !model::wait_until_ready(port, Duration::from_secs(300)) {
            eprintln!("{name}: never became ready");
            continue;
        }
        let load = load.elapsed();
        // The first rewrite still pays for warmup, so settle before reading.
        std::thread::sleep(Duration::from_secs(2));
        let vram = vram_used().saturating_sub(before);
        eprintln!("{name}: ready in {load:.1?}, {vram} MiB");

        for run in 0..runs {
            for item in corpus {
                let id = item["id"].as_str().unwrap_or("?");
                let text = item["text"].as_str().expect("text");
                let doc = Doc::Plain(text.to_owned());

                let started = Instant::now();
                let outcome = rewrite::run(&rules, &doc, &config, Some(port), Mode::Unslop);
                let ms = started.elapsed().as_millis();

                // The rules-only result is what stands whenever the model pass
                // is rejected, so it is the baseline every score is against.
                let baseline = rules.clean(text).text;
                let (output, rejected) = match outcome {
                    Ok(doc) => (doc.text().to_owned(), serde_json::Value::Null),
                    Err(err) => (baseline.clone(), err.to_string().into()),
                };
                println!(
                    "{}",
                    serde_json::json!({
                        "model": name, "run": run, "id": id, "ms": ms,
                        "input": text, "baseline": baseline,
                        "output": output, "rejected": rejected,
                        "facts": item["facts"], "load_ms": load.as_millis(), "vram_mib": vram,
                    })
                );
            }
            eprintln!("{name}: run {} of {runs} done", run + 1);
        }
    }
}

/// Total GPU memory in use, in MiB. Zero where nvidia-smi is not available,
/// which only blanks the VRAM column.
fn vram_used() -> u64 {
    let out = std::process::Command::new("nvidia-smi")
        .args(["--query-gpu=memory.used", "--format=csv,noheader,nounits"])
        .output();
    out.ok()
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .and_then(|text| text.lines().next()?.trim().parse().ok())
        .unwrap_or(0)
}
