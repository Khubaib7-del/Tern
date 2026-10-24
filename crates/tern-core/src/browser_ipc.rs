//! The line protocol between a Tern front end and the browser helper
//! (`tern-browser`), the small process that owns the system web view
//! (WebView2 on Windows, WKWebView on macOS, WebKitGTK on Linux) with its
//! own event loop (`docs/decisions/0011-browser-helper-process.md`).
//!
//! Each message is one JSON object on one line: commands go to the
//! helper's stdin, events come back on its stdout. Anything that does not
//! parse is ignored by both sides, so an older helper and a newer app (or
//! the reverse) degrade instead of failing.

use serde::{Deserialize, Serialize};

/// The protocol version the helper reports in [`Event::Ready`].
pub const VERSION: u32 = 1;

/// A rectangle in logical (DPI-independent) pixels on the screen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self { x, y, w, h }
    }

    /// A rectangle inside a window, placed on the screen: `origin` is the
    /// window's content origin on the screen. Sizes are kept at least
    /// one pixel so a native window never gets a zero size.
    pub fn on_screen(origin: (f64, f64), inner: Rect) -> Self {
        Self {
            x: origin.0 + inner.x,
            y: origin.1 + inner.y,
            w: inner.w.max(1.),
            h: inner.h.max(1.),
        }
    }

    /// Whether two rectangles differ by at least half a pixel anywhere
    /// (so tiny layout jitter does not flood the helper with moves).
    pub fn moved(&self, other: &Rect) -> bool {
        (self.x - other.x).abs() >= 0.5
            || (self.y - other.y).abs() >= 0.5
            || (self.w - other.w).abs() >= 0.5
            || (self.h - other.h).abs() >= 0.5
    }
}

/// From the front end to the helper.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    /// Load an address.
    Navigate { url: String },
    Back,
    Forward,
    Reload,
    /// Dock the page over this rectangle on the screen (logical px).
    Bounds { rect: Rect },
    /// Show the page window (docked, or attached when `detached`).
    Show,
    /// Hide it (the pane closed, a menu covers it, Tern lost focus).
    Hide,
    /// `true`: an ordinary window with a title bar beside Tern instead of
    /// docked over the pane (Wayland, or when the user pops it out).
    Detach { detached: bool },
    /// Keep the page above other apps' windows while Tern is in front.
    OnTop { on_top: bool },
    /// Close the page and exit.
    Quit,
}

/// From the helper to the front end.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "ev", rename_all = "snake_case")]
pub enum Event {
    /// The web view is up and takes commands.
    Ready {
        version: u32,
        /// The engine: "WebView2", "WKWebView", or "WebKitGTK".
        engine: String,
        /// The helper cannot place its window itself (Wayland): the front
        /// end should send `Detach { detached: true }`.
        #[serde(default)]
        cannot_dock: bool,
    },
    /// The page is at this address now (a link clicked inside it, a
    /// redirect, or a navigation the front end asked for).
    Url { url: String },
    Title { title: String },
    /// A page started (`true`) or finished (`false`) loading.
    Loading { loading: bool },
    /// The page window gained or lost the keyboard focus.
    Focus { focused: bool },
    /// The user closed the detached page window.
    Closed,
    /// Something failed; the helper keeps running when it can.
    Error { message: String },
}

impl Command {
    /// One line of JSON with its newline.
    pub fn to_line(&self) -> String {
        let mut s = serde_json::to_string(self).unwrap_or_default();
        s.push('\n');
        s
    }

    pub fn parse(line: &str) -> Option<Self> {
        serde_json::from_str(line.trim()).ok()
    }
}

impl Event {
    /// One line of JSON with its newline.
    pub fn to_line(&self) -> String {
        let mut s = serde_json::to_string(self).unwrap_or_default();
        s.push('\n');
        s
    }

    pub fn parse(line: &str) -> Option<Self> {
        serde_json::from_str(line.trim()).ok()
    }
}

/// The helper's file name on this platform.
pub fn helper_file_name() -> &'static str {
    if cfg!(windows) {
        "tern-browser.exe"
    } else {
        "tern-browser"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_and_events_round_trip_as_single_lines() {
        let cmds = [
            Command::Navigate {
                url: "https://example.com/".into(),
            },
            Command::Back,
            Command::Bounds {
                rect: Rect::new(10., 20., 300., 400.),
            },
            Command::Detach { detached: true },
            Command::Quit,
        ];
        for c in cmds {
            let line = c.to_line();
            assert!(line.ends_with('\n'));
            assert_eq!(line.matches('\n').count(), 1);
            assert_eq!(Command::parse(&line), Some(c));
        }
        let evs = [
            Event::Ready {
                version: VERSION,
                engine: "WebView2".into(),
                cannot_dock: false,
            },
            Event::Url {
                url: "https://a.b/\nx".into(),
            },
            Event::Title { title: "T".into() },
            Event::Closed,
        ];
        for e in evs {
            let line = e.to_line();
            assert_eq!(line.matches('\n').count(), 1, "{line}");
            assert_eq!(Event::parse(&line), Some(e));
        }
    }

    #[test]
    fn unknown_or_broken_lines_are_ignored() {
        assert_eq!(Command::parse("{\"cmd\":\"teleport\"}"), None);
        assert_eq!(Event::parse("not json"), None);
        assert_eq!(Event::parse(""), None);
        // A Ready from an older helper without `cannot_dock` still parses.
        assert_eq!(
            Event::parse("{\"ev\":\"ready\",\"version\":1,\"engine\":\"WKWebView\"}"),
            Some(Event::Ready {
                version: 1,
                engine: "WKWebView".into(),
                cannot_dock: false
            })
        );
    }

    #[test]
    fn rectangles_go_on_screen_and_ignore_jitter() {
        let r = Rect::on_screen((100., 50.), Rect::new(10., 20., 0., 30.));
        assert_eq!(r, Rect::new(110., 70., 1., 30.));
        assert!(!r.moved(&Rect::new(110.2, 70.1, 1., 30.)));
        assert!(r.moved(&Rect::new(111., 70., 1., 30.)));
    }
}
