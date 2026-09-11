//! The notification-area icon.
//!
//! A global hotkey is invisible: without something in the tray there is no
//! sign the application is running, and no way to stop it once the popup is
//! closed. The menu deliberately offers only what the hotkey and the popup
//! already do.

use std::sync::atomic::{AtomicBool, Ordering};

use tao::event_loop::EventLoopProxy;
use tray_icon::{
    TrayIcon, TrayIconBuilder,
    menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem},
};

use crate::{Message, icon};

/// Held for as long as the application runs: dropping it removes the icon.
pub struct Tray {
    _tray: TrayIcon,
}

impl Tray {
    pub fn new(
        image: &icon::Image,
        hotkey: &str,
        at_startup: bool,
        proxy: EventLoopProxy<Message>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let open = MenuItem::new(format!("Open ({hotkey})"), true, None);
        let startup = CheckMenuItem::new("Start with Windows", true, at_startup, None);
        // The item ticks itself on click, but it is `Rc` inside and so cannot
        // be read from the handler, which has to be `Send`. Mirroring the tick
        // costs one flag and keeps the two in step: nothing else moves it.
        let checked = AtomicBool::new(at_startup);
        let exit = MenuItem::new("Exit", true, None);
        let (open_id, startup_id, exit_id) =
            (open.id().clone(), startup.id().clone(), exit.id().clone());

        let menu = Menu::new();
        menu.append_items(&[
            &open,
            &PredefinedMenuItem::separator(),
            &startup,
            &PredefinedMenuItem::separator(),
            &exit,
        ])?;

        let tray = TrayIconBuilder::new()
            .with_tooltip("Unslop")
            .with_icon(tray_icon::Icon::from_rgba(
                image.rgba.clone(),
                image.width,
                image.height,
            )?)
            .with_menu(Box::new(menu))
            .build()?;

        // The menu runs on its own message loop, so as with the hotkey the
        // only safe thing to do from here is post an event.
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            let message = if event.id == open_id {
                Message::Hotkey
            } else if event.id == startup_id {
                Message::LaunchAtStartup(!checked.fetch_xor(true, Ordering::Relaxed))
            } else if event.id == exit_id {
                Message::Exit
            } else {
                return;
            };
            let _ = proxy.send_event(message);
        }));

        Ok(Self { _tray: tray })
    }
}
