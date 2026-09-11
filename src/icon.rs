//! The application icon, shared by the tray, the window and the taskbar.
//!
//! Decoded at startup rather than shipped as a raw blob, so the asset in the
//! repository is a picture anyone can open, and so a user's own icon goes
//! through exactly the same path as the built-in one.

use crate::paths;

/// 64px is enough for the tray at 200% scaling and small enough that decoding
/// it costs nothing at startup.
const BUILT_IN: &[u8] = include_bytes!("../assets/unslop.png");

/// 32-bit RGBA, the one format both `tao` and `tray-icon` accept.
pub struct Image {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// The configured icon, or the built-in one if there is none or it will not
/// load. An unreadable icon is a cosmetic problem, never a fatal one.
pub fn load(path: &str) -> Image {
    if !path.is_empty() {
        match std::fs::read(paths::beside_exe(path)).map(|bytes| decode(&bytes)) {
            Ok(Ok(image)) => return image,
            Ok(Err(err)) => eprintln!("{path} is not a usable PNG, using the built-in icon: {err}"),
            Err(err) => eprintln!("cannot read {path}, using the built-in icon: {err}"),
        }
    }
    decode(BUILT_IN).expect("the built-in icon must decode")
}

fn decode(bytes: &[u8]) -> Result<Image, String> {
    let mut reader = png::Decoder::new(std::io::Cursor::new(bytes))
        .read_info()
        .map_err(|err| err.to_string())?;
    let mut buffer = vec![
        0;
        reader
            .output_buffer_size()
            .ok_or("the image is too large")?
    ];
    let info = reader
        .next_frame(&mut buffer)
        .map_err(|err| err.to_string())?;
    buffer.truncate(info.buffer_size());

    // Icons are usually RGBA already, but a PNG saved without transparency is
    // three bytes a pixel and would otherwise render as coloured noise.
    let rgba = match info.color_type {
        png::ColorType::Rgba => buffer,
        png::ColorType::Rgb => buffer
            .chunks_exact(3)
            .flat_map(|pixel| [pixel[0], pixel[1], pixel[2], 255])
            .collect(),
        other => {
            return Err(format!(
                "{other:?} images are not supported, use RGB or RGBA"
            ));
        }
    };
    Ok(Image {
        rgba,
        width: info.width,
        height: info.height,
    })
}
