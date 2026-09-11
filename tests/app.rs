//! What the popup says about the setup, which is the only sign the user gets
//! that a pass did not happen.

use unslop::{
    app::setup_note,
    config::{Config, Provider},
};

fn local() -> Config {
    Config::default()
}

#[test]
fn weights_without_a_backend_are_not_passed_over_in_silence() {
    // Observed with a release build whose folder had the weights but not the
    // runtime: the model pass never ran and nothing on screen said so.
    assert_eq!(
        setup_note(&local(), None, false),
        Some("no GPU backend found, rules only".to_owned())
    );
}

#[test]
fn a_working_local_setup_says_nothing() {
    assert_eq!(setup_note(&local(), None, true), None);
}

#[test]
fn a_missing_model_is_the_louder_problem() {
    // The installer's own note wins: telling someone their GPU is unusable is
    // no help when the thing that would use it has not been downloaded.
    let installing = Some("Downloading model, 12%".to_owned());
    assert_eq!(setup_note(&local(), installing.clone(), false), installing);
}

#[test]
fn the_privacy_warning_outranks_everything() {
    let mut config = Config::default();
    config.rewrite.provider = Provider::Remote;
    let note = setup_note(&config, Some("ignored".to_owned()), false).expect("a warning");
    assert!(note.contains(&config.remote.base_url), "got {note:?}");
}

#[test]
fn switching_the_model_off_is_not_a_fault() {
    // Nothing is missing when nothing was asked for.
    let mut config = Config::default();
    config.rewrite.provider = Provider::Off;
    assert_eq!(setup_note(&config, None, false), None);
}
