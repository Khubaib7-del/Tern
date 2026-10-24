//! The Tern desktop app (GPUI).
//!
//! First slice of roadmap phase 6: the window shell (title bar with the
//! harness switch, icon rail, chat list, transcript, composer, run panel)
//! driving the same `ChatSession` as the TUI. Colours stay neutral grey until
//! the palette decision (ADR 0002) is made.

// Runs on Windows (DirectX), Linux (X11 and Wayland, Vulkan through wgpu),
// and macOS (Metal); see docs/linux.md and docs/macos.md.

// Hide the console window on Windows release builds.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app_menu;
mod appearance;
mod assets;
mod brands;
mod bridge;
mod browser;
mod chatflow;
mod commands;
mod composer;
mod continuity;
mod crashlog;
mod decision;
mod dictation;
mod editor;
mod evaluation;
mod explain;
mod extensions;
mod gauge;
mod git;
mod harness;
mod layout;
mod model;
mod motion;
mod onboarding;
mod panels;
mod providers;
mod runmap;
mod scout;
mod scrollbar;
mod settings;
mod study;
mod theme;
mod timeline;
mod toast;
mod view;
mod webview;

use gpui_kit::component::TitleBar;
use gpui_kit::{AppContext as _, Bounds, WindowBounds, WindowOptions, px, size};

use bridge::Bridge;

/// The window's application id (Wayland app id, X11 class).
pub(crate) const APP_ID: &str = "io.github.khubaib7_del.Tern";
use view::Workspace;

fn main() {
    // Every panic is written to <data>/logs/crash.log first.
    crashlog::install();
    // On Windows release builds this process only watches the app, run as
    // a child, so a crash no panic hook sees (a stack overflow, an abort,
    // a driver fault) still leaves a line in crash.log.
    if let Some(code) = crashlog::watch() {
        std::process::exit(code);
    }
    let (bridge, rx) = match Bridge::new() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("Tern could not start its background runtime: {e}");
            std::process::exit(1);
        }
    };
    // The kit's Lucide icons are embedded assets; without a source every
    // icon renders empty. `Assets` holds only the components' default set,
    // so icons outside it (Pencil, Archive, the run panel's) need the full
    // catalog, behind Tern's own mark.
    gpui_kit::application()
        .with_assets(assets::TernAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            app_menu::install(cx);
            assets::register_fonts(cx);
            let look = appearance::Appearance::from_prefs(&tern_agent::Preferences::load());
            // The window's own light or dark setting is read once it opens;
            // until then System starts dark.
            theme::apply_kit(look.is_dark(true), &look, None, cx);
            // Closing the window ends the app on every platform.
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            let bounds = Bounds::centered(None, size(px(1280.), px(820.)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(720.), px(480.))),
                // Matches the `.desktop` file's StartupWMClass on Linux, so
                // the dock shows Tern's icon for this window.
                app_id: Some(APP_ID.into()),
                ..TitleBar::window_options()
            };
            let opened = gpui_kit::open_window(options, cx, move |window, cx| {
                cx.new(|cx| Workspace::new(bridge, rx, window, cx))
            });
            if let Err(e) = opened {
                eprintln!("Tern could not open its window: {e}");
                cx.quit();
            }
            cx.activate(true);
        });
}
