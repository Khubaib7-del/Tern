//! The composer's `/` menu (Tern's commands, skills, and MCP prompts),
//! the argument form for a prompt or a resource template, and the banner
//! over composer text an MCP server wrote. Every menu row starts with a
//! logo: the tern mark for Tern's own, the server's logo for a server's.

use gpui_kit::assets::IconName as AssetIcon;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, Context, Hsla, SharedString, Window, div, px, svg,
};

use super::Workspace;
use super::press::Capsule as _;
use crate::assets;
use crate::commands::{FormFor, RowLogo, TernCommand};
use crate::extensions::ServerGlyph;
use crate::model::SlashChosen;
use crate::motion;
use crate::theme::Palette;

/// A row's logo in a small square tile.
pub(super) fn row_logo(logo: RowLogo, p: Palette) -> AnyElement {
    let icon = |name: AssetIcon, color: Hsla| {
        gpui_kit::component::Icon::new(name)
            .small()
            .text_color(color)
            .into_any_element()
    };
    let inner = match logo {
        RowLogo::Tern => svg()
            .path(assets::MARK)
            .size(px(13.))
            .text_color(p.text)
            .into_any_element(),
        RowLogo::Server(ServerGlyph::Service(k)) => super::service_logo(k, 13., p, p.text),
        RowLogo::Server(ServerGlyph::Terminal) => icon(AssetIcon::SquareTerminal, p.text),
        RowLogo::Server(ServerGlyph::Globe) => icon(AssetIcon::Globe, p.text),
    };
    div()
        .size(px(22.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .border_1()
        .border_color(p.border)
        .bg(p.raised)
        .child(inner)
        .into_any_element()
}

impl Workspace {
    /// Picks a `/` row (the highlighted one for `None`).
    pub(super) fn choose_slash(
        &mut self,
        index: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(chosen) = self.model.choose_slash(index) else {
            cx.notify();
            return;
        };
        let set = |this: &mut Self, text: String, window: &mut Window, cx: &mut Context<Self>| {
            this.composer.update(cx, |c, cx| {
                crate::view::set_end(c, text, window, cx);
                c.focus(window, cx);
            });
        };
        match chosen {
            SlashChosen::Command(c) => {
                set(self, String::new(), window, cx);
                self.run_command(c, window, cx);
            }
            SlashChosen::Text(t) => set(self, t, window, cx),
            SlashChosen::Prompt(server, name, args) => {
                set(self, String::new(), window, cx);
                self.model.notice_line(format!(
                    "Asking {server} for its “{name}” prompt. Its text goes in the composer for you to read first."
                ));
                self.bridge.mcp_prompt(server, name, args);
            }
            SlashChosen::Form => {
                set(self, String::new(), window, cx);
                self.open_arg_form(window, cx);
            }
        }
        cx.notify();
    }

    /// Runs one of Tern's own commands.
    fn run_command(&mut self, c: TernCommand, window: &mut Window, cx: &mut Context<Self>) {
        match c {
            TernCommand::New => self.new_chat(window, cx),
            TernCommand::Deep => self.model.deep_mode = !self.model.deep_mode,
            TernCommand::Library => self.open_library(cx),
            TernCommand::Editor => self.open_editor(window, cx),
            TernCommand::Git => self.open_git(window, cx),
            TernCommand::Browser => self.toggle_browser(window, cx),
            TernCommand::Customize => self.open_customize(cx),
            TernCommand::Instructions => {
                self.open_customize(cx);
                self.set_customize_tab(crate::extensions::CustomizeTab::Instructions, cx);
            }
            TernCommand::Providers => self.open_providers(cx),
            TernCommand::Appearance => self.open_appearance(cx),
            TernCommand::Decision => self.open_decision(cx),
        }
    }

    /// Up, Down, Tab, and Escape drive the `/` menu while it is open.
    /// Returns true when the key was the menu's.
    pub(super) fn slash_keys(
        &mut self,
        e: &gpui_kit::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.model.slash.is_none() {
            return false;
        }
        match e.keystroke.key.as_str() {
            "up" => self.model.move_slash(-1),
            "down" => self.model.move_slash(1),
            "tab" => {
                cx.stop_propagation();
                self.choose_slash(None, window, cx);
                return true;
            }
            "escape" => self.model.slash = None,
            _ => return false,
        }
        cx.stop_propagation();
        cx.notify();
        true
    }

    /// The `/` menu over the composer.
    pub(super) fn slash_menu(&self, p: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.model.slash.as_ref()?;
        let rows = self.model.slash_rows();
        if rows.is_empty() {
            return None;
        }
        let selected = menu.selected;
        Some(
            div()
                .id("slash-menu")
                .flex()
                .flex_col()
                .gap(px(2.))
                .p_1()
                .rounded_lg()
                .border_1()
                .border_color(p.border)
                .bg(p.canvas)
                .child(
                    div()
                        .px_2()
                        .pt_1()
                        .text_xs()
                        .text_color(p.faint)
                        .child("Commands, skills, and MCP prompts"),
                )
                .children(rows.into_iter().enumerate().map(|(i, row)| {
                    div()
                        .id(("slash", i))
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .text_sm()
                        .cursor_pointer()
                        .when(i == selected, |d| d.bg(p.hover))
                        .hover(move |d| d.bg(p.hover))
                        .child(row_logo(row.logo, p))
                        .child(
                            div()
                                .flex_none()
                                .text_color(p.text)
                                .child(SharedString::from(row.label)),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(p.muted)
                                .child(SharedString::from(row.description)),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_xs()
                                .text_color(p.faint)
                                .child(SharedString::from(row.source)),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.choose_slash(Some(i), window, cx);
                        }))
                }))
                .with_animation(
                    "slash-menu-in",
                    Animation::new(if self.reduced_motion {
                        std::time::Duration::ZERO
                    } else {
                        motion::UI
                    })
                    .with_easing(motion::ease),
                    |d, t| d.opacity(t),
                )
                .into_any_element(),
        )
    }

    /// Opens the argument form for the prompt or template in
    /// `model.arg_form`: one field per argument.
    pub(super) fn open_arg_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = self.model.arg_form.clone() else {
            return;
        };
        self.arg_inputs.clear();
        self.arg_subs.clear();
        let n = form.item.args.len();
        for (i, a) in form.item.args.iter().enumerate() {
            let mut hint = a.title.clone().unwrap_or_else(|| a.name.clone());
            if !a.description.trim().is_empty() {
                hint.push_str(&format!(" — {}", a.description.trim()));
            }
            if !a.required {
                hint.push_str(" (optional)");
            }
            let input = cx.new(|cx| InputState::new(window, cx).placeholder(hint));
            let last = i + 1 == n;
            let sub = cx.subscribe_in(
                &input,
                window,
                move |this: &mut Self, _, e: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { .. } = e {
                        if last {
                            this.submit_arg_form(window, cx);
                        } else if let Some(next) = this.arg_inputs.get(i + 1).cloned() {
                            next.update(cx, |s, cx| s.focus(window, cx));
                        }
                    }
                },
            );
            self.arg_inputs.push(input);
            self.arg_subs.push(sub);
        }
        if let Some(first) = self.arg_inputs.first().cloned() {
            first.update(cx, |s, cx| s.focus(window, cx));
        }
        cx.notify();
    }

    fn close_arg_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.model.arg_form = None;
        self.arg_inputs.clear();
        self.arg_subs.clear();
        self.composer.update(cx, |c, cx| c.focus(window, cx));
        cx.notify();
    }

    /// Sends the form: a prompt to its server (its text comes back for the
    /// composer), or a template's address to be read and attached.
    fn submit_arg_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = self.model.arg_form.clone() else {
            return;
        };
        let typed: Vec<String> = self
            .arg_inputs
            .iter()
            .map(|i| i.read(cx).value().to_string())
            .collect();
        let values = match form.values(&typed) {
            Ok(v) => v,
            Err(e) => {
                if let Some(f) = &mut self.model.arg_form {
                    f.error = Some(e);
                }
                cx.notify();
                return;
            }
        };
        let item = form.item;
        match form.purpose {
            FormFor::Prompt => {
                self.bridge
                    .mcp_prompt(item.server.clone(), item.name.clone(), values);
            }
            FormFor::Resource => match tern_agent::mcp::catalog::address(&item, &values) {
                Ok(uri) => {
                    self.model.attaching += 1;
                    self.bridge.mcp_resource(item.server.clone(), uri);
                }
                Err(e) => {
                    if let Some(f) = &mut self.model.arg_form {
                        f.error = Some(e);
                    }
                    cx.notify();
                    return;
                }
            },
        }
        self.close_arg_form(window, cx);
    }

    /// The argument form, inside the composer box.
    pub(super) fn arg_form_view(&self, p: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let form = self.model.arg_form.as_ref()?;
        let item = &form.item;
        let (title, go) = match form.purpose {
            FormFor::Prompt => (format!("/{}", item.name), "Get the prompt"),
            FormFor::Resource => (item.label().to_owned(), "Attach"),
        };
        let logo = crate::commands::server_logo(item);
        Some(
            div()
                .id("arg-form")
                .flex()
                .flex_col()
                .gap_2()
                .p_2()
                .rounded_lg()
                .border_1()
                .border_color(p.border)
                .bg(p.canvas)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(row_logo(logo, p))
                        .child(
                            div()
                                .text_sm()
                                .text_color(p.text)
                                .child(SharedString::from(title)),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(p.faint)
                                .child(SharedString::from(format!(
                                    "{} · {}",
                                    item.server,
                                    if item.uri.is_empty() {
                                        item.description.clone()
                                    } else {
                                        item.uri.clone()
                                    }
                                ))),
                        ),
                )
                .children(
                    self.arg_inputs
                        .iter()
                        .map(|i| Input::new(i).small().into_any_element()),
                )
                .children(form.error.clone().map(|e| {
                    div()
                        .text_xs()
                        .text_color(p.text)
                        .child(SharedString::from(e))
                }))
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("arg-cancel")
                                .capsule()
                                .label("Cancel")
                                .ghost()
                                .small()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.close_arg_form(window, cx)
                                })),
                        )
                        .child(Button::new("arg-go").capsule().label(go).small().on_click(
                            cx.listener(|this, _, window, cx| this.submit_arg_form(window, cx)),
                        )),
                )
                .into_any_element(),
        )
    }

    /// A prompt came back: its text replaces the composer, under the
    /// banner. Never sent from here.
    pub(super) fn prompt_ready(
        &mut self,
        r: Result<tern_agent::mcp::Inserted, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(text) = self.model.prompt_ready(r) {
            self.composer.update(cx, |c, cx| {
                crate::view::set_end(c, text, window, cx);
                c.focus(window, cx);
            });
            self.model.slash = None;
            self.model.mention = None;
        }
        cx.notify();
    }

    /// The banner over text a server wrote: its logo, whose text it is,
    /// and anything suspicious in it.
    pub(super) fn inserted_banner(&self, p: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let b = self.model.inserted.as_ref()?;
        Some(
            div()
                .id("inserted-banner")
                .flex()
                .flex_col()
                .gap_1()
                .px_2()
                .py_1()
                .rounded_lg()
                .border_1()
                .border_color(if b.inserted.flags.is_empty() {
                    p.border
                } else {
                    p.muted
                })
                .bg(p.canvas)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(row_logo(b.logo, p))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_xs()
                                .text_color(p.muted)
                                .child(SharedString::from(b.line())),
                        )
                        .child(
                            Button::new("inserted-dismiss")
                                .capsule()
                                .icon(AssetIcon::X)
                                .ghost()
                                .xsmall()
                                .tooltip("Hide this note (the text stays)")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.model.inserted = None;
                                    cx.notify();
                                })),
                        ),
                )
                .children(b.inserted.flags.iter().map(|f| {
                    div()
                        .pl(px(30.))
                        .text_xs()
                        .text_color(p.text)
                        .child(SharedString::from(format!(
                            "⚠ {}{}",
                            f.reason,
                            if f.excerpt.is_empty() {
                                String::new()
                            } else {
                                format!(": “{}”", f.excerpt)
                            }
                        )))
                }))
                .into_any_element(),
        )
    }

    /// The logo for an `@` resource row: its server's.
    pub(super) fn resource_logo(&self, item: &tern_agent::mcp::MenuItem, p: Palette) -> AnyElement {
        row_logo(crate::commands::server_logo(item), p)
    }
}
