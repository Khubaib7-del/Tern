//! App-level actions, their keys, and the macOS menu bar.
//!
//! | Action | macOS | Linux | Windows |
//! | --- | --- | --- | --- |
//! | Quit | Cmd+Q (Tern menu) | Ctrl+Q | Alt+F4 (the system's) |
//! | Close window | Cmd+W | Window manager | Alt+F4 |
//! | Minimize | Cmd+M | Window manager | Window manager |
//! | Hide Tern / Hide others / Show all | Cmd+H / Option+Cmd+H / menu | — | — |
//!
//! Text fields take the platform's own keys from the kit (Cmd on macOS,
//! Ctrl elsewhere). Ctrl+W is left alone on Linux and Windows, where people
//! expect it to close a tab, not the window.

use gpui_kit::{App, KeyBinding, Menu, MenuItem, actions};

actions!(
    tern,
    [Quit, CloseWindow, Minimize, Hide, HideOthers, ShowAll, Zoom]
);

/// The key bindings for this OS, as `(keystroke, action name)` pairs, so
/// tests can read them without a running app.
pub fn bindings_for(os: &str) -> Vec<(&'static str, &'static str)> {
    match os {
        "macos" => vec![
            ("cmd-q", "Quit"),
            ("cmd-w", "CloseWindow"),
            ("cmd-m", "Minimize"),
            ("cmd-h", "Hide"),
            ("alt-cmd-h", "HideOthers"),
        ],
        "linux" | "freebsd" => vec![("ctrl-q", "Quit")],
        _ => Vec::new(),
    }
}

fn binding(key: &str, name: &str) -> Option<KeyBinding> {
    Some(match name {
        "Quit" => KeyBinding::new(key, Quit, None),
        "CloseWindow" => KeyBinding::new(key, CloseWindow, None),
        "Minimize" => KeyBinding::new(key, Minimize, None),
        "Hide" => KeyBinding::new(key, Hide, None),
        "HideOthers" => KeyBinding::new(key, HideOthers, None),
        _ => return None,
    })
}

/// Registers the actions, binds this OS's keys, and sets the menu bar (only
/// macOS shows one).
pub fn install(cx: &mut App) {
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    cx.on_action(|_: &CloseWindow, cx| {
        if let Some(w) = cx.active_window() {
            let _ = w.update(cx, |_, window, _| window.remove_window());
        }
    });
    cx.on_action(|_: &Minimize, cx| {
        if let Some(w) = cx.active_window() {
            let _ = w.update(cx, |_, window, _| window.minimize_window());
        }
    });
    cx.on_action(|_: &Zoom, cx| {
        if let Some(w) = cx.active_window() {
            let _ = w.update(cx, |_, window, _| window.zoom_window());
        }
    });
    cx.bind_keys(
        bindings_for(std::env::consts::OS)
            .into_iter()
            .filter_map(|(key, name)| binding(key, name)),
    );
    cx.set_menus([
        Menu::new("Tern").items([
            MenuItem::action("Hide Tern", Hide),
            MenuItem::action("Hide Others", HideOthers),
            MenuItem::action("Show All", ShowAll),
            MenuItem::separator(),
            MenuItem::action("Quit Tern", Quit),
        ]),
        Menu::new("Window").items([
            MenuItem::action("Minimize", Minimize),
            MenuItem::action("Zoom", Zoom),
            MenuItem::separator(),
            MenuItem::action("Close Window", CloseWindow),
        ]),
    ]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macos_uses_cmd_and_others_do_not() {
        let mac = bindings_for("macos");
        assert!(mac.iter().all(|(k, _)| k.contains("cmd-")));
        assert!(mac.contains(&("cmd-q", "Quit")));
        for os in ["linux", "windows"] {
            assert!(bindings_for(os).iter().all(|(k, _)| !k.contains("cmd")));
            // Ctrl+W stays free for tabs.
            assert!(!bindings_for(os).iter().any(|(k, _)| *k == "ctrl-w"));
        }
        for os in ["macos", "linux"] {
            for (k, name) in bindings_for(os) {
                assert!(binding(k, name).is_some(), "{k} {name}");
            }
        }
    }
}
