//! Embeds Tern's icon (16 to 256 px, from the brand mark) and version
//! information into the Windows executable, so Explorer and the taskbar
//! show the Tern mark. GPUI loads the window icon from resource 1, which
//! is the id `winresource` gives the icon. Also gives the Windows
//! executable an 8 MB main-thread stack. Nothing happens on other systems.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // An 8 MB main-thread stack on Windows, as on Linux and macOS. Windows
    // gives an executable's main thread 1 MB, and GPUI lays out and paints
    // its element tree recursively on that thread: a deep view (the Explain
    // in 3D stage with its labels, caption and step bar inside the
    // workspace) can run past 1 MB, and a stack overflow ends the process
    // at once, with no panic to catch or log. Zed's own build sets the
    // same size for the same reason.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
            println!("cargo:rustc-link-arg-bins=/STACK:8388608");
        } else {
            println!("cargo:rustc-link-arg-bins=-Wl,--stack,8388608");
        }
    }
    println!("cargo:rerun-if-changed=../../assets/brand/tern.ico");
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../assets/brand/tern.ico")
            .set("ProductName", "Tern")
            .set("FileDescription", "Tern")
            .set("CompanyName", "Khubaib Nazeer")
            .set("LegalCopyright", "Copyright (c) Khubaib Nazeer");
        if let Err(e) = res.compile() {
            // A missing resource compiler must not stop the build; the
            // program still runs, with Windows' generic icon.
            println!("cargo:warning=Tern's Windows icon was not embedded: {e}");
        }
    }
}
