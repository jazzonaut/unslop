//! Finding the files we ship.

use std::{env, path::PathBuf};

/// Resolve a path next to the executable, falling back to the working
/// directory so that `cargo run` finds the same files as an installed copy.
pub fn beside_exe(relative: &str) -> PathBuf {
    let installed = env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(relative)));

    match installed {
        Some(path) if path.exists() => path,
        Some(path) => {
            let local = PathBuf::from(relative);
            if local.exists() { local } else { path }
        }
        None => PathBuf::from(relative),
    }
}
