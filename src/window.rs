//! The preview popup.
//!
//! The window must appear without taking keyboard focus, so that the user can
//! press Ctrl+V straight after the hotkey and have it land in the application
//! they were already working in. `with_focusable(false)`
//! gives us `WS_EX_NOACTIVATE`, which also means the window never takes
//! keyboard input until the user deliberately clicks it, so we lift the style
//! on click and restore it on hide.
//!
//! The system caption is off and the title bar lives in the page instead, so
//! dragging and maximising have to come back through the IPC channel.

use std::sync::atomic::{AtomicBool, Ordering};

use tao::{
    dpi::LogicalSize,
    event_loop::{EventLoopProxy, EventLoopWindowTarget},
    platform::windows::{WindowBuilderExtWindows, WindowExtWindows},
    window::{Icon, Window, WindowBuilder},
};
use windows::Win32::{
    Foundation::HWND,
    Graphics::Dwm::{DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute},
    UI::WindowsAndMessaging::{
        GWL_EXSTYLE, GetWindowLongPtrW, SetForegroundWindow, SetWindowLongPtrW, WS_EX_NOACTIVATE,
    },
};
use wry::{WebView, WebViewBuilder};

use crate::{Message, config::Mode, doc::Doc, icon, render};

/// What the app is doing, as the page's status pill shows it.
#[derive(Clone, Copy)]
pub enum Phase {
    Working,
    Done,
    Warn,
}

impl Phase {
    /// The class name the stylesheet keys off.
    fn class(self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::Done => "done",
            Self::Warn => "warn",
        }
    }
}

pub struct Popup {
    window: Window,
    webview: WebView,
}

impl Popup {
    pub fn new(
        target: &EventLoopWindowTarget<Message>,
        image: &icon::Image,
        proxy: EventLoopProxy<Message>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let window = WindowBuilder::new()
            .with_title("Unslop")
            .with_window_icon(Icon::from_rgba(image.rgba.clone(), image.width, image.height).ok())
            .with_inner_size(LogicalSize::new(680.0, 520.0))
            .with_min_inner_size(LogicalSize::new(420.0, 260.0))
            .with_visible(false)
            // The title bar is drawn by the page, so the system one goes.
            .with_decorations(false)
            .with_always_on_top(true)
            // Sets WS_EX_NOACTIVATE, so showing the window cannot steal focus.
            .with_focusable(false)
            .with_skip_taskbar(true)
            .build(target)?;

        let loaded = AtomicBool::new(false);
        let webview = WebViewBuilder::new()
            .with_html(include_str!("../ui/popup.html"))
            .with_ipc_handler(move |request| {
                let body = request.body().as_str();
                if let Some(mode) = body.strip_prefix("mode:") {
                    if let Some(mode) = Mode::parse(mode) {
                        let _ = proxy.send_event(Message::SetMode(mode));
                    }
                    return;
                }
                let message = match body {
                    "activate" => Message::Activate,
                    "install" => Message::InstallModel,
                    "copy" => Message::Copy,
                    "drag" => Message::Drag,
                    "maximize" => Message::Maximize,
                    "hide" => Message::Hide,
                    _ => return,
                };
                let _ = proxy.send_event(message);
            })
            // The page is loaded from memory and must never navigate anywhere,
            // least of all somewhere a pasted link points. The load of the page
            // itself is a navigation too, so exactly one is allowed: refusing
            // that one leaves the popup blank.
            .with_navigation_handler(move |_| !loaded.swap(true, Ordering::Relaxed))
            .build(&window)?;

        // An undecorated window keeps square corners unless it asks, which
        // next to every other Windows 11 window looks like a bug.
        let round = DWMWCP_ROUND;
        unsafe {
            let _ = DwmSetWindowAttribute(
                HWND(window.hwnd() as *mut _),
                DWMWA_WINDOW_CORNER_PREFERENCE,
                std::ptr::from_ref(&round).cast(),
                size_of_val(&round) as u32,
            );
        }

        Ok(Self { window, webview })
    }

    pub fn is_visible(&self) -> bool {
        self.window.is_visible()
    }

    /// Show the popup with `doc` rendered, leaving focus where it is.
    pub fn show(&self, doc: &Doc, original: &str) {
        self.update(doc, original);
        // A fresh clipboard deserves a fresh view, whatever the last press
        // was left showing.
        self.run("setDiff(false)");
        self.window.set_visible(true);
    }

    /// Replace the content without touching the window's visibility, for a
    /// rewrite that lands while the popup is already open.
    ///
    /// The diff is rendered here rather than on demand so that toggling the
    /// view costs no round trip, and so a rewrite landing while the diff is
    /// open updates what is on screen.
    pub fn update(&self, doc: &Doc, original: &str) {
        self.run(&format!(
            "setContent({}, {})",
            json(Some(&render::preview(doc))),
            json(Some(&render::diff(original, doc.text()))),
        ));
    }

    /// Say which pass is running, or that both have finished.
    ///
    /// Separate from the content because the rules pass is instant and the
    /// model is not: without this the window would look finished the moment
    /// it appeared, whatever was still happening behind it.
    pub fn set_phase(&self, phase: Phase, label: &str) {
        self.run(&format!(
            "setPhase({}, {})",
            json(Some(phase.class())),
            json(Some(label)),
        ));
    }

    /// Move the window from a press on the page's title bar.
    pub fn drag(&self) {
        if let Err(err) = self.window.drag_window() {
            eprintln!("could not drag the window: {err}");
        }
    }

    pub fn toggle_maximized(&self) {
        self.window.set_maximized(!self.window.is_maximized());
    }

    /// Point the dropdown at the mode in force: the one saved in the config
    /// when the page first comes up.
    pub fn set_mode(&self, mode: Mode) {
        self.run(&format!("setMode({})", json(Some(mode.as_str()))));
    }

    /// Refresh the installation button and note, which change while the
    /// window is open and independently of its content.
    pub fn set_install(&self, label: Option<String>, note: Option<String>) {
        self.run(&format!(
            "setInstall({}, {})",
            json(label.as_deref()),
            json(note.as_deref()),
        ));
    }

    pub fn hide(&self) {
        self.window.set_visible(false);
        // Restore the no-activate style so the next hotkey press is silent
        // again even if this showing was clicked into.
        self.set_no_activate(true);
    }

    /// Let the window take keyboard focus, in response to a deliberate click.
    ///
    /// `SetForegroundWindow` is refused while `WS_EX_NOACTIVATE` is set, so the
    /// style has to come off first.
    pub fn activate(&self) {
        self.set_no_activate(false);
        unsafe {
            let _ = SetForegroundWindow(self.hwnd());
        }
    }

    fn run(&self, script: &str) {
        if let Err(err) = self.webview.evaluate_script(script) {
            eprintln!("failed to update the preview: {err}");
        }
    }

    fn hwnd(&self) -> HWND {
        HWND(self.window.hwnd() as *mut _)
    }

    fn set_no_activate(&self, enabled: bool) {
        let hwnd = self.hwnd();
        unsafe {
            let current = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
            let updated = if enabled {
                current | WS_EX_NOACTIVATE.0
            } else {
                current & !WS_EX_NOACTIVATE.0
            };
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, updated as isize);
        }
    }
}

/// Encode for injection into the page, so no clipboard content can escape the
/// string it is being passed in.
fn json(value: Option<&str>) -> serde_json::Value {
    value.map_or(serde_json::Value::Null, serde_json::Value::from)
}
