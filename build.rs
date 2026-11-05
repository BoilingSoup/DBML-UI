// Embeds the app icon into the Windows executable (Explorer + taskbar).
// Other targets build nothing — the window icon comes from main.rs and
// the bundles carry the PNGs.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("icons/app-icon.ico");
    res.compile().expect("embed app-icon.ico into the exe");
}
