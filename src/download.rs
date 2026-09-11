//! Fetching the model weights on first run.
//!
//! The weights are several gigabytes, so this resumes rather than restarting
//! after a dropped connection, and writes to a `.part` file that is only
//! renamed once the transfer is complete. A truncated file that looked like a
//! model would fail much later and much more confusingly.

use std::{
    fs::{self, File},
    io::{self, Read, Seek, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// How often progress is reported, so a 4.7GB transfer does not flood the
/// event loop with several thousand messages a second.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);

const CHUNK: usize = 128 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub downloaded: u64,
    /// Absent when the server does not report a length.
    pub total: Option<u64>,
}

impl Progress {
    pub fn percent(&self) -> Option<u8> {
        let total = self.total?;
        (total > 0).then(|| ((self.downloaded * 100) / total).min(100) as u8)
    }
}

/// Download `url` to `dest`, resuming any partial transfer already there.
///
/// `on_progress` is called at most a few times a second, and once at the end.
pub fn to_file(
    url: &str,
    dest: &Path,
    mut on_progress: impl FnMut(Progress),
) -> Result<(), Box<dyn std::error::Error>> {
    if dest.exists() {
        return Ok(());
    }

    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }

    let partial = part_path(dest);
    let resume_from = fs::metadata(&partial).map_or(0, |meta| meta.len());

    let mut request = ureq::get(url);
    if resume_from > 0 {
        request = request.header("Range", &format!("bytes={resume_from}-"));
    }
    let mut response = request.call()?;

    // A server that ignored the range header sends the whole file again, and
    // appending to the partial file would corrupt it.
    let resuming = response.status().as_u16() == 206;
    let offset = if resuming { resume_from } else { 0 };
    let total = content_length(&response).map(|len| len + offset);

    let mut file = File::options()
        .create(true)
        .write(true)
        .truncate(!resuming)
        .open(&partial)?;
    if resuming {
        file.seek(io::SeekFrom::End(0))?;
    }

    let mut reader = response.body_mut().as_reader();
    let mut buffer = vec![0u8; CHUNK];
    let mut downloaded = offset;
    let mut last_report = Instant::now();

    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        file.write_all(&buffer[..read])?;
        downloaded += read as u64;

        if last_report.elapsed() >= PROGRESS_INTERVAL {
            on_progress(Progress { downloaded, total });
            last_report = Instant::now();
        }
    }
    file.sync_all()?;
    drop(file);

    if let Some(total) = total
        && downloaded != total
    {
        // Leave the partial file in place so the next attempt resumes.
        return Err(format!("transfer ended early at {downloaded} of {total} bytes").into());
    }

    // Only now is the file safe to treat as a model.
    fs::rename(&partial, dest)?;
    on_progress(Progress {
        downloaded,
        total: total.or(Some(downloaded)),
    });
    Ok(())
}

fn part_path(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    dest.with_file_name(name)
}

fn content_length(response: &ureq::http::Response<ureq::Body>) -> Option<u64> {
    response
        .headers()
        .get("content-length")?
        .to_str()
        .ok()?
        .parse()
        .ok()
}
