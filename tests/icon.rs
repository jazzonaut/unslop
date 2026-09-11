//! The icon is cosmetic, so the only thing that matters is that a bad one
//! never takes the application down and that the built-in one always works.

use unslop::icon;

#[test]
fn the_built_in_icon_decodes_to_rgba() {
    let image = icon::load("");
    assert_eq!(image.width, image.height, "the icon should be square");
    assert_eq!(
        image.rgba.len() as u32,
        image.width * image.height * 4,
        "expected four bytes a pixel"
    );
}

#[test]
fn an_unusable_icon_falls_back_rather_than_panicking() {
    let missing = icon::load("no-such-file.png");
    let built_in = icon::load("");
    assert_eq!(missing.rgba, built_in.rgba);

    let not_a_png = std::env::temp_dir().join("unslop-not-an-icon.png");
    std::fs::write(&not_a_png, b"this is not a PNG").unwrap();
    assert_eq!(
        icon::load(not_a_png.to_str().unwrap()).rgba,
        built_in.rgba,
        "a corrupt file should fall back to the built-in icon"
    );
}
