// The icon Windows shows for the executable itself, in Explorer, the taskbar
// and Alt+Tab. It has to be compiled in as a resource; nothing at runtime can
// set it.
fn main() {
    if std::env::var("CARGO_CFG_WINDOWS").is_ok() {
        println!("cargo:rerun-if-changed=assets/unslop.ico");
        winresource::WindowsResource::new()
            .set_icon("assets/unslop.ico")
            .compile()
            .expect("embed the application icon");
    }
}
