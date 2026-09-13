//! The install button has to name a number before anything is fetched, so the
//! sizes are pinned in the source and formatted here.

use unslop::install::{pinned_size, sha256_matches, size};

#[test]
fn only_the_pinned_weights_are_held_to_a_size() {
    // Any other file is the user's own. Holding it to the pinned size called
    // it missing, which kept the model pass from running and offered to
    // replace it.
    assert_eq!(
        pinned_size("models/qwen3.5-4b-q6_k.gguf"),
        Some(3_525_956_768)
    );
    assert_eq!(pinned_size("models/qwen3-8b-q4_k_m.gguf"), None);
}

#[test]
fn a_download_is_checked_against_its_digest() {
    let path = std::env::temp_dir().join("unslop-sha256-check.txt");
    std::fs::write(&path, "abc").unwrap();
    let abc = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    assert!(sha256_matches(&path, abc).unwrap());
    assert!(!sha256_matches(&path, &"0".repeat(64)).unwrap());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn sizes_read_the_way_a_person_would_say_them() {
    // The three archives and the weights, as the button reports them.
    assert_eq!(size(31_666_541), "30MB");
    assert_eq!(size(254_078_211), "242MB");
    assert_eq!(size(254_078_211 + 391_443_627), "615MB");
    assert_eq!(size(3_525_956_768), "3.3GB");
    // Everything at once, which is what a genuinely first run offers.
    assert_eq!(size(3_525_956_768 + 254_078_211 + 391_443_627), "3.9GB");
}

#[test]
fn the_boundary_does_not_read_as_1024mb() {
    assert_eq!(size(1024 * 1024 * 1024 - 1), "1023MB");
    assert_eq!(size(1024 * 1024 * 1024), "1.0GB");
}
