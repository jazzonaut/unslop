# Unslop

Copy some text, press a hotkey, and get it back without the AI tells. A
deterministic rule pass strips the stock phrasing instantly; a model running on
your own machine is then given a chance to improve on that, and its output is
only shown once it has been checked.

Windows only. Nothing leaves the machine unless you explicitly turn on a remote
provider.

## How it works

1. **Hotkey.** `CTRL+ALT+U` by default. Unslop reads the clipboard, preferring
   rich text so tables, lists and links survive the round trip.
2. **Rules pass.** A vendored pack of regex detectors, vocabulary tiers and
   phrase substitutions runs first. It is a linter that only rewrites what it
   can rewrite safely, so it is fast and trustworthy enough to be the baseline.
3. **Model pass.** The baseline is handed to a local `llama-server` for a
   better rewrite. Anything that cannot be verified is rejected and the
   baseline stands.
4. **Preview.** A popup appears without taking focus, so `Ctrl+V` still lands
   in whatever you were working in. It shows a word-level diff against what
   you copied. A dropdown in the footer picks what the model does: **Unslop** strips the tells and keeps
   everything else, **Simplify** rewrites in plain language with the
   repetition cut, and **tl;dr** condenses to about a quarter of the length.
   Changing it redoes the text on show, and the choice is remembered.
5. **Copy.** Nothing reaches the clipboard until you press Copy. Silently
   overwriting what someone just copied is not the tool's to do.

## What it will not do

The parts of a document that must not change are never shown to the model at
all. Prices, email addresses, dates, phone numbers, reference codes and URLs
are swapped for placeholders before the text is sent, and the original bytes
are put back afterwards. If a placeholder does not survive intact the rewrite
is rejected rather than patched up, on the theory that a model which dropped
one has probably rearranged the sentence around it too.

A rewrite is also refused if it explains the task instead of doing it, or if it
flattens a plain-text bullet list into prose.

## Download

The latest zip is on the [releases page](https://github.com/jazzonaut/unslop/releases/latest).
Unpack it wherever you like. There is no installer: `unslop.exe` is
self-contained, `config.toml` appears beside it on first run, and the rule pack
travels in the zip so `rules.path` has something to point at.

Windows will announce that it protected your PC, because the executable is not
signed. "More info", then "Run anyway". Every release also carries a `.sha256`
file, if you would rather check the download than trust it:

```
Get-FileHash unslop-v0.3.4-x86_64-windows.zip -Algorithm SHA256
```

Or build it yourself.

## Building

Rust 2024 edition and a recent stable toolchain.

```
cargo build --release
```

The weights and the llama.cpp runtime are not bundled: several gigabytes
arriving unannounced is a poor first impression. On first run the popup offers
each as a download, naming the size before it fetches anything. The rules pass
works throughout, and works fine if you never install either.

| Download | Size |
| --- | --- |
| Qwen3.5-4B Q6_K weights | 3.5 GB |
| llama.cpp CUDA runtime, plus cudart | 254 MB + 391 MB |
| llama.cpp Vulkan runtime | 32 MB |

CUDA is preferred where it works. On a laptop 4070 the same rewrite takes 1.7s
on CUDA and 20.6s on Vulkan for identical output, but Vulkan is the native path
on AMD and Intel, so both builds are offered.

## Configuration

`config.toml` is written next to the executable on first run, with all of its
comments intact. Delete it to get the documented defaults back. Edit, save,
then restart. Anything the application has to say for itself, a rule pack that
will not parse or a server that would not start, goes to `unslop.log` in the
same folder. The settings worth knowing about:

- `hotkey` - a global hotkey is taken system wide, so pick accordingly.
- `rewrite.provider` - `local`, `remote`, or `off`. `off` is a genuine option
  rather than a degraded one: the rules pass alone fixes filler phrases and em
  dashes instantly.
- `rewrite.chunk_chars` - how much text the model is handed at a time. Longer
  clipboard content is cut at blank lines into groups of about this size, each
  rewritten in its own call and joined back up. Small on purpose: the 4B edits
  a short text well and a long one barely at all.
- `rewrite.max_total_chars` - the most text the model pass takes on at all.
  Above it the rules result is what you get, and the popup says so.
- `local.idle_unload_mins` - how long an idle model server keeps its VRAM. 8 GB
  is not enough to hold the weights resident all day, so it is handed back and
  the server restarted on demand.
- `rules.path` - point this at a copy of `rules/slop-rules.json` to add your own
  tells without rebuilding. A pack that will not parse is reported and ignored
  rather than taking the application down.

### Remote providers

Setting `rewrite.provider = "remote"` sends your clipboard text to a third
party, who may log it. This is the one setting that breaks the promise the rest
of the tool is built around. Placeholders still protect the facts, but the prose
around them leaves the machine.

Any OpenAI-compatible endpoint works. Leave `remote.api_key` empty and the
`OPENROUTER_API_KEY` environment variable is used instead, which keeps the key
out of the config file and out of your backups.

## Why this model

Measured over 12 samples x 5 runs, twice, scored against the rule pack's own
detectors. The figure is AI tells left per run, out of the 53 the rules pass
cannot fix on its own.

| Model | Tells left | VRAM |
| --- | --- | --- |
| Qwen3-8B Q4_K_M | 0.2 | 5325 MB |
| **Qwen3.5-4B Q6_K** | **0.7** | **3634 MB** |
| Qwen3.5-4B Q4_K_M | 2.2 | 3127 MB |
| Qwen3-4B Q4_K_M | 10.0 | 3226 MB |

Generation quality rather than parameter count was what failed in the older 4B.
Q6_K over Q4_K_M because the coarser quantisation measurably loses adherence,
and 500 MB is the cheapest thing here to spend. No facts were lost across 420
rewrites.

## Tests and benchmarks

```
cargo test
```

The tests that need a running `llama-server` are marked `#[ignore]`.

To score a model yourself:

```
cargo run --release --example bench -- bench/corpus.json 5 models/your.gguf > bench.jsonl
python bench/score.py bench.jsonl
```

The benchmark runs the real pipeline rather than talking to `llama-server`
directly, so the prompt, the placeholders and the rejection checks are the ones
that ship.

## Licence

MIT, see [LICENSE](LICENSE).

The rule pack in `rules/` is third-party MIT work with its own notice, which is
preserved in [rules/LICENSE](rules/LICENSE). The pattern research behind it
ultimately derives from Wikipedia:Signs of AI writing, maintained by WikiProject
AI Cleanup, under CC BY-SA.
