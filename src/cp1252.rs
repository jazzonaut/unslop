//! Decoding the Windows ANSI code page.
//!
//! CF_HTML is specified as UTF-8, but legacy writers emit Windows-1252
//! instead. Replacing those bytes with U+FFFD is not an option here: the
//! 0x80-0x9F range holds precisely the em dash, en dash, curly quotes and
//! ellipsis this tool exists to find, so a lossy read would destroy the very
//! characters it is meant to fix, and turn a client's "£4,500" into rubble.

/// The 0x80-0x9F range, which is where Windows-1252 departs from Latin-1.
/// The five unassigned positions become replacement characters.
const HIGH: [char; 32] = [
    '\u{20ac}', '\u{fffd}', '\u{201a}', '\u{0192}', '\u{201e}', '\u{2026}', '\u{2020}', '\u{2021}',
    '\u{02c6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{fffd}', '\u{017d}', '\u{fffd}',
    '\u{fffd}', '\u{2018}', '\u{2019}', '\u{201c}', '\u{201d}', '\u{2022}', '\u{2013}', '\u{2014}',
    '\u{02dc}', '\u{2122}', '\u{0161}', '\u{203a}', '\u{0153}', '\u{fffd}', '\u{017e}', '\u{0178}',
];

/// Decode bytes that are not valid UTF-8, assuming the Windows code page.
///
/// ponytail: assumes Windows-1252, the default on Western installs. A Cyrillic
/// or Greek ANSI code page would still mis-map; wire in a full decoder only if
/// that turns up in practice.
pub fn decode(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&byte| match byte {
            // Every other byte maps to the code point of the same value.
            0x80..=0x9f => HIGH[(byte - 0x80) as usize],
            _ => byte as char,
        })
        .collect()
}
