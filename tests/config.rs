//! The configuration file is the user's interface to everything the tool does,
//! so a partial or outdated one has to keep working.

use unslop::config::{Config, Mode, Provider, with_launch_at_startup, with_setting};

#[test]
fn the_bundled_default_is_complete_and_valid() {
    // Every other default is derived from this file, so if it drifts out of
    // step with the structs nothing else works.
    let config = Config::default();
    assert_eq!(config.rewrite.provider, Provider::Local);
    assert!(config.hotkey.contains("KeyU"));
    assert!(config.local.model.ends_with(".gguf"));
    assert!(config.rewrite.max_input_chars > 1000);
    // Nothing should start itself or replace its own icon without being asked.
    assert!(!config.launch_at_startup);
    assert!(config.icon.is_empty());
    assert_eq!(config.mode, Mode::Unslop);
}

#[test]
fn the_dropdown_is_remembered_the_same_way_as_the_tray_toggle() {
    // One line, in place, comments intact, and a file from before the
    // dropdown existed gains the key above the first section.
    let file = include_str!("../defaults/config.toml");
    let saved = with_setting(file, "mode", "\"tldr\"");
    assert_eq!(Config::from_str_for_test(&saved).unwrap().mode, Mode::Tldr);
    assert_eq!(
        saved
            .lines()
            .filter(|l| l.trim_start().starts_with('#'))
            .count(),
        file.lines()
            .filter(|l| l.trim_start().starts_with('#'))
            .count(),
    );

    let old = "hotkey = \"CTRL+ALT+KeyU\"\n\n[local]\nidle_unload_mins = 2\n";
    let config = Config::from_str_for_test(&with_setting(old, "mode", "\"simplify\"")).unwrap();
    assert_eq!(config.mode, Mode::Simplify);
    assert_eq!(config.local.idle_unload_mins, 2);

    // A key that merely starts with the name is someone else's line.
    let other = "model_x = 1\n";
    assert!(with_setting(other, "mode", "\"tldr\"").contains("model_x = 1"));
    assert!(with_setting(other, "mode", "\"tldr\"").contains("mode = \"tldr\""));
}

#[test]
fn the_shipped_file_still_carries_its_comments() {
    // It is written out verbatim on first run; without the comments it would
    // be a list of magic numbers.
    let file = include_str!("../defaults/config.toml");
    assert!(
        file.lines()
            .filter(|l| l.trim_start().starts_with('#'))
            .count()
            > 30
    );
    assert!(
        file.contains("WARNING"),
        "the remote section must warn about privacy"
    );
}

#[test]
fn remote_needs_a_key_a_model_and_a_url() {
    let mut remote = Config::default().remote;
    // The shipped default deliberately leaves the model blank.
    assert!(
        !remote.is_usable(),
        "remote looked usable with no model set"
    );
    remote.model = "some/model".into();
    remote.api_key = "sk-test".into();
    assert!(remote.is_usable());
    remote.base_url.clear();
    assert!(!remote.is_usable());
}

#[test]
fn an_api_key_can_stay_out_of_the_file() {
    let mut remote = Config::default().remote;
    remote.api_key.clear();
    // Reading the environment is what keeps the key out of backups.
    unsafe { std::env::set_var("OPENROUTER_API_KEY", "sk-from-env") };
    assert_eq!(remote.key().as_deref(), Some("sk-from-env"));
    unsafe { std::env::remove_var("OPENROUTER_API_KEY") };
    assert_eq!(remote.key(), None);
}

#[test]
fn a_file_with_one_changed_line_keeps_every_other_default() {
    // The most common way to edit a config is to change one line, so a partial
    // file must not wipe out the settings it does not mention.
    let partial = "[local]\nidle_unload_mins = 2\n";
    let config = Config::from_str_for_test(partial).expect("a partial file must load");
    assert_eq!(config.local.idle_unload_mins, 2);
    assert_eq!(config.local.context, Config::default().local.context);
    assert_eq!(config.hotkey, Config::default().hotkey);
}

#[test]
fn an_unknown_setting_is_reported_rather_than_ignored() {
    // Silently ignoring a typo means the user thinks they changed something.
    let typo = "[local]\nidle_unload_minutes = 2\n";
    assert!(Config::from_str_for_test(typo).is_err());
}

#[test]
fn the_tray_toggle_rewrites_one_line_and_nothing_else() {
    // The toggle writes the file the user reads, so it has to leave the
    // comments and every untouched setting exactly where they were.
    let file = include_str!("../defaults/config.toml");
    let on = with_launch_at_startup(file, true);
    assert!(on.contains("launch_at_startup = true"));
    assert!(!on.contains("launch_at_startup = false"));
    assert_eq!(
        file.lines()
            .filter(|l| l.trim_start().starts_with('#'))
            .count(),
        on.lines()
            .filter(|l| l.trim_start().starts_with('#'))
            .count(),
        "the toggle lost comments"
    );
    // Everything else must survive the round trip untouched.
    let before = Config::from_str_for_test(file).expect("the shipped file must load");
    let after = Config::from_str_for_test(&on).expect("the rewritten file must load");
    assert!(after.launch_at_startup);
    assert_eq!(after.hotkey, before.hotkey);
    assert_eq!(after.local.context, before.local.context);
    assert_eq!(after.remote.base_url, before.remote.base_url);

    // And back off again, landing on the file we started from.
    let off = with_launch_at_startup(&on, false);
    assert!(!Config::from_str_for_test(&off).unwrap().launch_at_startup);
}

#[test]
fn a_file_predating_the_setting_gains_it_as_a_top_level_key() {
    // Appending at the end would bury the key inside the last table, where it
    // would be read as `[local].launch_at_startup` and silently do nothing.
    let old = "hotkey = \"CTRL+ALT+KeyU\"\n\n[local]\nidle_unload_mins = 2\n";
    let updated = with_launch_at_startup(old, true);
    let config = Config::from_str_for_test(&updated).expect("the rewritten file must load");
    assert!(config.launch_at_startup);
    assert_eq!(config.local.idle_unload_mins, 2);
}
