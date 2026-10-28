//! Linux and macOS: the command leads its own process group, killed as a
//! group, under rlimits (CPU time, core dumps off, file size; address
//! space on Linux).
#![allow(unsafe_code)]

use super::Limits;

/// Sets the group and the limits before the program starts.
pub(super) fn configure(cmd: &mut tokio::process::Command, limits: &Limits) {
    cmd.process_group(0);
    let cpu = limits.cpu_secs;
    #[cfg(target_os = "linux")]
    let mem = limits.memory_mb.saturating_mul(1024 * 1024);
    // SAFETY: the closure runs in the child between fork and exec and only
    // calls setrlimit, which is async-signal-safe; it allocates nothing.
    unsafe {
        cmd.pre_exec(move || {
            set(libc::RLIMIT_CPU, cpu);
            set(libc::RLIMIT_CORE, 0);
            set(libc::RLIMIT_FSIZE, 4 * 1024 * 1024 * 1024);
            #[cfg(target_os = "linux")]
            set(libc::RLIMIT_AS, mem);
            Ok(())
        });
    }
}

#[cfg(target_os = "linux")]
type Resource = libc::__rlimit_resource_t;
#[cfg(not(target_os = "linux"))]
type Resource = libc::c_int;

fn set(resource: Resource, value: u64) {
    let lim = libc::rlimit {
        rlim_cur: value as libc::rlim_t,
        rlim_max: value as libc::rlim_t,
    };
    // SAFETY: a plain system call with a valid pointer to a local struct.
    unsafe {
        libc::setrlimit(resource, &lim);
    }
}

/// The command's process group.
pub(super) struct Tree {
    pgid: Option<i32>,
}

impl Tree {
    pub(super) fn of(child: &tokio::process::Child) -> Self {
        Self {
            pgid: child.id().and_then(|p| i32::try_from(p).ok()),
        }
    }

    /// Kills every process in the group.
    pub(super) fn kill(&self) {
        if let Some(g) = self.pgid.filter(|g| *g > 1) {
            // SAFETY: kill(2) with a negative pid signals the group.
            unsafe {
                libc::kill(-g, libc::SIGKILL);
            }
        }
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        // Children the command left behind go with it.
        self.kill();
    }
}
