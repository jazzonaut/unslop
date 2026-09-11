//! First-run installation of the model weights and the llama.cpp runtime.
//!
//! Neither is shipped with the application. The weights are several gigabytes
//! and the CUDA runtime is most of another, and nothing starts without the
//! user asking: that much arriving unannounced on a tethered laptop is a bad
//! first impression. The rules pass works throughout, so the tool is useful
//! during the download and useful if it never happens at all.

use std::{
    fs,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::Command,
    thread,
};

use tao::event_loop::EventLoopProxy;

use crate::{
    Message,
    download::{self, Progress},
    model::CREATE_NO_WINDOW,
    paths,
};

const MODEL_URL: &str =
    "https://huggingface.co/unsloth/Qwen3.5-4B-GGUF/resolve/main/Qwen3.5-4B-Q6_K.gguf";
/// The exact size of the pinned quantisation. Our own downloads are atomic, so
/// they are never short, but a file put there by hand can be: without this a
/// truncated model looks installed and fails inside llama-server every time,
/// with no obvious way back.
const MODEL_BYTES: u64 = 3_525_956_768;

/// The llama.cpp build the runtime is pinned to. Bumping it is a deliberate
/// act: the prompt is tuned against a particular build's behaviour, so a new
/// one is measured before anybody is handed it.
const BUILD: &str = "b10905";
const RELEASE: &str = "https://github.com/ggml-org/llama.cpp/releases/download";

/// Where each backend unpacks. These are the paths `model::BACKENDS` probes,
/// so a download lands exactly where the search already looks.
const CUDA_DIR: &str = "runtime";
const VULKAN_DIR: &str = "runtime-vulkan";

/// Sizes are pinned rather than read from the API: a release asset never
/// changes, and the button has to name a number before anything is fetched.
const CUDA_BYTES: u64 = 254_078_211;
const CUDART_BYTES: u64 = 391_443_627;
const VULKAN_BYTES: u64 = 31_666_541;

#[derive(Debug, Clone, PartialEq)]
enum State {
    Missing,
    Downloading(Progress),
    Installed,
    Failed(String),
}

/// One thing that has to be on disk before a local rewrite can happen.
#[derive(Clone)]
struct Part {
    url: String,
    /// The file whose presence proves this part arrived.
    marker: PathBuf,
    /// Where to unpack a zip, or `None` to save straight to `marker`.
    unpack_to: Option<PathBuf>,
    /// Download size, for the button label and the progress total.
    bytes: u64,
    /// The exact size `marker` must be, where it is known.
    exact: Option<u64>,
    /// What to call this in a sentence.
    what: &'static str,
}

impl Part {
    fn present(&self) -> bool {
        match (fs::metadata(&self.marker), self.exact) {
            (Ok(meta), Some(exact)) => meta.len() == exact,
            (Ok(_), None) => true,
            (Err(_), _) => false,
        }
    }

    /// Fetch this part, unpacking it if it arrived as an archive.
    fn fetch(&self, on_progress: impl FnMut(Progress)) -> Result<(), Box<dyn std::error::Error>> {
        let Some(dir) = &self.unpack_to else {
            return download::to_file(&self.url, &self.marker, on_progress);
        };

        // Named after the asset, so the two archives that share a folder do
        // not collide while they are on their way down.
        let name = self.url.rsplit('/').next().unwrap_or("runtime.zip");
        let archive = dir.with_file_name(name);
        download::to_file(&self.url, &archive, on_progress)?;

        let unpacked = unzip(&archive, dir);
        if unpacked.is_err() {
            // A complete but unreadable archive would otherwise be reused for
            // ever, since a download skips a destination that already exists.
            let _ = fs::remove_file(&archive);
        }
        unpacked?;
        let _ = fs::remove_file(&archive);
        Ok(())
    }
}

pub struct Installer {
    /// The weights, which the model server is pointed at directly.
    model: PathBuf,
    parts: Vec<Part>,
    state: State,
}

impl Installer {
    /// `model` is `local.model` from the configuration, so the file the
    /// installer fetches and the file the server is pointed at cannot drift.
    pub fn new(model: &str) -> Self {
        let model = paths::beside_exe(model);
        let mut parts = vec![Part {
            url: MODEL_URL.to_owned(),
            marker: model.clone(),
            unpack_to: None,
            bytes: MODEL_BYTES,
            exact: Some(MODEL_BYTES),
            what: "model",
        }];
        parts.extend(backend_parts());

        let state = if parts.iter().all(Part::present) {
            State::Installed
        } else {
            State::Missing
        };
        Self {
            model,
            parts,
            state,
        }
    }

    /// Where the weights belong, installed or not.
    pub fn path(&self) -> &PathBuf {
        &self.model
    }

    /// The model file, if everything a local rewrite needs is actually there.
    pub fn model_path(&self) -> Option<&PathBuf> {
        (self.state == State::Installed).then_some(&self.model)
    }

    /// Begin downloading, unless it is already running or already done.
    pub fn start(&mut self, proxy: EventLoopProxy<Message>) {
        if matches!(self.state, State::Downloading(_) | State::Installed) {
            return;
        }

        let missing: Vec<Part> = self
            .parts
            .iter()
            .filter(|p| !p.present())
            .cloned()
            .collect();
        // A download skips a destination that already exists, so a file of the
        // wrong size has to go first or retrying would succeed instantly and
        // leave the same broken model in place.
        for part in &missing {
            if part.unpack_to.is_none() && part.marker.exists() {
                let _ = fs::remove_file(&part.marker);
            }
        }

        self.state = State::Downloading(Progress {
            downloaded: 0,
            total: None,
        });

        thread::spawn(move || {
            let total: u64 = missing.iter().map(|part| part.bytes).sum();
            // Progress is reported across the whole set, so a two archive
            // backend does not appear to finish and start again.
            let mut done = 0;
            for part in &missing {
                let reporter = proxy.clone();
                let outcome = part.fetch(|progress| {
                    let _ = reporter.send_event(Message::DownloadProgress(Progress {
                        downloaded: done + progress.downloaded,
                        total: Some(total),
                    }));
                });
                if let Err(err) = outcome {
                    let _ = proxy.send_event(Message::DownloadFinished(Err(err.to_string())));
                    return;
                }
                done += part.bytes;
            }
            let _ = proxy.send_event(Message::DownloadFinished(Ok(())));
        });
    }

    pub fn on_progress(&mut self, progress: Progress) {
        if matches!(self.state, State::Downloading(_)) {
            self.state = State::Downloading(progress);
        }
    }

    pub fn on_finished(&mut self, outcome: Result<(), String>) {
        self.state = match outcome {
            // Trust the disk rather than the exit status: a transfer can end
            // cleanly and still be short, and an archive can unpack into a
            // shape we were not expecting.
            Ok(()) if self.parts.iter().all(Part::present) => State::Installed,
            Ok(()) => State::Failed("the download did not complete".to_owned()),
            Err(err) => State::Failed(err),
        };
    }

    /// The label for the install button, or `None` when there is nothing to do.
    pub fn button(&self) -> Option<String> {
        match self.state {
            State::Missing => {
                let bytes = self.missing().map(|part| part.bytes).sum();
                Some(format!("Install {} ({})", self.missing_what(), size(bytes)))
            }
            State::Failed(_) => Some("Retry download".to_owned()),
            _ => None,
        }
    }

    /// A line describing installation, or `None` when it is not worth saying.
    pub fn note(&self) -> Option<String> {
        match &self.state {
            State::Missing => Some(format!(
                "Rules only until the {} is installed",
                self.missing_what()
            )),
            State::Downloading(progress) => Some(match progress.percent() {
                Some(percent) => format!("Downloading the {}, {percent}%", self.missing_what()),
                None => format!("Downloading the {}", self.missing_what()),
            }),
            // Resuming is automatic, so the retry button is the whole story.
            State::Failed(err) => Some(format!("Rules only, {err}")),
            State::Installed => None,
        }
    }

    fn missing(&self) -> impl Iterator<Item = &Part> {
        self.parts.iter().filter(|part| !part.present())
    }

    /// "model", "runtime", or "model and runtime", for whatever is absent.
    fn missing_what(&self) -> String {
        let mut what: Vec<&str> = self.missing().map(|part| part.what).collect();
        what.dedup();
        what.join(" and ")
    }
}

/// The runtime for whatever this machine can actually use.
///
/// The backend has to be chosen before any llama.cpp binary exists to ask, so
/// the NVIDIA driver is the only thing there is to go on. CUDA needs a second
/// archive carrying the CUDA runtime libraries, which unpacks into the same
/// folder. Vulkan is one archive and a twentieth of the size, and it is the
/// native path on AMD and Intel rather than a fallback.
fn backend_parts() -> Vec<Part> {
    if has_nvidia() {
        let dir = paths::beside_exe(CUDA_DIR);
        vec![
            zip_part(
                &format!("{RELEASE}/{BUILD}/llama-{BUILD}-bin-win-cuda-12.4-x64.zip"),
                &dir,
                "llama-server.exe",
                CUDA_BYTES,
            ),
            zip_part(
                &format!("{RELEASE}/{BUILD}/cudart-llama-bin-win-cuda-12.4-x64.zip"),
                &dir,
                "cudart64_12.dll",
                CUDART_BYTES,
            ),
        ]
    } else {
        let dir = paths::beside_exe(VULKAN_DIR);
        vec![zip_part(
            &format!("{RELEASE}/{BUILD}/llama-{BUILD}-bin-win-vulkan-x64.zip"),
            &dir,
            "llama-server.exe",
            VULKAN_BYTES,
        )]
    }
}

fn zip_part(url: &str, dir: &Path, marker: &str, bytes: u64) -> Part {
    Part {
        url: url.to_owned(),
        marker: dir.join(marker),
        unpack_to: Some(dir.to_owned()),
        bytes,
        exact: None,
        what: "runtime",
    }
}

/// Whether an NVIDIA driver is installed. `nvcuda.dll` arrives with the
/// driver, so it is there on any machine CUDA could work on and absent on
/// every machine where it could not.
fn has_nvidia() -> bool {
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_owned());
    Path::new(&root).join("System32/nvcuda.dll").exists()
}

/// Windows ships bsdtar, which reads zips. A zip crate would be a dependency
/// for something the platform has done since 2018.
fn unzip(archive: &Path, into: &Path) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(into)?;
    let status = Command::new("tar")
        .arg("-xf")
        .arg(archive)
        .arg("-C")
        .arg(into)
        // tar is a console application, and without this one flashes up.
        .creation_flags(CREATE_NO_WINDOW)
        .status()?;
    if !status.success() {
        return Err(format!("{} could not be unpacked", archive.display()).into());
    }
    Ok(())
}

/// Sizes as a person would say them, for a button label.
pub fn size(bytes: u64) -> String {
    const GB: u64 = 1024 * 1024 * 1024;
    if bytes >= GB {
        format!("{:.1}GB", bytes as f64 / GB as f64)
    } else {
        format!("{}MB", bytes / (1024 * 1024))
    }
}
