//! A plain-text record of what went wrong, for the next bug report: a
//! panic anywhere (with its place and a backtrace) goes to
//! `<data>/logs/crash.log` before the default handler runs, and notes such
//! as a slow Explain in 3D frame go to `<data>/logs/tern.log`. Nothing is
//! sent anywhere; both files only grow to [`MAX_LOG`] before starting over.
//!
//! Some failures never reach a panic hook: a stack overflow, an abort, or
//! a crash in a graphics driver ends the process at once. On Windows
//! release builds Tern therefore starts as a small watchdog that runs the
//! app as a child process ([`watch`]): when the app ends with anything but
//! success, the watchdog writes the exit code (and what it means), the last
//! thing the app was doing ([`breadcrumb`], `last-action.txt`) and the end
//! of the app's error output (`stderr.log`) to `crash.log`.
//!
//! On Windows the logs are in `%LOCALAPPDATA%\Tern\data\logs\`.

use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Largest a log grows before it starts over, bytes.
const MAX_LOG: u64 = 512 * 1024;
/// Notes are written at most this often, so a run of slow frames is one
/// line, not hundreds.
const NOTE_EVERY: Duration = Duration::from_secs(5);

fn logs_dir() -> Option<PathBuf> {
    let d = tern_agent::platform::AppPaths::discover()
        .ok()?
        .data_dir
        .join("logs");
    std::fs::create_dir_all(&d).ok()?;
    Some(d)
}

/// The environment variable that marks the app run by the watchdog.
const WATCHED: &str = "TERN_WATCHED";
/// Set to anything to run without the watchdog.
const NO_WATCHDOG: &str = "TERN_NO_WATCHDOG";

fn append(name: &str, text: &str) {
    let Some(path) = logs_dir().map(|d| d.join(name)) else {
        return;
    };
    let too_big = std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_LOG);
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(!too_big)
        .truncate(too_big)
        .open(&path);
    if let Ok(mut f) = file {
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let _ = writeln!(f, "[{secs}] {text}");
    }
}

/// Records every panic in `crash.log`, then runs the default handler.
pub fn install() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let place = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_default();
        let msg = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_default();
        let thread = std::thread::current().name().unwrap_or("?").to_owned();
        let bt = std::backtrace::Backtrace::force_capture();
        append(
            "crash.log",
            &format!("panic on thread {thread} at {place}: {msg}\n{bt}"),
        );
        default(info);
    }));
}

/// Writes `text` to `tern.log`, at most once every [`NOTE_EVERY`].
pub fn note(text: &str) {
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    let Ok(mut last) = LAST.lock() else {
        return;
    };
    let now = Instant::now();
    if last.is_some_and(|t| now.duration_since(t) < NOTE_EVERY) {
        return;
    }
    *last = Some(now);
    append("tern.log", text);
}

/// Records what the app is doing now (`explain: step 2 of 15`), so the
/// watchdog can say what was on screen when the process ended. One small
/// file, overwritten; call it for user actions, not per frame.
pub fn breadcrumb(text: &str) {
    if let Some(d) = logs_dir() {
        let _ = std::fs::write(d.join("last-action.txt"), text);
    }
}

/// What a Windows exit code means, for the crash log.
pub fn exit_meaning(code: i32) -> &'static str {
    // Exit codes are NTSTATUS values; compare their bits.
    match code as u32 {
        0xC000_00FD => "stack overflow",
        0xC000_0409 => {
            "fail-fast abort (a Rust abort, a panic that could not unwind, or a buffer overrun check)"
        }
        0xC000_0005 => "access violation (often a graphics driver)",
        0xC000_0374 => "heap corruption",
        0xC000_001D => "illegal instruction",
        0xC000_0094 => "integer divide by zero",
        0x887A_0005 | 0x887A_0006 | 0x887A_0007 => "the graphics device was removed or reset",
        0x4000_0015 => "abort() (STATUS_FATAL_APP_EXIT)",
        3 => "abort()",
        101 => "a panic on the main thread",
        _ => "unexpected exit",
    }
}

/// The crash.log entry for an app that ended with `code`.
pub fn exit_report(code: i32, last_action: &str, stderr_tail: &str) -> String {
    let mut out = format!(
        "Tern stopped unexpectedly: exit code {code} (0x{:08X}, {})",
        code as u32,
        exit_meaning(code)
    );
    let last = last_action.trim();
    if !last.is_empty() {
        out.push_str(&format!("\nlast action: {last}"));
    }
    let tail = stderr_tail.trim();
    if !tail.is_empty() {
        out.push_str(&format!("\nerror output (end):\n{tail}"));
    }
    out
}

/// The last `max` bytes of `text`, starting at a character boundary.
pub fn tail(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut at = text.len() - max;
    while !text.is_char_boundary(at) {
        at += 1;
    }
    &text[at..]
}

/// On Windows release builds, runs the app as a child process and waits
/// for it, then returns the exit code to end with; logs an unexpected end
/// to `crash.log`. Returns `None` (run the app in this process) everywhere
/// else, in the child, when `TERN_NO_WATCHDOG` is set, or when the child
/// cannot be started.
pub fn watch() -> Option<i32> {
    if !cfg!(all(windows, not(debug_assertions)))
        || std::env::var_os(WATCHED).is_some()
        || std::env::var_os(NO_WATCHDOG).is_some()
    {
        return None;
    }
    let exe = std::env::current_exe().ok()?;
    let dir = logs_dir()?;
    let _ = std::fs::remove_file(dir.join("last-action.txt"));
    let err = std::fs::File::create(dir.join("stderr.log")).ok()?;
    let status = std::process::Command::new(exe)
        .args(std::env::args_os().skip(1))
        .env(WATCHED, "1")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(err)
        .status()
        .ok()?;
    let code = status.code().unwrap_or(1);
    if !status.success() {
        let last = std::fs::read_to_string(dir.join("last-action.txt")).unwrap_or_default();
        let errs = std::fs::read(dir.join("stderr.log")).unwrap_or_default();
        let errs = String::from_utf8_lossy(&errs);
        append("crash.log", &exit_report(code, &last, tail(&errs, 4_000)));
    }
    Some(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_are_named_and_reported() {
        assert_eq!(exit_meaning(0xC000_00FDu32 as i32), "stack overflow");
        assert!(exit_meaning(0xC000_0409u32 as i32).starts_with("fail-fast"));
        assert_eq!(exit_meaning(7), "unexpected exit");
        let r = exit_report(
            0xC000_00FDu32 as i32,
            "explain: step 2 of 15\n",
            "thread 'main' has overflowed its stack",
        );
        assert!(r.contains("0xC00000FD, stack overflow"), "{r}");
        assert!(r.contains("last action: explain: step 2 of 15"));
        assert!(r.contains("overflowed its stack"));
        assert!(!exit_report(1, "", "").contains("last action"));
    }

    #[test]
    fn tail_keeps_the_end_on_a_character_boundary() {
        assert_eq!(tail("abc", 10), "abc");
        assert_eq!(tail("abcdef", 3), "def");
        let s = "ééé";
        let t = tail(s, 3);
        assert!(s.ends_with(t) && t.len() <= 3);
    }
}
