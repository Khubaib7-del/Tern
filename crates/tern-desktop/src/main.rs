//! The Tern desktop app (GPUI).
//!
//! First slice of roadmap phase 6: the window shell (title bar with the
//! harness switch, icon rail, chat list, transcript, composer, run panel)
//! driving the same `ChatSession` as the TUI. Colours stay neutral grey until
//! the palette decision (ADR 0002) is made.

// Hide the console window on Windows release builds.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod assets;
mod bridge;
mod browser;
mod commands;
mod continuity;
mod decision;
mod editor;
mod evaluation;
mod explain;
mod extensions;
mod git;
mod layout;
mod model;
mod motion;
mod providers;
mod scout;
mod study;
mod theme;
mod view;

use gpui_kit::component::TitleBar;
use gpui_kit::{AppContext as _, Bounds, WindowBounds, WindowOptions, px, size};

use bridge::Bridge;
use view::Workspace;

fn main() {
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
            assets::register_fonts(cx);
            theme::apply_kit(true, None, cx);
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
