//! The weights are several gigabytes, so resuming and atomic completion are
//! the parts that matter: a truncated file mistaken for a model fails much
//! later and much more confusingly than a failed download.

use std::fs;

use unslop::download::{self, Progress};

#[test]
fn percent_is_reported_only_when_the_size_is_known() {
    assert_eq!(
        Progress {
            downloaded: 50,
            total: Some(200)
        }
        .percent(),
        Some(25)
    );
    assert_eq!(
        Progress {
            downloaded: 0,
            total: Some(0)
        }
        .percent(),
        None
    );
    assert_eq!(
        Progress {
            downloaded: 10,
            total: None
        }
        .percent(),
        None
    );
    // A server reporting less than it sends must not produce nonsense.
    assert_eq!(
        Progress {
            downloaded: 300,
            total: Some(200)
        }
        .percent(),
        Some(100)
    );
}

#[test]
fn an_existing_file_is_not_downloaded_again() {
    let dir = std::env::temp_dir().join("unslop-test-existing");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let dest = dir.join("already-there.bin");
    fs::write(&dest, b"untouched").unwrap();

    // An unreachable URL proves no request was attempted.
    download::to_file("https://0.0.0.0/nope", &dest, |_| {}).expect("should skip");
    assert_eq!(fs::read(&dest).unwrap(), b"untouched");
    let _ = fs::remove_dir_all(&dir);
}

/// Exercises the resume path against a real server that honours Range.
#[test]
#[ignore = "needs the network"]
fn a_partial_transfer_resumes_to_the_same_bytes() {
    const URL: &str = "https://huggingface.co/Qwen/Qwen3-8B-GGUF/resolve/main/README.md";
    let dir = std::env::temp_dir().join("unslop-test-resume");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();

    let whole = dir.join("whole.md");
    download::to_file(URL, &whole, |_| {}).expect("first download");
    let expected = fs::read(&whole).unwrap();
    assert!(
        expected.len() > 64,
        "test file is too small to truncate meaningfully"
    );

    // Leave a half-finished transfer behind, exactly as a dropped connection would.
    let resumed = dir.join("resumed.md");
    fs::write(dir.join("resumed.md.part"), &expected[..expected.len() / 2]).unwrap();

    let mut seen_progress = false;
    download::to_file(URL, &resumed, |_| seen_progress = true).expect("resumed download");

    assert_eq!(
        fs::read(&resumed).unwrap(),
        expected,
        "resumed file differs"
    );
    assert!(
        !dir.join("resumed.md.part").exists(),
        "partial file was left behind"
    );
    assert!(seen_progress, "completion was never reported");
    let _ = fs::remove_dir_all(&dir);
}
