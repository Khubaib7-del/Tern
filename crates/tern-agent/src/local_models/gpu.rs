//! Finding a GPU for whisper.cpp (#67): Vulkan on Windows and Linux
//! (`whisper-vulkan`), Metal on macOS (`whisper-metal`).
//!
//! ggml registers its backends the first time anything asks for a device.
//! The Vulkan backend then starts a Vulkan instance; a missing driver or
//! no device makes it register nothing (ggml catches that), so the CPU is
//! the only device and Whisper runs there.
//!
//! On Windows `vulkan-1.dll` (the Vulkan loader, installed with a GPU
//! driver) is delay-loaded (see `build.rs` of the binaries), so the app
//! starts without it. If the loader is missing when ggml first calls it,
//! the delay-load failure hook below hands ggml a stand-in
//! `vkGetInstanceProcAddr` whose every function returns
//! `VK_ERROR_INITIALIZATION_FAILED`: ggml's instance start throws, catches
//! its own error, and registers no Vulkan device. Without the hook the
//! delay-load helper would raise an SEH exception and end the process.
#![allow(unsafe_code)]

/// A GPU whisper.cpp can use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Gpu {
    /// Its place among ggml's GPU devices (Whisper's `gpu_device`).
    pub index: i32,
    /// `vulkan` or `metal`.
    pub api: &'static str,
    /// The device's own name ("Intel(R) UHD Graphics 620").
    pub name: String,
}

/// The GPU API this build was made with, if any.
pub const API: Option<&str> = if cfg!(feature = "whisper-vulkan") {
    Some("vulkan")
} else if cfg!(feature = "whisper-metal") {
    Some("metal")
} else {
    None
};

/// The first GPU ggml can use, or why there is none. Looked up once per
/// process.
pub fn first() -> Result<Gpu, String> {
    FOUND.get_or_init(find).clone()
}

static FOUND: std::sync::OnceLock<Result<Gpu, String>> = std::sync::OnceLock::new();

/// [`first`] when it has already run, without starting the lookup.
pub fn peek() -> Option<Result<Gpu, String>> {
    FOUND.get().cloned()
}

#[cfg(not(any(feature = "whisper-vulkan", feature = "whisper-metal")))]
fn find() -> Result<Gpu, String> {
    Err("this build has no GPU speech".into())
}

#[cfg(any(feature = "whisper-vulkan", feature = "whisper-metal"))]
fn find() -> Result<Gpu, String> {
    use std::ffi::CStr;
    use whisper_rs::whisper_rs_sys as sys;

    // Without the Vulkan loader ggml's registry still starts (every
    // transcription starts it, on the CPU too): asking for the devices
    // here goes through the delay-load hook the same way, so a broken
    // hook shows on the first look rather than in the middle of a run.
    #[cfg(all(windows, feature = "whisper-vulkan"))]
    let no_loader = {
        win::keep_hook();
        !win::loader_present()
    };
    #[cfg(not(all(windows, feature = "whisper-vulkan")))]
    let no_loader = false;
    let text = |p: *const std::os::raw::c_char| {
        if p.is_null() {
            String::new()
        } else {
            // SAFETY: ggml returns NUL-terminated strings it owns for the
            // life of the process.
            unsafe { CStr::from_ptr(p) }
                .to_string_lossy()
                .trim()
                .to_owned()
        }
    };
    // SAFETY: ggml's registry calls; each device pointer comes from the
    // registry and lives as long as the process.
    let n = unsafe { sys::ggml_backend_dev_count() };
    for i in 0..n {
        let dev = unsafe { sys::ggml_backend_dev_get(i) };
        if dev.is_null() {
            continue;
        }
        let kind = unsafe { sys::ggml_backend_dev_type(dev) };
        if kind == sys::ggml_backend_dev_type_GGML_BACKEND_DEVICE_TYPE_GPU
            || kind == sys::ggml_backend_dev_type_GGML_BACKEND_DEVICE_TYPE_IGPU
        {
            let mut name = text(unsafe { sys::ggml_backend_dev_description(dev) });
            if name.is_empty() {
                name = text(unsafe { sys::ggml_backend_dev_name(dev) });
            }
            // The first GPU-type device: Whisper's `gpu_device` 0.
            crate::diag::note(&format!(
                "speech: gpu probe: {} {name}",
                API.unwrap_or("gpu")
            ));
            return Ok(Gpu {
                index: 0,
                api: API.unwrap_or("gpu"),
                name,
            });
        }
    }
    crate::diag::note(&format!(
        "speech: gpu probe: {n} ggml devices, none a GPU{}",
        if no_loader { " (no vulkan-1.dll)" } else { "" }
    ));
    Err(match API {
        _ if no_loader => "no Vulkan driver (vulkan-1.dll not found)".into(),
        Some("vulkan") => "no Vulkan GPU found".into(),
        Some("metal") => "no Metal GPU found".into(),
        _ => "no GPU found".into(),
    })
}

/// The Windows side: whether the Vulkan loader is there, and the
/// delay-load failure hook.
#[cfg(all(windows, feature = "whisper-vulkan"))]
mod win {
    use std::ffi::{CStr, c_char, c_void};

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LoadLibraryW(name: *const u16) -> *mut c_void;
        fn GetModuleHandleW(name: *const u16) -> *mut c_void;
    }

    /// Whether `vulkan-1.dll` loads (it stays loaded, which is what the
    /// delay-load helper then finds).
    pub fn loader_present() -> bool {
        let name: Vec<u16> = "vulkan-1.dll\0".encode_utf16().collect();
        // SAFETY: a NUL-terminated wide string.
        !unsafe { LoadLibraryW(name.as_ptr()) }.is_null()
    }

    /// `DelayLoadProc` from delayimp.h.
    #[repr(C)]
    #[allow(dead_code)]
    pub struct DelayLoadProc {
        f_import_by_name: i32,
        sz_proc_name: *const c_char,
    }

    /// `DelayLoadInfo` from delayimp.h.
    #[repr(C)]
    #[allow(dead_code)]
    pub struct DelayLoadInfo {
        cb: u32,
        pidd: *const c_void,
        pp_iat: *mut *mut c_void,
        sz_dll: *const c_char,
        dlp: DelayLoadProc,
        hmod_cur: *mut c_void,
        pfn_cur: *mut c_void,
        dw_last_error: u32,
    }

    const DLI_FAIL_LOAD_LIB: u32 = 3;
    const DLI_FAIL_GET_PROC: u32 = 4;
    /// `VK_ERROR_INITIALIZATION_FAILED`.
    const VK_INIT_FAILED: i32 = -3;

    /// Every Vulkan call through the stand-in loader fails this way.
    unsafe extern "system" fn vk_fails() -> i32 {
        VK_INIT_FAILED
    }

    /// The stand-in `vkGetInstanceProcAddr`: every function it hands out
    /// is [`vk_fails`] (on x64 the caller cleans the stack, so the extra
    /// arguments are harmless).
    unsafe extern "system" fn stand_in_get_instance_proc_addr(
        _instance: *mut c_void,
        _name: *const c_char,
    ) -> *mut c_void {
        vk_fails as unsafe extern "system" fn() -> i32 as *mut c_void
    }

    unsafe extern "system" fn delay_load_failed(
        notify: u32,
        info: *mut DelayLoadInfo,
    ) -> *mut c_void {
        if info.is_null() {
            return std::ptr::null_mut();
        }
        // SAFETY: the delay-load helper passes a valid DelayLoadInfo.
        let dll = unsafe { (*info).sz_dll };
        if dll.is_null()
            || !unsafe { CStr::from_ptr(dll) }
                .to_bytes()
                .eq_ignore_ascii_case(b"vulkan-1.dll")
        {
            return std::ptr::null_mut();
        }
        match notify {
            // Any module handle will do: the next step asks it for the
            // function, fails, and comes back here for it.
            DLI_FAIL_LOAD_LIB => unsafe { GetModuleHandleW(std::ptr::null()) },
            DLI_FAIL_GET_PROC => {
                stand_in_get_instance_proc_addr
                    as unsafe extern "system" fn(*mut c_void, *const c_char) -> *mut c_void
                    as *mut c_void
            }
            _ => std::ptr::null_mut(),
        }
    }

    type Hook = unsafe extern "system" fn(u32, *mut DelayLoadInfo) -> *mut c_void;

    /// Read by the MSVC delay-load helper (delayimp.lib) when a delayed
    /// DLL or function cannot be found.
    #[unsafe(no_mangle)]
    #[used]
    #[allow(non_upper_case_globals)]
    pub static __pfnDliFailureHook2: Hook = delay_load_failed;

    /// Keeps the hook in the binary.
    pub fn keep_hook() {
        std::hint::black_box(&__pfnDliFailureHook2);
    }
}

/// Keeps the Windows delay-load hook linked in (called at engine start).
pub fn keep_hook() {
    #[cfg(all(windows, feature = "whisper-vulkan"))]
    win::keep_hook();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_gpu_or_a_reason_and_never_a_crash() {
        match first() {
            Ok(g) => {
                assert!(API.is_some());
                assert!(g.index >= 0);
            }
            Err(why) => assert!(!why.is_empty()),
        }
    }
}
