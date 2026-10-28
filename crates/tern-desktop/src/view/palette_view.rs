//! Ctrl+K (Cmd+K on macOS): the command palette (v2 s18). A short list of
//! what the window can do, filtered as you type; Up/Down pick, Enter runs,
//! Esc closes. It reads keys itself, so it needs no text field of its own.

use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, KeyDownEvent, SharedString, Window, div, px};

use super::Workspace;
use crate::model::{RailTab, Screen};
use crate::theme::Palette;

/// One thing the palette can do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Cmd {
    NewChat,
    Research,
    Study,
    Sidebar,
    Rail,
    Tab(RailTab),
    Library,
    Browser,
    Editor,
    Git,
    Settings,
    Theme,
    Scout,
}

const ALL: [Cmd; 19] = [
    Cmd::NewChat,
    Cmd::Research,
    Cmd::Study,
    Cmd::Sidebar,
    Cmd::Rail,
    Cmd::Tab(RailTab::Plan),
    Cmd::Tab(RailTab::Sources),
    Cmd::Tab(RailTab::Report),
    Cmd::Tab(RailTab::Graph),
    Cmd::Tab(RailTab::Context),
    Cmd::Tab(RailTab::Files),
    Cmd::Tab(RailTab::Terminal),
    Cmd::Library,
    Cmd::Browser,
    Cmd::Editor,
    Cmd::Git,
    Cmd::Settings,
    Cmd::Theme,
    Cmd::Scout,
];

impl Cmd {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::NewChat => "New chat",
            Self::Research => "Switch to Research",
            Self::Study => "Switch to Study",
            Self::Sidebar => "Show or hide the chat list",
            Self::Rail => "Show or hide the right rail",
            Self::Tab(RailTab::Plan) => "Rail: Plan",
            Self::Tab(RailTab::Sources) => "Rail: Sources",
            Self::Tab(RailTab::Report) => "Rail: Report",
            Self::Tab(RailTab::Graph) => "Rail: Graph",
            Self::Tab(RailTab::Context) => "Rail: Context",
            Self::Tab(RailTab::Files) => "Rail: Files",
            Self::Tab(RailTab::Terminal) => "Rail: Terminal",
            Self::Library => "Open the library",
            Self::Browser => "Show or hide the browser",
            Self::Editor => "Open the editor",
            Self::Git => "Open Git",
            Self::Settings => "Open Settings",
            Self::Theme => "Switch light or dark theme",
            Self::Scout => "Show or hide Scout",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Self::Scout => "Ctrl+.",
            Self::Rail => "Ctrl+\\",
            _ => "",
        }
    }
}

/// The commands whose label holds every word of `query`, in order.
pub(super) fn matches(query: &str) -> Vec<Cmd> {
    let q = query.to_lowercase();
    let words: Vec<&str> = q.split_whitespace().collect();
    ALL.into_iter()
        .filter(|c| {
            let l = c.label().to_lowercase();
            words.iter().all(|w| l.contains(w))
        })
        .collect()
}

/// The palette's own state while it is open.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct CmdK {
    pub query: String,
    pub sel: usize,
}

/// Whether the keystroke opens the palette: Ctrl+K, or Cmd+K on macOS.
pub(super) fn is_open_key(key: &str, control: bool, platform: bool, alt: bool) -> bool {
    key == "k" && (control || platform) && !alt
}

impl Workspace {
    /// Opens or drives the palette. Returns whether the key was taken.
    pub(super) fn palette_keys(
        &mut self,
        e: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let k = &e.keystroke;
        let m = &k.modifiers;
        if is_open_key(&k.key, m.control, m.platform, m.alt) {
            self.cmdk = match self.cmdk {
                Some(_) => None,
                None => Some(CmdK::default()),
            };
            cx.stop_propagation();
            cx.notify();
            return true;
        }
        let Some(state) = self.cmdk.as_mut() else {
            return false;
        };
        let found = matches(&state.query);
        match k.key.as_str() {
            "escape" => self.cmdk = None,
            "up" => state.sel = state.sel.saturating_sub(1),
            "down" => state.sel = (state.sel + 1).min(found.len().saturating_sub(1)),
            "enter" => {
                if let Some(&c) = found.get(state.sel) {
                    self.cmdk = None;
                    self.run_cmd(c, window, cx);
                }
            }
            "backspace" => {
                state.query.pop();
                state.sel = 0;
            }
            "space" => {
                state.query.push(' ');
                state.sel = 0;
            }
            key if key.chars().count() == 1 && !m.control && !m.platform => {
                state.query.push_str(key);
                state.sel = 0;
            }
            _ => return false,
        }
        cx.stop_propagation();
        cx.notify();
        true
    }

    pub(super) fn run_cmd(&mut self, c: Cmd, window: &mut Window, cx: &mut Context<Self>) {
        match c {
            Cmd::NewChat => self.new_chat(window, cx),
            Cmd::Research => self.switch_harness(tern_agent::Harness::Research, None, cx),
            Cmd::Study => self.switch_harness(tern_agent::Harness::Study, None, cx),
            Cmd::Sidebar => self.model.sidebar_open = !self.model.sidebar_open,
            Cmd::Rail => self.model.run_panel_open = !self.model.run_panel_open,
            Cmd::Tab(t) => {
                let run = self.model.panel_run();
                if let Some(&v) = t.views(run).first() {
                    self.model.set_run_view(v);
                }
                self.model.browser.visible = false;
                self.model.run_panel_open = true;
            }
            Cmd::Library => {
                if self.model.screen != Screen::Library {
                    self.open_library(cx);
                }
            }
            Cmd::Browser => self.toggle_browser(window, cx),
            Cmd::Editor => {
                if self.model.screen != Screen::Editor {
                    self.open_editor(window, cx);
                }
            }
            Cmd::Git => {
                if self.model.screen != Screen::Git {
                    self.open_git(window, cx);
                }
            }
            Cmd::Settings => {
                if self.model.screen != Screen::Settings {
                    let s = self.model.settings.section;
                    self.open_settings(s, window, cx);
                }
            }
            Cmd::Theme => self.toggle_theme(window, cx),
            Cmd::Scout => self.set_scout_hidden(!self.scout_hidden, cx),
        }
        cx.notify();
    }

    /// The palette over the window, when open.
    pub(super) fn palette_overlay(&self, p: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let state = self.cmdk.as_ref()?;
        let found = matches(&state.query);
        let sel = state.sel.min(found.len().saturating_sub(1));
        let query: SharedString = if state.query.is_empty() {
            "Type a command…".into()
        } else {
            state.query.clone().into()
        };
        let rows = found.into_iter().enumerate().map(|(i, c)| {
            let on = i == sel;
            div()
                .id(SharedString::from(format!("cmdk-{i}")))
                .flex()
                .items_center()
                .justify_between()
                .px_3()
                .py(px(6.))
                .rounded_md()
                .text_sm()
                .cursor_pointer()
                .text_color(if on { p.text } else { p.muted })
                .when(on, |d| d.bg(p.hover))
                .hover(|d| d.bg(p.hover).text_color(p.text))
                .child(c.label())
                .child(div().text_xs().text_color(p.faint).child(c.hint()))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.cmdk = None;
                    this.run_cmd(c, window, cx);
                }))
        });
        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .flex_col()
                .items_center()
                .pt(px(96.))
                .bg(gpui_kit::hsla(0., 0., 0., 0.25))
                .occlude()
                .child(
                    div()
                        .id("cmdk")
                        .w(px(520.))
                        .max_w(px(560.))
                        .p_2()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .rounded_xl()
                        .border_1()
                        .border_color(p.border)
                        .bg(p.canvas)
                        .shadow_lg()
                        .child(
                            div()
                                .px_3()
                                .py_2()
                                .text_sm()
                                .border_b_1()
                                .border_color(p.border)
                                .text_color(if state.query.is_empty() {
                                    p.faint
                                } else {
                                    p.text
                                })
                                .child(query),
                        )
                        .children(rows)
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.cmdk = None;
                            cx.notify();
                        })),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_filters_by_every_word() {
        assert_eq!(matches("").len(), ALL.len());
        assert_eq!(matches("rail graph"), vec![Cmd::Tab(RailTab::Graph)]);
        assert!(matches("new").contains(&Cmd::NewChat));
        assert!(matches("zzz").is_empty());
    }

    #[test]
    fn ctrl_or_cmd_k_opens_it() {
        assert!(is_open_key("k", true, false, false));
        assert!(is_open_key("k", false, true, false));
        assert!(!is_open_key("k", false, false, false));
        assert!(!is_open_key("j", true, false, false));
    }
}
