//! Windows clipboard reads and writes.
//!
//! We do not use `clipboard_win::formats::Html` for reading: its getter checks
//! `size > data_size` but not `start + size > data_size`, so malformed offsets
//! read out of bounds, and its `Getter<String>` impl copies unvalidated bytes
//! into a `String`. Clipboard HTML is untrusted input, so we parse it ourselves.

use core::num::NonZeroU32;

use clipboard_win::{Clipboard, Getter, SysResult, formats, options, raw};

use crate::{cf_html, doc::Doc, markdown};

/// Read the clipboard, preferring CF_HTML and falling back to CF_UNICODETEXT.
///
/// `Ok(None)` means there was nothing usable there, which is not an error.
pub fn read() -> SysResult<Option<Doc>> {
    let _clip = Clipboard::new_attempts(10)?;

    let mut text = String::new();
    let has_text = formats::Unicode.read_clipboard(&mut text).is_ok();

    let mut raw = Vec::new();
    if let Some(format) = html_format()
        && formats::RawData(format.get())
            .read_clipboard(&mut raw)
            .is_ok()
        && let Some(html) = cf_html::parse(&raw)
    {
        // Every application that offers CF_HTML also offers plain text, but if
        // one ever does not, an empty companion would be written back and
        // pasting into a terminal would produce nothing at all.
        if !has_text {
            text = markdown::from_html(&html).unwrap_or_default();
        }
        return Ok(Some(Doc::Rich { html, text }));
    }

    Ok(has_text.then_some(Doc::Plain(text)))
}

/// Write a document back, setting every format in a single clipboard open.
///
/// The `NoClear` variants are essential: the default setters empty the
/// clipboard first, so the second format written would wipe the first.
pub fn write(doc: &Doc) -> SysResult<()> {
    let _clip = Clipboard::new_attempts(10)?;
    raw::empty()?;

    // Plain text goes down for both kinds, so terminals and Notepad get
    // something sensible even when the source was rich.
    raw::set_string_with(doc.text(), options::NoClear)?;

    if let Doc::Rich { html, .. } = doc
        && let Some(format) = html_format()
    {
        raw::set_without_clear(format.get(), &cf_html::build(html))?;
    }

    Ok(())
}

fn html_format() -> Option<NonZeroU32> {
    raw::register_format("HTML Format")
}

/// A counter Windows bumps on every clipboard change.
///
/// Cheaper than opening the clipboard, which is what makes it fit for the
/// poll that notices a Ctrl+C while the popup is open.
pub fn sequence() -> u32 {
    raw::seq_num().map_or(0, NonZeroU32::get)
}
