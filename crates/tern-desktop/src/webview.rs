//! The real browser engine behind the built-in browser's page: the
//! system web view (WebView2 on Windows, WKWebView on macOS) embedded as
//! a native child of Tern's window through `wry`, placed over the browser
//! pane's page area and kept on its bounds every frame it is drawn.
//!
//! Linux has no engine in this build: wry needs WebKitGTK and a GTK main
//! loop beside GPUI's, so the page shows Tern's reader view there
//! (`docs/decisions/0010-browser-engine.md`).
//!
//! The engine is only touched on the main thread (GPUI's), which is the
//! thread every system web view needs. A native child view sits above
//! everything GPUI draws in its rectangle, so the pane hides it whenever
//! the page is not the thing showing (a menu, Explanation only, another
//! screen).

use std::cell::RefCell;
use std::rc::Rc;

/// Whether this build embeds a real engine.
pub const AVAILABLE: bool = cfg!(any(target_os = "windows", target_os = "macos"));

/// What the page reported since the last look: where it is now and its
/// title, for the address bar and the tab.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reported {
    pub url: Option<String>,
    pub title: Option<String>,
}

/// The engine host: one web view per Tern window, reused across tabs (a
/// tab switch loads that tab's address).
#[derive(Default)]
pub struct Host {
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    view: Option<wry::WebView>,
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    context: Option<wry::WebContext>,
    /// The address the page should show.
    want: Option<String>,
    /// The address last sent to the engine.
    #[cfg_attr(not(any(target_os = "windows", target_os = "macos")), allow(dead_code))]
    sent: Option<String>,
    /// The rectangle it was last placed on, logical px in the window.
    #[cfg_attr(not(any(target_os = "windows", target_os = "macos")), allow(dead_code))]
    bounds: Option<[f32; 4]>,
    shown: bool,
    /// The page element was placed since the last frame check.
    seen: bool,
    /// Events from the engine, read by the view on its next frame.
    reports: Rc<RefCell<Reported>>,
    /// Why the engine could not start, once.
    pub error: Option<String>,
    /// Where the engine keeps its profile (cookies, cache).
    #[cfg_attr(not(any(target_os = "windows", target_os = "macos")), allow(dead_code))]
    pub data_dir: Option<std::path::PathBuf>,
}

impl std::fmt::Debug for Host {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Host")
            .field("want", &self.want)
            .field("shown", &self.shown)
            .finish_non_exhaustive()
    }
}

/// Shared between the workspace and the page element's prepaint.
pub type Shared = Rc<RefCell<Host>>;

impl Host {
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
    }

    /// Takes what the engine reported since the last call.
    pub fn take_reports(&self) -> Reported {
        std::mem::take(&mut *self.reports.borrow_mut())
    }

    /// Reload acts on the engine itself (the address did not change).
    pub fn reload(&self) {
        self.script("location.reload()");
    }

    fn script(&self, _js: &str) {
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        if let Some(v) = &self.view {
            let _ = v.evaluate_script(_js);
        }
    }

    /// Once per window render: hide the page when it should not show, or
    /// when its element was not drawn last frame (the pane closed or the
    /// layout moved it away), so a native view never lingers over Tern.
    pub fn frame(&mut self, show: bool) {
        if !show || !self.seen {
            self.hide();
        }
        self.seen = false;
    }

    /// The page is not on screen this frame: hide the native view so it
    /// never covers Tern's own UI.
    pub fn hide(&mut self) {
        if !self.shown {
            return;
        }
        self.shown = false;
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        if let Some(v) = &self.view {
            let _ = v.set_visible(false);
        }
    }

    /// Puts the page on `bounds` (logical px in the window), starting
    /// the engine the first time. `window` is Tern's window, the parent.
    pub fn place<W: raw_window_handle::HasWindowHandle>(&mut self, bounds: [f32; 4], window: &W) {
        self.seen = true;
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        {
            use wry::dpi::{LogicalPosition, LogicalSize};
            let rect = wry::Rect {
                position: LogicalPosition::new(bounds[0] as f64, bounds[1] as f64).into(),
                size: LogicalSize::new(bounds[2].max(1.) as f64, bounds[3].max(1.) as f64).into(),
            };
            if self.view.is_none() && self.error.is_none() {
                let url = self.want.clone().unwrap_or_else(|| "about:blank".into());
                if self.context.is_none() {
                    self.context = Some(wry::WebContext::new(self.data_dir.clone()));
                }
                let reports = self.reports.clone();
                let titles = self.reports.clone();
                let built = match self.context.as_mut() {
                    Some(ctx) => wry::WebViewBuilder::new_with_web_context(ctx),
                    None => wry::WebViewBuilder::new(),
                }
                .with_url(url.clone())
                .with_bounds(rect)
                .with_on_page_load_handler(move |e, url| {
                    if matches!(e, wry::PageLoadEvent::Finished) {
                        reports.borrow_mut().url = Some(url);
                    }
                })
                .with_document_title_changed_handler(move |t| {
                    titles.borrow_mut().title = Some(t);
                })
                .build_as_child(window);
                match built {
                    Ok(v) => {
                        self.view = Some(v);
                        self.sent = Some(url);
                        self.bounds = Some(bounds);
                        self.shown = true;
                    }
                    Err(e) => self.error = Some(e.to_string()),
                }
                return;
            }
            let Some(v) = &self.view else {
                return;
            };
            if self.bounds != Some(bounds) {
                let _ = v.set_bounds(rect);
                self.bounds = Some(bounds);
            }
            if !self.shown {
                let _ = v.set_visible(true);
                self.shown = true;
            }
            if self.want.is_some() && self.want != self.sent {
                if let Some(u) = &self.want {
                    let _ = v.load_url(u);
                }
                self.sent = self.want.clone();
            }
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        {
            let _ = (bounds, window);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_host_without_a_window_keeps_what_to_show() {
        let mut h = Host::default();
        h.go("https://example.com/");
        assert_eq!(h.want.as_deref(), Some("https://example.com/"));
        h.hide();
        assert!(!h.shown);
        assert_eq!(h.take_reports(), Reported::default());
        assert_eq!(
            AVAILABLE,
            cfg!(any(target_os = "windows", target_os = "macos"))
        );
    }
}
