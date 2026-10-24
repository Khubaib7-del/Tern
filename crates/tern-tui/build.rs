//! Embeds Tern's icon (16 to 256 px, from the brand mark) and version
//! information into the Windows executable, so Explorer and the taskbar
//! show the Tern mark. GPUI loads the window icon from resource 1, which
//! is the id `winresource` gives the icon. Nothing happens on other
//! systems.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../assets/brand/tern.ico");
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../assets/brand/tern.ico")
            .set("ProductName", "Tern")
            .set("FileDescription", "Tern command line")
            .set("CompanyName", "Khubaib Nazeer")
            .set("LegalCopyright", "Copyright (c) Khubaib Nazeer");
        if let Err(e) = res.compile() {
            // A missing resource compiler must not stop the build; the
            // program still runs, with Windows' generic icon.
            println!("cargo:warning=Tern's Windows icon was not embedded: {e}");
        }
    }
}
