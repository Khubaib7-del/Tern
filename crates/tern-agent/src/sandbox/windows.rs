//! Windows: a Job Object holds the command's whole process tree. The
//! process starts suspended, joins the job, then runs, so nothing it
//! starts escapes. The job kills the tree when its handle closes, and
//! limits job memory, the number of processes and CPU time.
#![allow(unsafe_code)]

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
    JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION, JOB_OBJECT_LIMIT_JOB_MEMORY,
    JOB_OBJECT_LIMIT_JOB_TIME, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::Threading::{
    CREATE_SUSPENDED, OpenThread, ResumeThread, THREAD_SUSPEND_RESUME,
};

use super::Limits;

/// No console window; start suspended so the job holds it from the start.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub(super) fn configure(cmd: &mut tokio::process::Command) {
    cmd.creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
}

/// The job holding a command's tree.
pub(super) struct Tree {
    job: Option<HANDLE>,
}

// SAFETY: a job handle may be used from any thread.
unsafe impl Send for Tree {}
unsafe impl Sync for Tree {}

impl Tree {
    /// Kills every process in the job.
    pub(super) fn kill(&self) {
        if let Some(j) = self.job {
            // SAFETY: a live job handle this value owns.
            unsafe {
                TerminateJobObject(j, 1);
            }
        }
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        if let Some(j) = self.job.take() {
            // SAFETY: closing the last handle kills what is left (kill on
            // close).
            unsafe {
                CloseHandle(j);
            }
        }
    }
}

/// Puts the suspended child in a new job with the limits, then lets it
/// run. Without a job (an old system refusing it), it runs anyway and
/// tern.log says so; the time limit still kills the process itself.
pub(super) fn contain(child: &tokio::process::Child, limits: &Limits) -> Tree {
    let job = make_job(limits);
    let pid = child.id();
    if let (Some(j), Some(h)) = (job, child.raw_handle()) {
        // SAFETY: both handles are live; the child is suspended.
        let ok = unsafe { AssignProcessToJobObject(j, h) } != 0;
        if !ok {
            crate::diag::note(&format!(
                "sandbox: could not put the process in a job object: {}",
                std::io::Error::last_os_error()
            ));
        }
    } else if job.is_none() {
        crate::diag::note(&format!(
            "sandbox: job object unavailable: {}; running without it",
            std::io::Error::last_os_error()
        ));
    }
    if let Some(pid) = pid {
        resume(pid);
    }
    Tree { job }
}

fn make_job(limits: &Limits) -> Option<HANDLE> {
    // SAFETY: plain Win32 calls with valid arguments; the struct is
    // zeroed plain data.
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            return None;
        }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
            | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION
            | JOB_OBJECT_LIMIT_JOB_MEMORY
            | JOB_OBJECT_LIMIT_ACTIVE_PROCESS
            | JOB_OBJECT_LIMIT_JOB_TIME;
        info.BasicLimitInformation.ActiveProcessLimit = limits.processes.max(1);
        // 100 ns units.
        info.BasicLimitInformation.PerJobUserTimeLimit =
            i64::try_from(limits.cpu_secs.saturating_mul(10_000_000)).unwrap_or(i64::MAX);
        info.JobMemoryLimit =
            usize::try_from(limits.memory_mb.saturating_mul(1024 * 1024)).unwrap_or(usize::MAX);
        let ok = SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            (&raw const info).cast(),
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        ) != 0;
        if !ok {
            crate::diag::note(&format!(
                "sandbox: job limits not set: {}",
                std::io::Error::last_os_error()
            ));
        }
        Some(job)
    }
}

/// Resumes every thread of a process that was started suspended.
fn resume(pid: u32) {
    // SAFETY: a thread snapshot walked with a correctly sized entry;
    // every handle opened is closed.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snap == INVALID_HANDLE_VALUE || snap.is_null() {
            crate::diag::note("sandbox: could not list threads to resume the command");
            return;
        }
        let mut e: THREADENTRY32 = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
        let mut more = Thread32First(snap, &mut e) != 0;
        while more {
            if e.th32OwnerProcessID == pid {
                let t = OpenThread(THREAD_SUSPEND_RESUME, 0, e.th32ThreadID);
                if !t.is_null() {
                    ResumeThread(t);
                    CloseHandle(t);
                }
            }
            more = Thread32Next(snap, &mut e) != 0;
        }
        CloseHandle(snap);
    }
}
