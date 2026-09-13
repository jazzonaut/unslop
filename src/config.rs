//! Settings, and the commented file they come from.
//!
//! The documented defaults live in `defaults/config.toml` and are written out
//! verbatim next to the executable on first run, so the file a user opens
//! explains itself and lives with the rest of the installation. Every
//! field also has a default here, which means an old or partial file still
//! loads rather than failing.

use std::{env, fs, path::PathBuf};

use serde::Deserialize;

use crate::paths;

/// Written out on first run and parsed for the built-in defaults, so the
/// comments and the code cannot drift apart.
const DEFAULT_FILE: &str = include_str!("../defaults/config.toml");

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub hotkey: String,
    pub launch_at_startup: bool,
    pub icon: String,
    pub mode: Mode,
    pub rewrite: Rewrite,
    pub local: Local,
    pub remote: Remote,
    pub rules: RulesFile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Local,
    Remote,
    Off,
}

/// What the model pass is asked to do with the text. The rules pass runs the
/// same way in every mode; this decides what the model does after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Unslop,
    Simplify,
    Tldr,
}

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::Unslop, Mode::Simplify, Mode::Tldr];

    /// The spelling used in the config file and by the page's dropdown.
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Unslop => "unslop",
            Mode::Simplify => "simplify",
            Mode::Tldr => "tldr",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.as_str() == text)
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rewrite {
    pub provider: Provider,
    pub temperature: f32,
    pub max_input_chars: usize,
    pub chunk_chars: usize,
    pub max_total_chars: usize,
    pub max_output_tokens: u32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Local {
    pub model: String,
    pub idle_unload_mins: u64,
    pub context: u32,
    pub gpu_layers: u32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Remote {
    pub base_url: String,
    pub model: String,
    pub api_key: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RulesFile {
    pub path: String,
}

impl Remote {
    /// The key to use, preferring the environment so it need not be written
    /// into a file that ends up in a backup.
    pub fn key(&self) -> Option<String> {
        if !self.api_key.is_empty() {
            return Some(self.api_key.clone());
        }
        env::var("OPENROUTER_API_KEY")
            .ok()
            .filter(|key| !key.is_empty())
    }

    /// Whether this is configured enough to be worth attempting.
    pub fn is_usable(&self) -> bool {
        !self.base_url.is_empty() && !self.model.is_empty() && self.key().is_some()
    }
}

impl Config {
    /// Load the user's configuration, writing the documented default if there
    /// is not one yet.
    ///
    /// The user's file is overlaid on the bundled defaults key by key, so a
    /// file containing only the lines someone changed still works, and so does
    /// one written before a new setting existed.
    ///
    /// A broken file is reported and ignored rather than being fatal. This is
    /// a resident application and a stray character should not stop it
    /// starting.
    pub fn load() -> Self {
        let path = Self::path();
        let user = match fs::read_to_string(&path) {
            Ok(text) => match toml::from_str::<toml::Value>(&text) {
                Ok(value) => Some(value),
                Err(err) => {
                    eprintln!(
                        "{} is not valid TOML, using defaults: {err}",
                        path.display()
                    );
                    None
                }
            },
            Err(_) => {
                // Written out with its comments intact, so the file explains
                // itself the first time someone opens it. A folder we cannot
                // write to is worth saying out loud: otherwise the file never
                // appears and there is nothing to edit.
                if let Err(err) = fs::write(&path, DEFAULT_FILE) {
                    eprintln!("could not write {}: {err}", path.display());
                }
                None
            }
        };

        Self::from_toml(user).unwrap_or_else(|err| {
            eprintln!(
                "{} has a bad setting, using defaults: {err}",
                path.display()
            );
            Self::default()
        })
    }

    /// Parse a configuration exactly as `load` would, for tests.
    pub fn from_str_for_test(text: &str) -> Result<Self, toml::de::Error> {
        Self::from_toml(Some(toml::from_str::<toml::Value>(text)?))
    }

    /// Merge `user` over the bundled defaults and interpret the result.
    fn from_toml(user: Option<toml::Value>) -> Result<Self, toml::de::Error> {
        let defaults = toml::from_str::<toml::Value>(DEFAULT_FILE)
            .expect("the bundled default config must be valid TOML");
        let merged = match user {
            Some(user) => overlay(defaults, user),
            None => defaults,
        };
        merged.try_into()
    }

    /// Beside the executable, so the whole installation is one folder that can
    /// be copied, backed up or carried on a stick with its settings intact.
    pub fn path() -> PathBuf {
        paths::beside_exe("config.toml")
    }

    /// Persist `launch_at_startup`, for the tray toggle.
    ///
    /// Only that one line is rewritten. The file is documentation as much as
    /// configuration, so serialising the whole struct back over it would cost
    /// the user every comment and every setting they had left at its default.
    pub fn remember_launch_at_startup(enabled: bool) {
        Self::remember(|text| with_launch_at_startup(text, enabled));
    }

    /// Persist the dropdown's choice, so the tool opens the way it was left.
    pub fn remember_mode(mode: Mode) {
        Self::remember(|text| with_setting(text, "mode", &format!("{:?}", mode.as_str())));
    }

    fn remember(edit: impl Fn(&str) -> String) {
        let path = Self::path();
        let text = fs::read_to_string(&path).unwrap_or_else(|_| DEFAULT_FILE.to_owned());
        if let Err(err) = fs::write(&path, edit(&text)) {
            eprintln!("could not save the setting to {}: {err}", path.display());
        }
    }
}

impl Default for Config {
    /// The bundled file is the single source of truth, so the comments in it
    /// describe what the code actually does.
    fn default() -> Self {
        Self::from_toml(None).expect("the bundled default config must be complete")
    }
}

/// Overlay `over` onto `base`, descending into tables so that setting one key
/// does not discard its neighbours.
fn overlay(base: toml::Value, over: toml::Value) -> toml::Value {
    let (base, over) = match (base, over) {
        (toml::Value::Table(base), toml::Value::Table(over)) => (base, over),
        // A scalar, or a mismatch in shape: the user's value wins outright.
        (_, over) => return over,
    };
    let mut base = base;
    for (key, value) in over {
        let merged = match base.remove(&key) {
            Some(existing) => overlay(existing, value),
            None => value,
        };
        base.insert(key, merged);
    }
    toml::Value::Table(base)
}

pub fn with_launch_at_startup(text: &str, enabled: bool) -> String {
    with_setting(text, "launch_at_startup", &enabled.to_string())
}

/// Replace the top-level `key = ...` line in `text`, or add it if the file
/// predates the setting. `value` is already TOML.
///
/// A missing key has to go in before the first `[section]` header: after one it
/// would be read as a key of that table rather than a top-level setting.
pub fn with_setting(text: &str, key: &str, value: &str) -> String {
    let setting = format!("{key} = {value}");
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let is_key = |line: &String| {
        let rest = line.trim_start().strip_prefix(key);
        rest.is_some_and(|rest| rest.trim_start().starts_with('='))
    };
    match lines.iter().position(is_key) {
        Some(at) => lines[at] = setting,
        None => {
            let at = lines
                .iter()
                .position(|line| line.trim_start().starts_with('['))
                .unwrap_or(lines.len());
            lines.insert(at, setting);
        }
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}
