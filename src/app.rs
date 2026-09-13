//! What the application does when something happens.
//!
//! The deterministic result is shown immediately and is never taken away. The
//! model is given a chance to improve on it, and its output is only shown once
//! it has been checked. Nothing reaches the
//! clipboard until the user presses Copy: silently overwriting what someone
//! just copied is not ours to do.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use tao::event_loop::EventLoopProxy;

use crate::{
    Message, clip,
    config::{Config, Mode, Provider},
    doc::Doc,
    download::Progress,
    install::Installer,
    model::{self, Model},
    rewrite,
    rules::Rules,
    window::{Phase, Popup},
};

/// Long enough to cover loading 8B of weights from a cold start.
const READY_TIMEOUT: Duration = Duration::from_secs(90);

/// What the popup calls things in each mode.
struct Wording {
    /// The status pill while the model runs, and once it has finished.
    working: &'static str,
    done: &'static str,
    /// The status pill when the mode needs a model and there is none.
    no_model: &'static str,
}

fn wording(mode: Mode) -> Wording {
    match mode {
        Mode::Unslop => Wording {
            working: "Unslopping\u{2026}",
            done: "Unslopped",
            no_model: "Unslopped",
        },
        Mode::Simplify => Wording {
            working: "Simplifying\u{2026}",
            done: "Simplified",
            no_model: "No model to simplify with, rules only",
        },
        Mode::Tldr => Wording {
            working: "Summarising\u{2026}",
            done: "Summarised",
            no_model: "No model to summarise with, rules only",
        },
    }
}

/// What the popup says when the hotkey lands on nothing. An image or a file
/// copied from Explorer offers no text either, so this covers more than a
/// clipboard that is literally empty.
const EMPTY_CLIPBOARD: &str = "Nothing to unslop on the clipboard";
const UNREADABLE_CLIPBOARD: &str = "Could not read the clipboard";

/// How often the clipboard is checked while the popup is open, so a Ctrl+C in
/// another window is picked up without reaching for the hotkey again. Only the
/// sequence number is read, which costs one function call.
const CLIPBOARD_POLL: Duration = Duration::from_millis(250);

pub struct App {
    config: Arc<Config>,
    rules: Arc<Rules>,
    /// Absent when no bundled backend can see a GPU, in which case the
    /// deterministic pass is the whole product and still works.
    model: Option<Model>,
    installer: Installer,
    /// What the dropdown says the model pass should do.
    mode: Mode,
    /// Rises with every hotkey press, so a slow rewrite belonging to an older
    /// press can be recognised and discarded. Shared with the workers, so a
    /// superseded one can also stop asking.
    active_job: Arc<AtomicU64>,
    /// Workers that have not reported back yet, superseded or not. While there
    /// are any the server is in use, whatever the idle clock says.
    in_flight: usize,
    /// The deterministic result, shown until the model improves on it.
    baseline: Option<Doc>,
    /// What was on the clipboard before anything touched it, kept only as text
    /// because that is all the diff compares.
    original: String,
    /// What is on show, and therefore what an explicit copy should publish.
    shown: Option<Doc>,
    /// The clipboard sequence number we last acted on, so the poll can tell a
    /// fresh Ctrl+C from our own Copy.
    seen_clipboard: u32,
}

impl App {
    pub fn new(config: Config, rules: Rules) -> Self {
        let backend = probe(&config);
        let installer = Installer::new(&config.local.model, backend.is_some());
        let model = build_model(&config, installer.path(), backend);
        Self {
            mode: config.mode,
            config: Arc::new(config),
            rules: Arc::new(rules),
            model,
            installer,
            active_job: Arc::new(AtomicU64::new(0)),
            in_flight: 0,
            baseline: None,
            original: String::new(),
            shown: None,
            seen_clipboard: 0,
        }
    }

    /// What the popup should tell the user about the setup right now.
    fn note(&self) -> Option<String> {
        setup_note(&self.config, self.installer.note(), self.model.is_some())
    }

    /// The install button, which belongs to the local provider alone. Nothing
    /// on this machine is missing when the rewriting happens somewhere else.
    fn install_button(&self) -> Option<String> {
        (self.config.rewrite.provider == Provider::Local)
            .then(|| self.installer.button())
            .flatten()
    }

    /// Unslop the clipboard, show it, then ask the model to do better.
    pub fn on_hotkey(&mut self, popup: &Popup, proxy: &EventLoopProxy<Message>) {
        self.unslop(popup, proxy, true);
    }

    /// `asked` marks a deliberate hotkey press, as opposed to the poll below.
    /// It decides only what happens when there is nothing to work on.
    fn unslop(&mut self, popup: &Popup, proxy: &EventLoopProxy<Message>, asked: bool) {
        // Recorded before the read, so a copy landing in between costs one
        // harmless repeat rather than going unnoticed.
        self.seen_clipboard = clip::sequence();
        let doc = match clip::read() {
            Ok(Some(doc)) => doc,
            Ok(None) => return self.nothing_to_do(popup, asked, EMPTY_CLIPBOARD),
            Err(err) => {
                eprintln!("clipboard read failed: {err}");
                return self.nothing_to_do(popup, asked, UNREADABLE_CLIPBOARD);
            }
        };

        self.original = doc.text().to_owned();
        self.baseline = Some(self.rules.clean_doc(&doc).doc);
        self.begin(popup, proxy);
    }

    /// Show the baseline and start the model pass on it in the current mode.
    /// Also where a mode change lands, so the same text is redone in place.
    fn begin(&mut self, popup: &Popup, proxy: &EventLoopProxy<Message>) {
        let Some(baseline) = self.baseline.clone() else {
            return;
        };
        self.shown = Some(baseline.clone());
        self.active_job.fetch_add(1, Ordering::Relaxed);

        // The rules pass is instant, so without saying that a second pass is
        // running the window looks finished the moment it appears. Unslop is
        // whole without a model; the other two are not, and say so.
        let words = wording(self.mode);
        let (phase, label) = match (self.start_rewrite(baseline.clone(), proxy), self.mode) {
            (Ok(true), _) => (Phase::Working, words.working.to_owned()),
            (Ok(false), Mode::Unslop) => (Phase::Done, words.done.to_owned()),
            (Ok(false), _) => (Phase::Warn, words.no_model.to_owned()),
            // A server that would not start is not a setup the note explains,
            // and a green "Unslopped" over it would say the pass happened.
            (Err(err), _) => (Phase::Warn, err),
        };

        popup.show(&baseline, &self.original);
        popup.set_phase(phase, &label);
        popup.set_mode(self.mode);
        popup.set_install(self.install_button(), self.note());
    }

    /// The dropdown changed: remember it, and redo whatever is on show.
    pub fn on_set_mode(&mut self, popup: &Popup, proxy: &EventLoopProxy<Message>, mode: Mode) {
        if mode == self.mode {
            return;
        }
        self.mode = mode;
        Config::remember_mode(mode);
        self.begin(popup, proxy);
    }

    /// Run the model pass again on the same baseline.
    ///
    /// Sampling is not deterministic, so a rewrite rejected for mangling a
    /// fact or leaving the tells in place is often fine on the next draw.
    /// Nothing is retried automatically: a second pass costs seconds of GPU
    /// time, and whether this one is worth it is the user's call.
    pub fn on_retry(&mut self, popup: &Popup, proxy: &EventLoopProxy<Message>) {
        self.begin(popup, proxy);
    }

    /// Notice a Ctrl+C that happened while the popup was open, and act on it.
    ///
    /// Polling rather than a clipboard listener keeps this out of the window
    /// procedure: the event loop already wakes on a deadline for the idle
    /// unload, so this rides on the same mechanism.
    // ponytail: a 250ms poll while the popup is visible; swap for
    // AddClipboardFormatListener if the wakeups ever show in a battery trace.
    pub fn poll_clipboard(&mut self, popup: &Popup, proxy: &EventLoopProxy<Message>) {
        if popup.is_visible() && clip::sequence() != self.seen_clipboard {
            self.unslop(popup, proxy, false);
        }
    }

    /// Answer a hotkey press that has nothing to work on.
    ///
    /// Returning in silence was the old behaviour, and a hotkey that opens no
    /// window reads as a crash: an empty clipboard, a copied image and a
    /// broken build all look identical from the outside. The poll is the one
    /// exception, since copying an image while the popup is open must not
    /// throw away the result the user is still reading.
    fn nothing_to_do(&mut self, popup: &Popup, asked: bool, message: &str) {
        if !asked {
            return;
        }
        // Cleared so Copy has nothing to publish, and so a rewrite still in
        // flight from an earlier press cannot land on top of the message.
        self.active_job.fetch_add(1, Ordering::Relaxed);
        self.baseline = None;
        self.shown = None;
        self.original.clear();

        popup.show(&Doc::Plain(String::new()), "");
        popup.set_phase(Phase::Warn, message);
        popup.set_install(self.install_button(), self.note());
    }

    /// When the event loop should next wake: for an idle server, or for the
    /// clipboard poll while the popup is on screen.
    pub fn wake_deadline(&self, popup_visible: bool) -> Option<Instant> {
        let poll = popup_visible.then(|| Instant::now() + CLIPBOARD_POLL);
        [self.idle_deadline(), poll].into_iter().flatten().min()
    }

    /// Publish a rewrite, unless a newer hotkey press has superseded it.
    pub fn on_rewrite(&mut self, popup: &Popup, job: u64, result: Result<Doc, String>) {
        // Every worker reports back exactly once, superseded or not, and the
        // idle clock runs from the end of the last job rather than its start.
        self.in_flight = self.in_flight.saturating_sub(1);
        if let Some(model) = &mut self.model {
            model.touch();
        }
        if job != self.active_job.load(Ordering::Relaxed) {
            return;
        }
        match result {
            Ok(doc) => {
                popup.update(&doc, &self.original);
                popup.set_phase(Phase::Done, wording(self.mode).done);
                self.shown = Some(doc);
            }
            // The baseline is already on show, so a failure costs nothing but
            // an explanation.
            Err(err) => {
                if let Some(baseline) = &self.baseline {
                    popup.update(baseline, &self.original);
                }
                popup.set_phase(Phase::Warn, &err);
            }
        }
    }

    /// Put the result on show on the clipboard. The only thing that writes to
    /// it, and only because the user asked.
    pub fn on_copy(&mut self, popup: &Popup) {
        let Some(shown) = self.shown.as_ref() else {
            return;
        };
        match clip::write(shown) {
            Ok(()) => {
                // Our own write bumps the sequence number, which the poll
                // would otherwise read as the user copying something new.
                self.seen_clipboard = clip::sequence();
                popup.set_phase(Phase::Done, "Copied to clipboard");
            }
            Err(err) => {
                eprintln!("clipboard write failed: {err}");
                popup.set_phase(Phase::Warn, "Could not copy the result");
            }
        }
    }

    pub fn install(&mut self, popup: &Popup, proxy: EventLoopProxy<Message>) {
        self.installer.start(proxy);
        popup.set_install(self.install_button(), self.note());
    }

    pub fn on_download_progress(&mut self, popup: &Popup, progress: Progress) {
        self.installer.on_progress(progress);
        popup.set_install(self.install_button(), self.note());
    }

    pub fn on_download_finished(&mut self, popup: &Popup, outcome: Result<(), String>) {
        // The backend was probed at startup, before any of this existed. A
        // runtime that has only just landed is no use until we look again, and
        // what it sees decides which runtime the installer offers next.
        if self.model.is_none() {
            let backend = probe(&self.config);
            self.model = build_model(&self.config, self.installer.path(), backend);
        }
        self.installer.on_finished(outcome, self.model.is_some());
        popup.set_install(self.install_button(), self.note());
    }

    /// When the event loop should next wake to check for an idle server. Not
    /// while a job is in flight: the server is in use however long ago it was
    /// started, and a chunked rewrite with repairs can outlast a short timeout.
    fn idle_deadline(&self) -> Option<Instant> {
        (self.in_flight == 0)
            .then(|| self.model.as_ref()?.idle_deadline())
            .flatten()
    }

    pub fn unload_if_idle(&mut self) {
        if self.in_flight == 0
            && let Some(model) = &mut self.model
        {
            model.unload_if_idle();
        }
    }

    /// Hand a rewrite to a worker thread. `Ok(true)` means one was started,
    /// `Ok(false)` that there is nothing to start and the setup note already
    /// says why, and `Err` a reason the user has not been told about.
    ///
    /// The server is started on demand rather than at launch: holding 5.3GB of
    /// VRAM all day for a tool used a few times an hour is not a fair trade on
    /// a laptop.
    fn start_rewrite(&mut self, doc: Doc, proxy: &EventLoopProxy<Message>) -> Result<bool, String> {
        let port = match self.config.rewrite.provider {
            Provider::Off => return Ok(false),
            // A remote provider needs no weights and no server of our own.
            Provider::Remote => {
                if !self.config.remote.is_usable() {
                    return Ok(false);
                }
                None
            }
            Provider::Local => {
                if self.installer.model_path().is_none() {
                    return Ok(false);
                }
                let Some(model) = &mut self.model else {
                    return Ok(false);
                };
                match model.port() {
                    Ok(port) => Some(port),
                    Err(err) => {
                        eprintln!("could not start the model server: {err}");
                        return Err(format!("could not start the model server: {err}"));
                    }
                }
            }
        };

        let rules = Arc::clone(&self.rules);
        let config = Arc::clone(&self.config);
        let active = Arc::clone(&self.active_job);
        let (job, mode, proxy) = (active.load(Ordering::Relaxed), self.mode, proxy.clone());
        self.in_flight += 1;
        thread::spawn(move || {
            let live = || active.load(Ordering::Relaxed) == job;
            let ready = port.is_none_or(|port| model::wait_until_ready(port, READY_TIMEOUT, live));
            let result = if ready {
                rewrite::run(&rules, &doc, &config, port, mode, &live)
                    .map_err(|rejected| rejected.to_string())
            } else {
                Err("the model server did not start".to_owned())
            };
            let _ = proxy.send_event(Message::RewriteFinished { job, result });
        });
        Ok(true)
    }
}

/// The fastest bundled backend that can see a GPU, when a local rewrite is
/// wanted at all.
///
/// Probed rather than assumed, which costs a few hundred milliseconds and
/// avoids running a CUDA build on an AMD laptop. Called at startup and again
/// after an install, since the answer changes when the runtime arrives.
fn probe(config: &Config) -> Option<PathBuf> {
    (config.rewrite.provider == Provider::Local)
        .then(model::choose_backend)
        .flatten()
}

/// The model server, when a local rewrite is both wanted and possible.
fn build_model(config: &Config, weights: &Path, backend: Option<PathBuf>) -> Option<Model> {
    // Saturating: the file is hand-edited, and a silly number should mean
    // "never" rather than a wrapped-around instant.
    let idle = Duration::from_secs(config.local.idle_unload_mins.saturating_mul(60));
    Some(Model::new(backend?, weights, idle, &config.local))
}

/// The one line of setup the popup shows, or nothing when there is nothing
/// worth saying.
///
/// The privacy warning takes precedence: a local model being absent matters
/// less than the clipboard leaving the machine, and when a remote provider is
/// configured the local weights are not used anyway.
///
/// A free function so the precedence can be tested without a five gigabyte
/// file on disk and a GPU in the machine.
pub fn setup_note(config: &Config, install: Option<String>, has_backend: bool) -> Option<String> {
    match config.rewrite.provider {
        // Nothing is sent without a model name and a key, and a note saying
        // it was would sit under a green "Unslopped" for a pass that never ran.
        Provider::Remote if !config.remote.is_usable() => {
            Some("remote provider needs a model and an API key, rules only".to_owned())
        }
        Provider::Remote => Some(format!("sending text to {}", config.remote.base_url)),
        // Nothing is missing when nothing local was asked for, so the install
        // state is none of the user's business here.
        Provider::Off => None,
        // A missing download is the louder problem and  says so.
        // This is the quieter one: everything is on disk and nothing on this
        // machine can run it, which would otherwise show as a green
        // "Unslopped" with no hint that the model pass never happened.
        Provider::Local => install
            .or_else(|| (!has_backend).then(|| "no GPU backend found, rules only".to_owned())),
    }
}
