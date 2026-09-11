//! Starting with Windows.
//!
//! The `Run` key is the lightest way to do this: no scheduled task, no
//! installer, and the user can see and remove the entry themselves. The key is
//! reconciled with the configuration file on every launch, and the tray toggle
//! writes both so the two can never disagree.

use windows_registry::CURRENT_USER;

use crate::config::Config;

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const NAME: &str = "Unslop";

/// Make the registry agree with `enabled`. Failure is reported and ignored:
/// the application still works, it just will not start itself next time.
pub fn apply(enabled: bool) {
    if let Err(err) = reconcile(enabled) {
        eprintln!("could not update the launch-at-startup setting: {err}");
    }
}

/// Turn the setting on or off at the user's request.
///
/// The registry is only half of it: `apply` reconciles against the
/// configuration file on every launch, so a toggle that did not write the file
/// back would be undone the next time the application started.
pub fn set(enabled: bool) {
    apply(enabled);
    Config::remember_launch_at_startup(enabled);
}

fn reconcile(enabled: bool) -> Result<(), Box<dyn std::error::Error>> {
    let key = CURRENT_USER.create(RUN_KEY)?;
    if !enabled {
        // Not being there is the desired state, so a value that was never
        // written is a success and not something to report.
        let _ = key.remove_value(NAME);
        return Ok(());
    }
    // Quoted, because the install path will contain spaces sooner or later.
    let exe = std::env::current_exe()?;
    key.set_string(NAME, format!("\"{}\"", exe.display()))?;
    Ok(())
}
