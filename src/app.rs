//! What the application does when something happens.
//!
//! The deterministic result is shown immediately and is never taken away. The
//! model is given a chance to improve on it, and its output is only shown once
//! it has been checked. Nothing reaches the
//! clipboard until the user presses Copy: silently overwriting what someone
//! just copied is not ours to do.

use std::{
    sync::Arc,
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
    /// press can be recognised and discarded.
    active_job: u64,
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
    pub fn new(config: Config, rules: Rules, installer: Installer) -> Self {
        let model = build_model(&config, installer.path());
        Self {
            mode: config.mode,
            config: Arc::new(config),
            rules: Arc::new(rules),
            model,
            installer,
            active_job: 0,
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
        self.active_job += 1;

        // The rules pass is instant, so without saying that a second pass is
        // running the window looks finished the moment it appears. Unslop is
        // whole without a model; the other two are not, and say so.
        let rewriting = self.start_rewrite(baseline.clone(), proxy);
        let words = wording(self.mode);
        let phase = match (rewriting, self.mode) {
            (true, _) => (Phase::Working, words.working),
            (false, Mode::Unslop) => (Phase::Done, words.done),
            (false, _) => (Phase::Warn, words.no_model),
        };

        popup.show(&baseline, &self.original);
        popup.set_phase(phase.0, phase.1);
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
        self.active_job += 1;
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
        if job != self.active_job {
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
        self.installer.on_finished(outcome);
        // The backend was probed at startup, before any of this existed. A
        // runtime that has only just landed is no use until we look again.
        if self.model.is_none() {
            self.model = build_model(&self.config, self.installer.path());
        }
        popup.set_install(self.install_button(), self.note());
    }

    /// When the event loop should next wake to check for an idle server.
    fn idle_deadline(&self) -> Option<Instant> {
        self.model.as_ref()?.idle_deadline()
    }

    pub fn unload_if_idle(&mut self) {
        if let Some(model) = &mut self.model {
            model.unload_if_idle();
        }
    }

    /// Hand a rewrite to a worker thread. Returns whether one was started.
    ///
    /// The server is started on demand rather than at launch: holding 5.3GB of
    /// VRAM all day for a tool used a few times an hour is not a fair trade on
    /// a laptop.
    fn start_rewrite(&mut self, doc: Doc, proxy: &EventLoopProxy<Message>) -> bool {
        let port = match self.config.rewrite.provider {
            Provider::Off => return false,
            // A remote provider needs no weights and no server of our own.
            Provider::Remote => {
                if !self.config.remote.is_usable() {
                    return false;
                }
                None
            }
            Provider::Local => {
                if self.installer.model_path().is_none() {
                    return false;
                }
                let Some(model) = &mut self.model else {
                    return false;
                };
                match model.port() {
                    Ok(port) => Some(port),
                    Err(err) => {
                        eprintln!("could not start the model server: {err}");
                        return false;
                    }
                }
            }
        };

        let rules = Arc::clone(&self.rules);
        let config = Arc::clone(&self.config);
        let (job, mode, proxy) = (self.active_job, self.mode, proxy.clone());
        thread::spawn(move || {
            let ready = port.is_none_or(|port| model::wait_until_ready(port, READY_TIMEOUT));
            let result = if ready {
                rewrite::run(&rules, &doc, &config, port, mode)
                    .map_err(|rejected| rejected.to_string())
            } else {
                Err("the model server did not start".to_owned())
            };
            let _ = proxy.send_event(Message::RewriteFinished { job, result });
        });
        true
    }
}

/// The model server, when a local rewrite is both wanted and possible.
///
/// The backend is probed rather than assumed, which costs a few hundred
/// milliseconds and avoids running a CUDA build on an AMD laptop. Called at
/// startup and again after an install, since the answer changes when the
/// runtime arrives.
fn build_model(config: &Config, weights: &std::path::Path) -> Option<Model> {
    if config.rewrite.provider != Provider::Local {
        return None;
    }
    let idle = Duration::from_secs(config.local.idle_unload_mins * 60);
    let backend = model::choose_backend()?;
    Some(Model::new(backend, weights, idle, &config.local))
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
