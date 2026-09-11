//! The CF_HTML clipboard format.
//!
//! CF_HTML prefixes markup with a header of byte offsets into its own payload,
//! which is easy to get subtly wrong and is why this module is pure and tested
//! in isolation.

use std::{fmt::Write, ops::Range};

use crate::cp1252;

const VERSION: &str = "Version:0.9\r\n";
const BODY_HEADER: &str = "<html>\r\n<body>\r\n<!--StartFragment-->";
const BODY_FOOTER: &str = "<!--EndFragment-->\r\n</body>\r\n</html>";
const OPEN_MARKER: &[u8] = b"<!--StartFragment-->";
const CLOSE_MARKER: &[u8] = b"<!--EndFragment-->";

/// Offsets are zero-padded to a fixed width so the header length, and every
/// offset within it, does not depend on the values themselves.
const LEN_WIDTH: usize = 10;

const HEADER_SIZE: usize = VERSION.len()
    + "StartHTML:".len()
    + LEN_WIDTH
    + 2
    + "EndHTML:".len()
    + LEN_WIDTH
    + 2
    + "StartFragment:".len()
    + LEN_WIDTH
    + 2
    + "EndFragment:".len()
    + LEN_WIDTH
    + 2;

/// Wrap a fragment in a CF_HTML payload with consistent byte offsets.
pub fn build(fragment: &str) -> Vec<u8> {
    let start_fragment = HEADER_SIZE + BODY_HEADER.len();
    let end_fragment = start_fragment + fragment.len();
    let end_html = end_fragment + BODY_FOOTER.len();

    let mut out = String::with_capacity(end_html);
    out.push_str(VERSION);
    let _ = write!(out, "StartHTML:{HEADER_SIZE:0LEN_WIDTH$}\r\n");
    let _ = write!(out, "EndHTML:{end_html:0LEN_WIDTH$}\r\n");
    let _ = write!(out, "StartFragment:{start_fragment:0LEN_WIDTH$}\r\n");
    let _ = write!(out, "EndFragment:{end_fragment:0LEN_WIDTH$}\r\n");
    debug_assert_eq!(out.len(), HEADER_SIZE, "header width must not vary");
    out.push_str(BODY_HEADER);
    out.push_str(fragment);
    out.push_str(BODY_FOOTER);

    debug_assert_eq!(out.len(), end_html);
    out.into_bytes()
}

/// Extract the fragment from a CF_HTML payload written by any application.
///
/// Clipboard HTML is untrusted: offsets arrive out of bounds, reversed, zeroed
/// and mid-character, and the payload is not always the UTF-8 the format
/// requires. Nothing here panics or reads past the buffer.
pub fn parse(raw: &[u8]) -> Option<String> {
    // Producers append the NUL terminator to the data. Trimming from the end
    // cannot disturb offsets, which are all measured from the start.
    let raw = trim_end_nuls(raw);

    // The fragment comments describe themselves, so they survive the offset
    // arithmetic that producers routinely get wrong. Prefer them.
    let markers = marker_range(raw);
    let offsets = offset_range(raw);

    // A range that decodes as clean UTF-8 is certainly the intended one, so try
    // every candidate strictly before settling for a lossy read of any of them.
    if let Some(fragment) = [&markers, &offsets]
        .into_iter()
        .flatten()
        .find_map(|range| decode(raw, range.clone()))
    {
        return Some(fragment);
    }

    // Some writers emit CF_HTML in the ANSI code page despite the format
    // requiring UTF-8. The marker boundaries are still trustworthy, so recover
    // the fragment and decode it as Windows-1252 rather than discarding the
    // bytes: that range is where the em dashes and curly quotes live.
    if let Some(range) = markers {
        return Some(decode_relaxed(&raw[range]));
    }

    // Malformed beyond repair. Degrade to the markup rather than losing the
    // rich text entirely. Bad offsets are not trusted even for a lossy read.
    let start = find(raw, b"<")?;
    Some(decode_relaxed(&raw[start..]))
}

/// The bytes between the fragment comments, if both are present and ordered.
fn marker_range(raw: &[u8]) -> Option<Range<usize>> {
    let start = find(raw, OPEN_MARKER)? + OPEN_MARKER.len();
    let end = find(raw, CLOSE_MARKER)?;
    (start <= end).then_some(start..end)
}

/// The range named by the header, if it points past the header at all.
fn offset_range(raw: &[u8]) -> Option<Range<usize>> {
    let (start, end) = header_offsets(raw);
    let (start, end) = (start?, end?);
    (start >= VERSION.len() && start <= end).then_some(start..end)
}

/// Decode as UTF-8, falling back to the Windows code page for bytes that are
/// not valid UTF-8. Used only once the strict passes have been exhausted.
fn decode_relaxed(bytes: &[u8]) -> String {
    str::from_utf8(bytes).map_or_else(|_| cp1252::decode(bytes), str::to_owned)
}

/// `None` unless the range is in bounds and valid UTF-8.
fn decode(raw: &[u8], range: Range<usize>) -> Option<String> {
    Some(str::from_utf8(raw.get(range)?).ok()?.to_owned())
}

fn trim_end_nuls(raw: &[u8]) -> &[u8] {
    let end = raw.iter().rposition(|&b| b != 0).map_or(0, |i| i + 1);
    &raw[..end]
}

/// Read `StartFragment`/`EndFragment` from the ASCII header, stopping at markup.
fn header_offsets(raw: &[u8]) -> (Option<usize>, Option<usize>) {
    let (mut start, mut end) = (None, None);

    for line in raw.split(|&b| b == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.first() == Some(&b'<') {
            break;
        }
        let Some(colon) = line.iter().position(|&b| b == b':') else {
            continue;
        };
        let value = str::from_utf8(&line[colon + 1..])
            .ok()
            .and_then(|v| v.trim().parse().ok());
        match &line[..colon] {
            b"StartFragment" => start = value,
            b"EndFragment" => end = value,
            _ => {}
        }
    }

    (start, end)
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}
