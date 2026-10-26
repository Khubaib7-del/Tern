//! The Windows test binaries of the Vulkan speech build delay-load the
//! Vulkan loader (vulkan-1.dll), as the app does, so they run on machines
//! and CI runners without a GPU driver (#67).

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var_os("CARGO_FEATURE_WHISPER_VULKAN").is_some()
        && std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
    {
        println!("cargo:rustc-link-arg=/DELAYLOAD:vulkan-1.dll");
        println!("cargo:rustc-link-arg=delayimp.lib");
    }
}
