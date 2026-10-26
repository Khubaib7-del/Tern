//! The real browser engine behind the built-in browser's page, run in its
//! own process: `tern-browser` (the system web view, WebView2 on Windows,
//! WKWebView on macOS, WebKitGTK on Linux, with its own event loop),
//! driven over JSON lines on its stdin and stdout
//! (`tern_core::browser_ipc`, `docs/decisions/0011-browser-helper-process.md`).
//!
//! Why a process: creating WebView2 inside GPUI's window ran a nested
//! Windows message pump on GPUI's thread in the middle of a frame, which
//! froze the app and then aborted it (ADR 0011 has the details). Here
//! nothing the engine does can block Tern's thread: the helper is spawned,
//! written to and read from on background threads, and every frame only
//! drains a channel. If the helper does not report ready within
//! [`START_TIMEOUT`], or it exits, the pane falls back to Tern's reader
//! view and offers a restart; Tern itself keeps running.
//!
//! The helper's window is docked over the browser pane's page area (it
//! follows the pane's rectangle on the screen every frame it moves), or,
//! where a window cannot place itself (Wayland) or the user pops it out,
//! shown as an ordinary window beside Tern.

use std::cell::RefCell;
use std::io::{BufRead as _, Write as _};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use tern_core::browser_ipc::{Command, Event, Rect, helper_file_name};

/// How long the helper has to report ready before the pane falls back to
/// the reader view.
pub const START_TIMEOUT: Duration = Duration::from_secs(5);

/// What the page reported since the last look: where it is now and its
/// title, for the address bar and the tab.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reported {
    pub url: Option<String>,
    pub title: Option<String>,
}

/// Where the helper is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// Not started yet (the page has not been shown).
    Idle,
    /// Spawned at this instant, waiting for its `ready`.
    Starting(Instant),
    /// Up, with this engine.
    Ready(String),
    /// It never came up, or it exited: why, for the pane.
    Stopped(String),
}

/// A running helper process: commands go out through a writer thread,
/// events come in through a reader thread.
struct Process {
    child: std::process::Child,
    tx: mpsc::Sender<String>,
    rx: mpsc::Receiver<Incoming>,
}

enum Incoming {
    Ev(Event),
    /// Its stdout closed: it exited or crashed.
    Gone,
}

impl Process {
    fn spawn(exe: &std::path::Path, args: &[String]) -> Result<Self, String> {
        let mut child = std::process::Command::new(exe)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| format!("could not start {}: {e}", exe.display()))?;
        let mut stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let (tx, wrx) = mpsc::channel::<String>();
        std::thread::Builder::new()
            .name("tern-browser-out".into())
            .spawn(move || {
                for line in wrx {
                    if stdin.write_all(line.as_bytes()).is_err() || stdin.flush().is_err() {
                        break;
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        let (etx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("tern-browser-in".into())
            .spawn(move || {
                let reader = std::io::BufReader::new(stdout);
                for line in reader.lines() {
                    let Ok(line) = line else { break };
                    if let Some(ev) = Event::parse(&line)
                        && etx.send(Incoming::Ev(ev)).is_err()
                    {
                        return;
                    }
                }
                let _ = etx.send(Incoming::Gone);
            })
            .map_err(|e| e.to_string())?;
        Ok(Self { child, tx, rx })
    }

    fn send(&self, c: &Command) {
        let _ = self.tx.send(c.to_line());
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        self.send(&Command::Quit);
        // Give it a moment to close its window cleanly, then make sure.
        std::thread::sleep(Duration::from_millis(20));
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

/// The engine host: one helper per Tern window, reused across tabs (a tab
/// switch loads that tab's address).
pub struct Host {
    proc: Option<Process>,
    pub status: Status,
    /// The address the page should show.
    want: Option<String>,
    /// The address last sent to the helper.
    sent: Option<String>,
    /// The rectangle on the screen it was last docked on.
    bounds: Option<Rect>,
    /// What the helper was last told about visibility.
    shown: bool,
    /// The page element was placed since the last frame check.
    seen: bool,
    /// The helper's window has the keyboard focus (so Tern being inactive
    /// does not hide it).
    page_focused: bool,
    /// The page shows as its own window beside Tern, not docked.
    pub detached: bool,
    /// The helper said it cannot dock (Wayland).
    cannot_dock: bool,
    reports: Reported,
    /// Where the engine keeps its profile (cookies, cache).
    pub data_dir: Option<PathBuf>,
    /// Tern's native window handle, so the page window can be owned by it
    /// (Windows).
    pub owner: Option<isize>,
    /// Starts so far, to stop retrying forever when it keeps failing.
    starts: u32,
    /// Since when the page has been off screen for a soft reason (not
    /// placed this frame, or neither window in front), still shown.
    away_since: Option<Instant>,
    /// Show and hide commands sent, for the flicker check (`TERN_PERF`).
    pub shows: u32,
    pub hides: u32,
}

/// How long the page may miss being placed, or have neither Tern nor the
/// page in front, before its window is hidden (#68). Hiding at once made
/// the native page flash dark and back: a frame that re-rendered without
/// painting the pane, or the moment focus passes from Tern to the page,
/// hid it and the next frame showed it again.
pub const AWAY_GRACE: std::time::Duration = std::time::Duration::from_millis(250);

impl std::fmt::Debug for Host {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Host")
            .field("status", &self.status)
            .field("want", &self.want)
            .field("shown", &self.shown)
            .finish_non_exhaustive()
    }
}

impl Default for Host {
    fn default() -> Self {
        Self {
            proc: None,
            status: Status::Idle,
            want: None,
            sent: None,
            bounds: None,
            shown: false,
            seen: false,
            page_focused: false,
            detached: false,
            cannot_dock: false,
            reports: Reported::default(),
            data_dir: None,
            owner: None,
            starts: 0,
            away_since: None,
            shows: 0,
            hides: 0,
        }
    }
}

/// Shared between the workspace and the page element's prepaint.
pub type Shared = Rc<RefCell<Host>>;

/// Where the helper binary is: `TERN_BROWSER_HELPER`, else next to this
/// executable (also inside a macOS app bundle's `MacOS` folder).
pub fn helper_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("TERN_BROWSER_HELPER") {
        return Some(PathBuf::from(p));
    }
    let exe = std::env::current_exe().ok()?;
    let p = exe.parent()?.join(helper_file_name());
    p.exists().then_some(p)
}

/// Whether a page window cannot be docked on this session (Wayland: a
/// client cannot place its own window), so it opens beside Tern.
pub fn cannot_dock_here() -> bool {
    cfg!(all(unix, not(target_os = "macos")))
        && std::env::var_os("WAYLAND_DISPLAY").is_some()
        && std::env::var("GDK_BACKEND").map_or(true, |b| !b.contains("x11"))
}

impl Host {
    /// Whether the helper should be read every so often (it is starting,
    /// or the page is up and may report a new address or title).
    pub fn wants_ticks(&self) -> bool {
        matches!(self.status, Status::Starting(_) | Status::Ready(_)) || self.away_since.is_some()
    }

    pub fn shared() -> Shared {
        let data_dir = tern_agent::platform::AppPaths::discover()
            .ok()
            .map(|p| p.cache_dir.join("webview"));
        Rc::new(RefCell::new(Self {
            data_dir,
            ..Self::default()
        }))
    }

    /// The page should show `url` (a navigation from the bar, a tab, a
    /// citation).
    pub fn go(&mut self, url: &str) {
        self.want = Some(url.to_owned());
        self.push_url();
    }

    /// Takes what the engine reported since the last call.
    pub fn take_reports(&mut self) -> Reported {
        std::mem::take(&mut self.reports)
    }

    /// Whether the real page is up (else the pane shows the reader).
    pub fn is_live(&self) -> bool {
        matches!(self.status, Status::Ready(_))
    }

    /// Why the page is not live, when it stopped.
    pub fn stopped(&self) -> Option<&str> {
        match &self.status {
            Status::Stopped(why) => Some(why),
            _ => None,
        }
    }

    pub fn is_idle(&self) -> bool {
        self.status == Status::Idle
    }

    pub fn is_starting(&self) -> bool {
        matches!(self.status, Status::Starting(_))
    }

    fn send(&self, c: Command) {
        if let Some(p) = &self.proc {
            p.send(&c);
        }
    }

    pub fn reload(&self) {
        self.send(Command::Reload);
    }

    /// Pops the page out into its own window (or docks it back).
    pub fn set_detached(&mut self, detached: bool) {
        self.detached = detached || self.cannot_dock;
        self.send(Command::Detach {
            detached: self.detached,
        });
        if !self.detached {
            self.bounds = None;
        }
    }

    fn push_url(&mut self) {
        if self.is_live() && self.want.is_some() && self.want != self.sent {
            if let Some(u) = self.want.clone() {
                self.send(Command::Navigate { url: u });
            }
            self.sent = self.want.clone();
        }
    }

    /// Starts the helper (again). Never blocks: the process is spawned and
    /// [`Host::frame`] watches for its `ready`.
    pub fn start(&mut self, now: Instant) {
        self.proc = None;
        self.sent = None;
        self.bounds = None;
        self.shown = false;
        self.starts += 1;
        let Some(exe) = helper_path() else {
            self.status = Status::Stopped(format!(
                "the browser helper ({}) is not next to Tern",
                helper_file_name()
            ));
            return;
        };
        let mut args = Vec::new();
        if let Some(d) = &self.data_dir {
            args.push("--data-dir".to_owned());
            args.push(d.display().to_string());
        }
        if let Some(h) = self.owner {
            args.push("--owner".to_owned());
            args.push(h.to_string());
        }
        if let Some(u) = &self.want {
            args.push("--url".to_owned());
            args.push(u.clone());
        }
        match Process::spawn(&exe, &args) {
            Ok(p) => {
                self.sent = self.want.clone();
                self.proc = Some(p);
                self.status = Status::Starting(now);
            }
            Err(e) => self.status = Status::Stopped(e),
        }
    }

    /// Restart after a stop (the pane's button).
    pub fn restart(&mut self) {
        self.starts = 0;
        self.start(Instant::now());
    }

    /// Reads what the helper sent and applies the start timeout.
    fn pump(&mut self, now: Instant) {
        let mut incoming = Vec::new();
        if let Some(p) = &self.proc {
            while let Ok(m) = p.rx.try_recv() {
                incoming.push(m);
            }
        }
        for m in incoming {
            self.on_incoming(m);
        }
        if let Status::Starting(at) = self.status
            && now.saturating_duration_since(at) >= START_TIMEOUT
        {
            self.proc = None;
            self.status = Status::Stopped(format!(
                "the page engine did not start within {} s",
                START_TIMEOUT.as_secs()
            ));
        }
    }

    fn on_incoming(&mut self, m: Incoming) {
        match m {
            Incoming::Ev(Event::Ready {
                engine,
                cannot_dock,
                ..
            }) => {
                self.status = Status::Ready(engine);
                self.cannot_dock = cannot_dock;
                if cannot_dock {
                    self.set_detached(true);
                }
                self.push_url();
            }
            Incoming::Ev(Event::Url { url }) => self.reports.url = Some(url),
            Incoming::Ev(Event::Title { title }) => self.reports.title = Some(title),
            Incoming::Ev(Event::Focus { focused }) => self.page_focused = focused,
            Incoming::Ev(Event::Closed) => {
                // The user closed the popped-out window: dock it back.
                self.detached = self.cannot_dock;
                self.shown = false;
                self.bounds = None;
            }
            Incoming::Ev(Event::Error { message }) => {
                if self.is_starting() {
                    self.proc = None;
                    self.status = Status::Stopped(message);
                }
            }
            Incoming::Ev(Event::Loading { .. }) => {}
            Incoming::Gone => {
                self.proc = None;
                if !matches!(self.status, Status::Stopped(_)) {
                    self.status = Status::Stopped("the page engine stopped".into());
                }
            }
        }
    }

    /// Once per window render: read the helper, hide the page when it
    /// should not show (or its element was not drawn last frame), and keep
    /// it on top only while Tern or the page is in front.
    pub fn frame(&mut self, show: bool, tern_active: bool) {
        let now = Instant::now();
        self.pump(now);
        self.frame_at(show, tern_active, now);
    }

    /// [`Host::frame`] at `now`: hidden at once when the pane is not
    /// showing; hidden after [`AWAY_GRACE`] when it was not placed or
    /// neither window is in front.
    fn frame_at(&mut self, show: bool, tern_active: bool, now: Instant) {
        let front = tern_active || self.page_focused || self.detached;
        if !show {
            self.away_since = None;
            self.hide();
        } else if !self.seen || !front {
            let since = *self.away_since.get_or_insert(now);
            if now.duration_since(since) >= AWAY_GRACE {
                self.away_since = None;
                self.hide();
            }
        } else {
            self.away_since = None;
        }
        self.seen = false;
    }

    /// The page is not on screen: hide its window so it never covers
    /// Tern's own UI.
    pub fn hide(&mut self) {
        if !self.shown {
            return;
        }
        self.shown = false;
        self.hides += 1;
        perf_line("hide", self.shows, self.hides);
        if !self.detached {
            self.send(Command::Hide);
            self.send(Command::OnTop { on_top: false });
        }
    }

    /// Puts the page on `inner` (logical px in Tern's window, whose
    /// content starts at `origin` on the screen), starting the helper the
    /// first time.
    pub fn place(&mut self, inner: Rect, origin: (f64, f64)) {
        self.seen = true;
        match self.status {
            Status::Idle => {
                self.start(Instant::now());
                return;
            }
            Status::Ready(_) => {}
            _ => return,
        }
        let rect = Rect::on_screen(origin, inner);
        if !self.detached && self.bounds.is_none_or(|b| b.moved(&rect)) {
            self.send(Command::Bounds { rect });
            self.bounds = Some(rect);
        }
        if !self.shown {
            self.shows += 1;
            perf_line("show", self.shows, self.hides);
            self.send(Command::Show);
            if !self.detached {
                self.send(Command::OnTop { on_top: true });
            }
            self.shown = true;
        }
        self.push_url();
    }
}

/// A `PERF web` line per show and hide of the page window under
/// `TERN_PERF=1` (stderr and `tern.log`): a flicker is a hide and a show
/// a frame apart.
fn perf_line(what: &str, shows: u32, hides: u32) {
    if !crate::perf::verbose() && !crate::view::tour::enabled() {
        return;
    }
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let line = format!("PERF web {what} shows={shows} hides={hides} unix_ms={ms}");
    if crate::perf::verbose() {
        eprintln!("{line}");
        crate::crashlog::log_line(&line);
    }
    if crate::view::tour::enabled() {
        println!("{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missed_frame_or_a_focus_hand_off_does_not_flash_the_page() {
        let mut h = Host {
            status: Status::Ready("test".into()),
            shown: true,
            ..Host::default()
        };
        let t0 = Instant::now();
        // A frame that re-rendered without placing the pane: still shown.
        h.frame_at(true, true, t0);
        assert!(h.shown);
        // Placed again next frame: nothing was sent.
        h.seen = true;
        h.frame_at(true, true, t0 + std::time::Duration::from_millis(16));
        assert!(h.shown && h.away_since.is_none());
        // Focus passes to the page before it reports focus: still shown.
        h.seen = true;
        h.frame_at(true, false, t0 + std::time::Duration::from_millis(32));
        assert!(h.shown);
        assert_eq!(h.hides, 0);
        // Really away: hidden once the grace (from the first miss) is up.
        h.frame_at(true, false, t0 + std::time::Duration::from_millis(200));
        assert!(h.shown, "the grace starts at the first miss");
        h.frame_at(true, false, t0 + std::time::Duration::from_millis(300));
        assert!(!h.shown);
        assert_eq!(h.hides, 1);
        // Not showing hides at once.
        h.shown = true;
        h.frame_at(false, true, t0 + std::time::Duration::from_millis(720));
        assert!(!h.shown);
    }

    #[test]
    fn a_host_without_a_helper_keeps_what_to_show() {
        let mut h = Host::default();
        h.go("https://example.com/");
        assert_eq!(h.want.as_deref(), Some("https://example.com/"));
        h.hide();
        assert!(!h.shown);
        assert_eq!(h.take_reports(), Reported::default());
        assert!(!h.is_live());
    }

    #[test]
    fn a_helper_that_never_reports_ready_times_out_to_stopped() {
        let mut h = Host::default();
        let t0 = Instant::now();
        h.status = Status::Starting(t0);
        h.pump(t0 + Duration::from_secs(1));
        assert!(h.is_starting());
        h.pump(t0 + START_TIMEOUT);
        assert!(h.stopped().is_some_and(|w| w.contains("did not start")));
    }

    #[test]
    fn ready_then_events_then_exit() {
        let mut h = Host {
            status: Status::Starting(Instant::now()),
            ..Host::default()
        };
        h.on_incoming(Incoming::Ev(Event::Ready {
            version: 1,
            engine: "WebView2".into(),
            cannot_dock: false,
        }));
        assert!(h.is_live());
        h.on_incoming(Incoming::Ev(Event::Url {
            url: "https://a.b/".into(),
        }));
        h.on_incoming(Incoming::Ev(Event::Title { title: "A".into() }));
        assert_eq!(
            h.take_reports(),
            Reported {
                url: Some("https://a.b/".into()),
                title: Some("A".into())
            }
        );
        h.on_incoming(Incoming::Gone);
        assert_eq!(h.stopped(), Some("the page engine stopped"));
        assert!(!h.is_live());
    }

    #[test]
    fn wayland_helpers_show_the_page_detached() {
        let mut h = Host::default();
        h.on_incoming(Incoming::Ev(Event::Ready {
            version: 1,
            engine: "WebKitGTK".into(),
            cannot_dock: true,
        }));
        assert!(h.detached);
        h.set_detached(false);
        assert!(h.detached, "Wayland cannot dock");
        h.on_incoming(Incoming::Ev(Event::Closed));
        assert!(h.detached);
    }

    #[test]
    fn a_start_error_stops_but_a_later_error_does_not() {
        let mut h = Host {
            status: Status::Starting(Instant::now()),
            ..Host::default()
        };
        h.on_incoming(Incoming::Ev(Event::Error {
            message: "no WebView2 runtime".into(),
        }));
        assert_eq!(h.stopped(), Some("no WebView2 runtime"));
        h.status = Status::Ready("WKWebView".into());
        h.on_incoming(Incoming::Ev(Event::Error {
            message: "bad url".into(),
        }));
        assert!(h.is_live());
    }

    #[test]
    fn placing_with_no_helper_binary_stops_with_a_reason_and_never_panics() {
        // SAFETY-free: point the lookup at a path that does not exist.
        let mut h = Host {
            status: Status::Stopped("x".into()),
            ..Host::default()
        };
        h.place(Rect::new(0., 0., 10., 10.), (0., 0.));
        assert!(h.seen);
        assert!(!h.shown);
    }
}
