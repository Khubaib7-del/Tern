//! Embeds Tern's icon (16 to 256 px, from the brand mark) and version
//! information into the Windows executable, so Explorer and the taskbar
//! show the Tern mark. GPUI loads the window icon from resource 1, which
//! is the id `winresource` gives the icon. Also gives the Windows
//! executable an 8 MB main-thread stack. Nothing happens on other systems.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // whisper.cpp's Vulkan backend links the Vulkan loader, vulkan-1.dll,
    // which comes with a GPU driver. Delay-loaded, so Tern starts on a
    // machine without it; tern_agent::local_models::gpu keeps the speech
    // engine on the CPU there (#67).
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
    {
        println!("cargo:rustc-link-arg=/DELAYLOAD:vulkan-1.dll");
        println!("cargo:rustc-link-arg=delayimp.lib");
    }
    embed_dir("assets/scout", "SCOUT_FILES", "scout_assets.rs");
    embed_dir("assets/mark", "MARK_FILES", "mark_assets.rs");
    // An 8 MB main-thread stack on Windows, as on Linux and macOS. Windows
    // gives an executable's main thread 1 MB, and GPUI lays out and paints
    // its element tree recursively on that thread, and a stack overflow
    // ends the process at once, with no panic to catch or log. Zed's own
    // build sets the same size for the same reason. (PR #56 added this for
    // the Explain in 3D crash on Next and Play; the crash log later showed
    // that crash was a panic in GPUI's `current_view`, fixed in PR #57,
    // not an overflow. The headroom stays.)
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

/// Embeds every file in `dir` as a `(name, bytes)` table: Scout's sprite
/// strips and `scout.json` (`crate::pet`), and the empty state's mark
/// strips and `mark.json` (`crate::mark`), so new art from the pipeline
/// drops in by file name with no code change.
fn embed_dir(rel: &str, table: &str, out_name: &str) {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .map(|r| r.filter_map(|e| e.ok()).map(|e| e.path()).collect())
        .unwrap_or_default();
    files.retain(|p| p.is_file());
    files.sort();
    let mut out = format!("pub(crate) const {table}: &[(&str, &[u8])] = &[\n");
    for p in files {
        println!("cargo:rerun-if-changed={}", p.display());
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        out.push_str(&format!(
            "    ({name:?}, include_bytes!({:?})),\n",
            p.display().to_string()
        ));
    }
    out.push_str("];\n");
    let dest = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join(out_name);
    std::fs::write(dest, out).unwrap();
}
