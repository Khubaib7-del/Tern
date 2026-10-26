//! `tern-browser`: the system web view (WebView2 on Windows, WKWebView on
//! macOS, WebKitGTK on Linux) in its own process, with its own event loop
//! (tao), so creating or running a page can never block or crash Tern's
//! window (`docs/decisions/0011-browser-helper-process.md`).
//!
//! Two ways to run it:
//!
//! - **Driven** (the desktop app): commands arrive as JSON lines on stdin
//!   and events leave on stdout (`tern_core::browser_ipc`). The window
//!   starts hidden and borderless and is docked over the app's page pane
//!   by `bounds` commands. When stdin closes (the app quit or crashed),
//!   the helper exits.
//! - **Standalone** (`tern browse --open`, or `--standalone`): an ordinary
//!   window on the given address; closing it exits.
//!
//! ```text
//! tern-browser [--standalone] [--url <address>] [--data-dir <dir>]
//!              [--owner <window handle>] [--title <text>]
//! ```

use std::io::{BufRead as _, Write as _};
use std::time::{Duration, Instant};

use tao::dpi::{LogicalPosition, LogicalSize};
use tao::event::{Event as TaoEvent, StartCause, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use tao::window::{Window, WindowBuilder};
use tern_core::browser_ipc::{Command, Event, VERSION};
use wry::{WebView, WebViewBuilder};

/// What the event loop wakes up for besides window events.
#[derive(Debug)]
enum Wake {
    Cmd(Command),
    /// Stdin closed: the app is gone.
    Orphaned,
    /// The page reported something (sent from wry's handlers).
    Out(Event),
    /// A real page (not `about:blank`) finished loading.
    Painted,
}

/// How long a page that is still loading may stay hidden after the app
/// asked to show it.
const REVEAL_MAX: Duration = Duration::from_millis(2500);

/// Whether the docked window shows when the app asks (#68). A page that
/// has not finished loading is a white rectangle, so it stays hidden,
/// with Tern's own reading of the page under it, until it has loaded or
/// for at most [`REVEAL_MAX`].
#[derive(Debug, Default)]
struct Reveal {
    loaded: bool,
    asked: Option<Instant>,
}

impl Reveal {
    /// The app asked to show the page: whether to show it now.
    fn show(&mut self, now: Instant) -> bool {
        if self.loaded {
            self.asked = None;
            true
        } else {
            self.asked.get_or_insert(now);
            false
        }
    }

    fn hide(&mut self) {
        self.asked = None;
    }

    /// A new address starts loading: a later show waits for it.
    fn navigate(&mut self) {
        self.loaded = false;
    }

    /// The page finished loading: whether a waiting show goes through.
    fn loaded(&mut self) -> bool {
        self.loaded = true;
        self.asked.take().is_some()
    }

    /// When the loop should wake to show a page that is taking long.
    fn deadline(&self) -> Option<Instant> {
        self.asked.map(|t| t + REVEAL_MAX)
    }

    /// The loop woke at `now`: whether a waiting show is due.
    fn due(&mut self, now: Instant) -> bool {
        match self.asked {
            Some(t) if now >= t + REVEAL_MAX => {
                self.asked = None;
                true
            }
            _ => false,
        }
    }
}

#[derive(Debug, Default)]
struct Args {
    standalone: bool,
    url: Option<String>,
    data_dir: Option<std::path::PathBuf>,
    owner: Option<isize>,
    title: Option<String>,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args::default();
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--standalone" => a.standalone = true,
            "--url" => a.url = it.next(),
            "--data-dir" => a.data_dir = it.next().map(Into::into),
            "--owner" => a.owner = it.next().and_then(|v| v.parse().ok()),
            "--title" => a.title = it.next(),
            "-h" | "--help" => {
                println!(
                    "tern-browser: Tern's browser helper.\n\nUsage: tern-browser [--standalone] [--url <address>] [--data-dir <dir>] [--owner <handle>] [--title <text>]\n\nWithout --standalone it takes JSON-line commands on stdin (see tern_core::browser_ipc)."
                );
                std::process::exit(0);
            }
            other if !other.starts_with('-') && a.url.is_none() => a.url = Some(other.to_owned()),
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(a)
}

/// Writes one event to stdout. A closed stdout (the app is gone) is not an
/// error worth reporting; the stdin reader ends the helper soon after.
fn emit(e: &Event) {
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(e.to_line().as_bytes());
    let _ = out.flush();
}

fn engine_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "WebView2"
    } else if cfg!(target_os = "macos") {
        "WKWebView"
    } else {
        "WebKitGTK"
    }
}

/// Wayland does not let a client place its own window, so docking is out
/// there; the app shows the page as an attached window instead.
fn cannot_dock() -> bool {
    cfg!(all(unix, not(target_os = "macos")))
        && std::env::var_os("WAYLAND_DISPLAY").is_some()
        && std::env::var("GDK_BACKEND").map_or(true, |b| !b.contains("x11"))
}

fn build_window(
    target: &tao::event_loop::EventLoopWindowTarget<Wake>,
    args: &Args,
) -> Result<Window, String> {
    let title = args.title.clone().unwrap_or_else(|| "Tern browser".into());
    let docked = !args.standalone && !cannot_dock();
    let b = WindowBuilder::new()
        .with_title(title)
        .with_visible(args.standalone)
        .with_decorations(!docked)
        .with_inner_size(LogicalSize::new(1100., 800.));
    #[cfg(target_os = "windows")]
    let b = {
        use tao::platform::windows::{IconExtWindows as _, WindowBuilderExtWindows as _};
        // The Tern mark embedded by build.rs (resource 1), for a
        // standalone window's title bar and taskbar button.
        let icon = tao::window::Icon::from_resource(1, None).ok();
        let b = b
            .with_window_icon(icon.clone())
            .with_taskbar_icon(icon)
            .with_skip_taskbar(docked);
        // An owned window stays above Tern's, minimises with it, and
        // never takes a taskbar button of its own.
        match args.owner {
            Some(h) if docked => b.with_owner_window(h),
            _ => b,
        }
    };
    #[cfg(not(target_os = "windows"))]
    let _ = args.owner;
    b.build(target).map_err(|e| e.to_string())
}

fn build_view(
    window: &Window,
    args: &Args,
    proxy: &EventLoopProxy<Wake>,
    context: &mut wry::WebContext,
) -> Result<WebView, String> {
    let url = args.url.clone().unwrap_or_else(|| "about:blank".into());
    let loads = proxy.clone();
    let titles = proxy.clone();
    let b = WebViewBuilder::new_with_web_context(context)
        .with_url(url)
        .with_on_page_load_handler(move |e, url| {
            let loading = matches!(e, wry::PageLoadEvent::Started);
            let _ = loads.send_event(Wake::Out(Event::Loading { loading }));
            if !loading {
                if !url.starts_with("about:") {
                    let _ = loads.send_event(Wake::Painted);
                }
                let _ = loads.send_event(Wake::Out(Event::Url { url }));
            }
        })
        .with_document_title_changed_handler(move |title| {
            let _ = titles.send_event(Wake::Out(Event::Title { title }));
        });
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    let built = b.build(window);
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let built = {
        use tao::platform::unix::WindowExtUnix as _;
        use wry::WebViewBuilderExtUnix as _;
        match window.default_vbox() {
            Some(vbox) => b.build_gtk(vbox),
            None => return Err("the window has no GTK box for the page".into()),
        }
    };
    built.map_err(|e| e.to_string())
}

/// Reads commands from stdin on a thread and hands them to the loop.
fn read_stdin(proxy: EventLoopProxy<Wake>) {
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            let Ok(line) = line else { break };
            if let Some(cmd) = Command::parse(&line)
                && proxy.send_event(Wake::Cmd(cmd)).is_err()
            {
                return;
            }
        }
        let _ = proxy.send_event(Wake::Orphaned);
    });
}

/// Applies one command. Returns `false` to exit.
fn apply(
    cmd: Command,
    window: &Window,
    view: &WebView,
    detached: &mut bool,
    reveal: &mut Reveal,
) -> bool {
    match cmd {
        Command::Navigate { url } => {
            reveal.navigate();
            if let Err(e) = view.load_url(&url) {
                emit(&Event::Error {
                    message: e.to_string(),
                });
            }
        }
        Command::Back => {
            let _ = view.evaluate_script("history.back()");
        }
        Command::Forward => {
            let _ = view.evaluate_script("history.forward()");
        }
        Command::Reload => {
            let _ = view.reload();
        }
        Command::Bounds { rect } => {
            if !*detached {
                window.set_outer_position(LogicalPosition::new(rect.x, rect.y));
                window.set_inner_size(LogicalSize::new(rect.w.max(1.), rect.h.max(1.)));
            }
        }
        Command::Show => {
            if *detached || reveal.show(Instant::now()) {
                window.set_visible(true);
            }
        }
        Command::Hide => {
            reveal.hide();
            window.set_visible(false);
        }
        Command::Detach { detached: d } => {
            *detached = d;
            window.set_decorations(d);
            #[cfg(target_os = "windows")]
            {
                use tao::platform::windows::WindowExtWindows as _;
                let _ = window.set_skip_taskbar(!d);
            }
            if d {
                window.set_always_on_top(false);
                window.set_inner_size(LogicalSize::new(1100., 800.));
                window.set_visible(true);
                window.set_focus();
            }
        }
        Command::OnTop { on_top } => {
            // Windows keeps an owned window above its owner already.
            if !cfg!(target_os = "windows") && !*detached {
                window.set_always_on_top(on_top);
            }
        }
        Command::Quit => return false,
    }
    true
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("tern-browser: {e}");
            std::process::exit(2);
        }
    };
    let event_loop = EventLoopBuilder::<Wake>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    let window = match build_window(&event_loop, &args) {
        Ok(w) => w,
        Err(e) => {
            emit(&Event::Error {
                message: format!("Could not open the page window: {e}"),
            });
            std::process::exit(1);
        }
    };
    let mut context = wry::WebContext::new(args.data_dir.clone());
    let view = match build_view(&window, &args, &proxy, &mut context) {
        Ok(v) => v,
        Err(e) => {
            emit(&Event::Error {
                message: format!("The {} web view did not start: {e}", engine_name()),
            });
            std::process::exit(1);
        }
    };
    let standalone = args.standalone;
    let mut detached = standalone || cannot_dock();
    if !standalone {
        read_stdin(proxy);
    }
    let base_title = args.title.clone().unwrap_or_else(|| "Tern browser".into());
    let mut reveal = Reveal::default();
    event_loop.run(move |event, _, flow| {
        *flow = ControlFlow::Wait;
        // Keep these alive for as long as the loop runs.
        let _ = &context;
        match event {
            TaoEvent::NewEvents(StartCause::Init) => {
                if !standalone {
                    emit(&Event::Ready {
                        version: VERSION,
                        engine: engine_name().into(),
                        cannot_dock: cannot_dock(),
                    });
                }
            }
            TaoEvent::NewEvents(StartCause::ResumeTimeReached { .. }) => {
                if reveal.due(Instant::now()) {
                    window.set_visible(true);
                }
            }
            TaoEvent::UserEvent(Wake::Cmd(cmd)) => {
                if !apply(cmd, &window, &view, &mut detached, &mut reveal) {
                    *flow = ControlFlow::Exit;
                }
            }
            TaoEvent::UserEvent(Wake::Painted) => {
                if reveal.loaded() {
                    window.set_visible(true);
                }
            }
            TaoEvent::UserEvent(Wake::Orphaned) => *flow = ControlFlow::Exit,
            TaoEvent::UserEvent(Wake::Out(e)) => {
                if let Event::Title { title } = &e
                    && detached
                    && !title.trim().is_empty()
                {
                    window.set_title(&format!("{title} · {base_title}"));
                }
                if !standalone {
                    emit(&e);
                }
            }
            TaoEvent::WindowEvent { event, .. } => match event {
                WindowEvent::CloseRequested => {
                    if standalone {
                        *flow = ControlFlow::Exit;
                    } else {
                        // The app decides; the page just goes away for now.
                        window.set_visible(false);
                        emit(&Event::Closed);
                    }
                }
                WindowEvent::Focused(focused) if !standalone => {
                    emit(&Event::Focus { focused });
                }
                _ => {}
            },
            _ => {}
        }
        // A show waiting for its page wakes the loop when it is due.
        if matches!(*flow, ControlFlow::Wait)
            && let Some(at) = reveal.deadline()
        {
            *flow = ControlFlow::WaitUntil(at);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_still_loading_shows_once_it_has_loaded() {
        let now = Instant::now();
        let mut r = Reveal::default();
        r.navigate();
        assert!(!r.show(now), "white while loading: wait");
        assert_eq!(r.deadline(), Some(now + REVEAL_MAX));
        assert!(r.loaded(), "the waiting show goes through");
        assert_eq!(r.deadline(), None);
        assert!(r.show(now), "a loaded page shows at once");
    }

    #[test]
    fn a_slow_page_shows_after_the_wait_and_a_hide_cancels_it() {
        let now = Instant::now();
        let mut r = Reveal::default();
        assert!(!r.show(now));
        assert!(!r.due(now + REVEAL_MAX / 2));
        assert!(r.due(now + REVEAL_MAX));
        assert!(!r.loaded(), "nothing waits any more");
        let mut r = Reveal::default();
        assert!(!r.show(now));
        r.hide();
        assert!(!r.due(now + REVEAL_MAX));
        assert!(!r.loaded(), "a hidden pane does not pop up on load");
    }
}
