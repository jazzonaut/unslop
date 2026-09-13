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

/// How long one request may spend on the body before it is cut and resumed.
///
/// ureq has no idle timeout, only a bound on the whole body, so a stalled
/// connection would otherwise sit there for good with the popup saying
/// "Downloading" and no way to retry. Cutting the body every few minutes and
/// picking up with a Range request costs a TLS handshake per segment, which
/// both hosts handle, and turns a stall into an error within this long.
const SEGMENT: Duration = Duration::from_secs(180);

/// For the connection and the response headers, which are quick or dead.
const HANDSHAKE: Duration = Duration::from_secs(30);

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
    loop {
        let before = len_of(&partial);
        let mut resumed = false;
        match segment(url, &partial, before, &mut resumed, &mut on_progress) {
            Ok(Progress { downloaded, total }) if total.is_none_or(|total| total == downloaded) => {
                // Only now is the file safe to treat as a model.
                fs::rename(&partial, dest)?;
                on_progress(Progress {
                    downloaded,
                    total: total.or(Some(downloaded)),
                });
                return Ok(());
            }
            // A segment that ended early after real progress is a slow or
            // flapping link, not a dead one: pick up where it stopped. Unless
            // the server ignored the Range header, in which case every attempt
            // starts over and trying again would never finish.
            _ if len_of(&partial) > before && (before == 0 || resumed) => continue,
            Ok(Progress { downloaded, total }) => {
                // Leave the partial file in place so the next attempt resumes.
                return Err(format!(
                    "transfer ended early at {downloaded} of {} bytes",
                    total.unwrap_or_default()
                )
                .into());
            }
            Err(err) => return Err(err),
        }
    }
}

/// One request's worth of the file, appended to `partial`. `resumed` reports
/// whether the server honoured the Range header, which the caller needs even
/// when the read then fails.
fn segment(
    url: &str,
    partial: &Path,
    resume_from: u64,
    resumed: &mut bool,
    on_progress: &mut impl FnMut(Progress),
) -> Result<Progress, Box<dyn std::error::Error>> {
    let mut request = ureq::get(url)
        .config()
        .timeout_connect(Some(HANDSHAKE))
        .timeout_recv_response(Some(HANDSHAKE))
        .timeout_recv_body(Some(SEGMENT))
        .build();
    if resume_from > 0 {
        request = request.header("Range", &format!("bytes={resume_from}-"));
    }
    let mut response = request.call()?;

    // A server that ignored the range header sends the whole file again, and
    // appending to the partial file would corrupt it.
    *resumed = response.status().as_u16() == 206;
    let offset = if *resumed { resume_from } else { 0 };
    let total = content_length(&response).map(|len| len + offset);

    let mut file = File::options()
        .create(true)
        .write(true)
        .truncate(!*resumed)
        .open(partial)?;
    if *resumed {
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
    Ok(Progress { downloaded, total })
}

fn len_of(path: &Path) -> u64 {
    fs::metadata(path).map_or(0, |meta| meta.len())
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
