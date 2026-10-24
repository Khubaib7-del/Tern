//! A plain-text record of what went wrong, for the next bug report: a
//! panic anywhere (with its place and a backtrace) goes to
//! `<data>/logs/crash.log` before the default handler runs, and notes such
//! as a slow Explain in 3D frame go to `<data>/logs/tern.log`. Nothing is
//! sent anywhere; both files only grow to [`MAX_LOG`] before starting over.

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
