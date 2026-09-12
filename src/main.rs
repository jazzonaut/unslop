// No console window: this is a resident background application.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::str::FromStr;

use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState, hotkey::HotKey};
use tao::{
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder},
};
use unslop::{
    Message, app::App, config::Config, icon, install::Installer, rules::Rules, startup, tray::Tray,
    window::Popup,
};

/// Used when the configured hotkey cannot be parsed, so a typo leaves the
/// application usable rather than unreachable.
const FALLBACK_HOTKEY: &str = "CTRL+ALT+KeyU";

/// Embedded so the binary stands alone. `rules.path` in the configuration
/// points at a copy on disk for anyone who wants to add their own tells.
const RULE_PACK: &str = include_str!("../rules/slop-rules.json");

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::load();
    let rules = load_rules(&config);
    let hotkey = hotkey(&config);
    startup::apply(config.launch_at_startup);
    let image = icon::load(&config.icon);
    let hotkey_label = config.hotkey.clone();
    let at_startup = config.launch_at_startup;
    let installer = Installer::new(&config.local.model);
    let mut app = App::new(config, rules, installer);

    let event_loop = EventLoopBuilder::<Message>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    let popup = Popup::new(&event_loop, &image, proxy.clone())?;
    // Held for the life of the process: dropping it takes the icon away.
    let _tray = Tray::new(&image, &hotkey_label, at_startup, proxy.clone())?;

    // The manager must outlive the loop or the hotkey is unregistered.
    let manager = GlobalHotKeyManager::new().expect("register global hotkey");
    manager.register(hotkey).expect("register global hotkey");

    let hotkey_proxy = proxy.clone();
    GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
        // Both press and release arrive; acting on both would toggle twice.
        if event.state == HotKeyState::Pressed {
            let _ = hotkey_proxy.send_event(Message::Hotkey);
        }
    }));

    event_loop.run(move |event, _target, control_flow| {
        // Wake for the idle check when a model server is running, so its VRAM
        // is handed back without a timer thread, and for the clipboard poll
        // while the popup is open.
        *control_flow = match app.wake_deadline(popup.is_visible()) {
            Some(deadline) => ControlFlow::WaitUntil(deadline),
            None => ControlFlow::Wait,
        };

        match event {
            Event::NewEvents(_) => {
                app.unload_if_idle();
                app.poll_clipboard(&popup, &proxy);
            }
            Event::UserEvent(Message::Hotkey) => {
                // Pressing again dismisses, since a window that never takes
                // focus cannot be closed with Escape until it is clicked.
                if popup.is_visible() {
                    popup.hide();
                } else {
                    app.on_hotkey(&popup, &proxy);
                }
            }
            Event::UserEvent(Message::RewriteFinished { job, result }) => {
                app.on_rewrite(&popup, job, result);
            }
            Event::UserEvent(Message::SetMode(mode)) => app.on_set_mode(&popup, &proxy, mode),
            Event::UserEvent(Message::Retry) => app.on_retry(&popup, &proxy),
            Event::UserEvent(Message::Copy) => app.on_copy(&popup),
            Event::UserEvent(Message::LaunchAtStartup(on)) => startup::set(on),
            Event::UserEvent(Message::InstallModel) => app.install(&popup, proxy.clone()),
            Event::UserEvent(Message::DownloadProgress(progress)) => {
                app.on_download_progress(&popup, progress);
            }
            Event::UserEvent(Message::DownloadFinished(outcome)) => {
                app.on_download_finished(&popup, outcome);
            }
            Event::UserEvent(Message::Activate) => popup.activate(),
            Event::UserEvent(Message::Drag) => popup.drag(),
            Event::UserEvent(Message::Maximize) => popup.toggle_maximized(),
            Event::UserEvent(Message::Hide)
            | Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => popup.hide(),
            Event::UserEvent(Message::Exit) => *control_flow = ControlFlow::Exit,
            _ => {}
        }
    })
}

/// The user's rule pack if they have one, otherwise the embedded copy.
///
/// A pack that will not parse is reported and ignored: it is an editable file
/// and a stray comma should not stop the application starting.
fn load_rules(config: &Config) -> Rules {
    if !config.rules.path.is_empty() {
        match std::fs::read_to_string(&config.rules.path).map(|pack| Rules::load(&pack)) {
            Ok(Ok(rules)) => return rules,
            Ok(Err(err)) => eprintln!("{} is not a valid rule pack: {err}", config.rules.path),
            Err(err) => eprintln!("cannot read {}: {err}", config.rules.path),
        }
    }
    // A compile-time constant, and the test suite proves it parses.
    Rules::load(RULE_PACK).expect("the embedded rule pack must parse")
}

fn hotkey(config: &Config) -> HotKey {
    HotKey::from_str(&config.hotkey).unwrap_or_else(|err| {
        eprintln!(
            "hotkey {:?} is not valid, using {FALLBACK_HOTKEY}: {err}",
            config.hotkey
        );
        HotKey::from_str(FALLBACK_HOTKEY).expect("the fallback hotkey must parse")
    })
}
