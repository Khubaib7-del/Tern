//! The composer row and its menus (+, mode, model, effort, search), the
//! Thinking and working sections inside answers, grouped message actions,
//! hover times, the workspace chip, stacked notices, and the Undo toast.
//! State lives in `crate::composer` and `crate::chatflow`.

use gpui_kit::assets::IconName as AssetIcon;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::{Disableable as _, IconName, Selectable as _, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnimationExt as _, AnyElement, Context, FontWeight, Hsla, SharedString, Window, div, px,
};
use tern_agent::{ChatSession, Effort, Harness, Run};

use super::Workspace;
use crate::chatflow::{self, ActionMenu, Notice, RollupView, UndoKind};
use crate::composer::{self, Menu, Mode, PlusItem};
use crate::providers::Kind;
use crate::theme::Palette;

/// A small icon in a colour.
fn icon(name: AssetIcon, color: Hsla) -> AnyElement {
    div()
        .flex_none()
        .text_color(color)
        .child(gpui_kit::component::Icon::new(name).xsmall())
        .into_any_element()
}

fn mode_icon(m: Mode) -> AssetIcon {
    match m {
        Mode::Chat => AssetIcon::MessageSquare,
        Mode::Plan => AssetIcon::ListChecks,
        Mode::Deep => AssetIcon::Telescope,
        Mode::Act => AssetIcon::Hammer,
        Mode::Tutor => AssetIcon::GraduationCap,
        Mode::Quiz => AssetIcon::ListTodo,
        Mode::Flashcards => AssetIcon::Layers,
        Mode::Viva => AssetIcon::MessagesSquare,
        Mode::Explain => AssetIcon::Lightbulb,
    }
}

fn plus_icon(i: PlusItem) -> AssetIcon {
    match i {
        PlusItem::Files => AssetIcon::FileText,
        PlusItem::Images => AssetIcon::Image,
        PlusItem::Pdfs => AssetIcon::FileType,
        PlusItem::Folder => AssetIcon::FolderArchive,
        PlusItem::Link => AssetIcon::Link,
        PlusItem::Resource => AssetIcon::AtSign,
        PlusItem::Paste => AssetIcon::ClipboardPaste,
    }
}

/// The effort meter: five rising square-ended bars on a baseline, `lit`
/// of them in the text colour. Upright bars read as a level, unlike the
/// round context ring beside it.
pub(super) fn meter(lit: u8, p: Palette) -> AnyElement {
    div()
        .flex_none()
        .flex()
        .items_end()
        .gap(px(1.))
        .h(px(12.))
        .pb(px(1.))
        .children((1..=5u8).map(|i| {
            div()
                .w(px(2.))
                .h(px(2. + f32::from(i) * 2.))
                .bg(if i <= lit {
                    p.text
                } else {
                    p.faint.opacity(0.45)
                })
        }))
        .into_any_element()
}

/// A greyscale 1-to-5 bar for the model hover card.
fn scale(label: &'static str, n: u8, p: Palette) -> AnyElement {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap_2()
        .text_xs()
        .child(div().text_color(p.muted).child(label))
        .child(div().flex().gap(px(2.)).children((1..=5u8).map(|i| {
            div()
                .w(px(14.))
                .h(px(4.))
                .rounded(px(2.))
                .bg(if i <= n { p.muted } else { p.border })
        })))
        .into_any_element()
}

impl Workspace {
    /// A chat came on screen: its remembered mode and its workspace.
    pub(super) fn chat_shown(&mut self, s: &ChatSession) {
        let id = (!s.is_draft()).then(|| s.session_id().to_string());
        self.model.composer.chat_opened(id.as_deref(), s.harness());
        self.model.deep_mode = self.model.composer.mode.is_deep();
        self.model.workspace = s.workspace().map(|w| (w.name(), w.path().to_path_buf()));
    }

    /// Saves a preference change both to disk and to the providers state
    /// (which a model swap saves too), so neither undoes the other.
    pub(super) fn save_ui_prefs(&mut self, f: impl Fn(&mut tern_agent::Preferences)) {
        let mut prefs = tern_agent::Preferences::load();
        f(&mut prefs);
        f(&mut self.model.providers.prefs);
        self.bridge.save_prefs(prefs);
    }

    /// Opens a composer menu, loading what it needs.
    fn open_menu(&mut self, menu: Menu, window: &mut Window, cx: &mut Context<Self>) {
        self.model.composer.toggle(menu);
        if matches!(self.model.composer.menu, Some(Menu::Model | Menu::Search)) {
            if !self.model.providers.loaded {
                self.bridge.load_providers();
            } else {
                self.refresh_models();
            }
        }
        if self.model.composer.menu == Some(Menu::Model) {
            self.model.composer.picker.query.clear();
            self.model.composer.picker.provider = None;
            self.picker_input.update(cx, |i, cx| {
                i.set_value("", window, cx);
                i.focus(window, cx);
            });
        }
        cx.notify();
    }

    /// The + menu picked an item.
    fn plus_picked(&mut self, item: PlusItem, window: &mut Window, cx: &mut Context<Self>) {
        self.model.composer.close();
        match item {
            PlusItem::Link => {
                self.model.composer.link_open = true;
                self.link_input.update(cx, |i, cx| {
                    i.set_value("", window, cx);
                    i.focus(window, cx);
                });
            }
            PlusItem::Resource => {
                self.insert_trigger('@', window, cx);
            }
            PlusItem::Paste => {
                if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
                    if !text.trim().is_empty() {
                        self.model
                            .attached(Ok(tern_agent::Attachment::paste(&text)));
                    }
                } else {
                    self.model.composer.note = Some("The clipboard has no text.".into());
                }
            }
            _ => {
                let dirs = item.picker() == Some(true);
                let picked = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
                    files: true,
                    directories: dirs,
                    multiple: true,
                    prompt: Some(item.label().into()),
                });
                cx.spawn(async move |this, cx| {
                    if let Ok(Ok(Some(paths))) = picked.await {
                        let _ = this.update(cx, |this, cx| {
                            let (keep, dropped) = item.keep(paths);
                            if dropped > 0 {
                                this.model.composer.note = Some(format!(
                                    "{dropped} item{} left out: not {}.",
                                    if dropped == 1 { " was" } else { "s were" },
                                    item.label()
                                ));
                            }
                            this.attach_paths(keep, cx);
                            cx.notify();
                        });
                    }
                })
                .detach();
            }
        }
        cx.notify();
    }

    /// A link typed in the + menu's Link field.
    pub(super) fn attach_link(&mut self, typed: &str, window: &mut Window, cx: &mut Context<Self>) {
        let t = typed.trim();
        let url = if t.starts_with("10.") && t.contains('/') {
            Some(format!("https://doi.org/{t}"))
        } else if let Some(doi) = t.strip_prefix("doi:") {
            Some(format!("https://doi.org/{}", doi.trim()))
        } else {
            tern_agent::browse::normalize(t)
        };
        match url {
            Some(u) => {
                self.model.attaching += 1;
                self.bridge.attach_link(u);
                self.model.composer.link_open = false;
                self.model.composer.note = None;
                self.composer.update(cx, |c, cx| c.focus(window, cx));
            }
            None => {
                self.model.composer.note = Some("That is not a web address or a DOI.".into());
            }
        }
        cx.notify();
    }

    /// Puts `@` or `/` at the end of the composer text and opens its menu.
    fn insert_trigger(&mut self, c: char, window: &mut Window, cx: &mut Context<Self>) {
        let mut text = self.composer.read(cx).value().to_string();
        if !(text.is_empty() || text.ends_with(char::is_whitespace)) {
            text.push(' ');
        }
        text.push(c);
        self.composer.update(cx, |s, cx| {
            s.set_value(text.clone(), window, cx);
            s.focus(window, cx);
        });
        self.model.on_composer_text(&text);
        cx.notify();
    }

    /// Picks a model, from this provider or another, in place.
    fn choose_model(&mut self, provider: &'static str, model: String, cx: &mut Context<Self>) {
        if self.model.is_busy() {
            return;
        }
        self.model.composer.close();
        // A voice, OCR, image, or embedding model never becomes the chat
        // model: the notice says what it does, and the chat keeps its model.
        if let Some(info) = composer::Composer::info_of(&self.model.providers, &model) {
            let key = format!("{provider}/{}", info.id);
            match composer::pick_outcome(provider, info) {
                composer::PickOutcome::Chat => {}
                composer::PickOutcome::Mic(why) => {
                    self.save_ui_prefs(|p| p.default_stt_model = Some(key.clone()));
                    self.model.entries.push(crate::model::Entry::Notice(why));
                    cx.notify();
                    return;
                }
                composer::PickOutcome::Ocr(why) => {
                    self.save_ui_prefs(|p| p.default_ocr_model = Some(key.clone()));
                    self.model.entries.push(crate::model::Entry::Notice(why));
                    cx.notify();
                    return;
                }
                composer::PickOutcome::Refused(why) => {
                    self.model.entries.push(crate::model::Entry::Notice(why));
                    cx.notify();
                    return;
                }
            }
        }
        let current = self.model.providers.default_id(Kind::Model);
        if current != Some(provider) && !self.model.providers.choose(Kind::Model, provider) {
            return;
        }
        self.model.providers.choose_model(&model);
        self.reconnect();
        cx.notify();
    }

    /// Picks a search service (or off) in place.
    fn choose_search(&mut self, id: Option<&'static str>, cx: &mut Context<Self>) {
        if self.model.is_busy() {
            return;
        }
        self.model.composer.close();
        match id {
            Some(id) => {
                if !self.model.providers.choose(Kind::Search, id) {
                    self.model.composer.note = Some(
                        "That search service needs a key or a tested address first: Manage search…"
                            .into(),
                    );
                    cx.notify();
                    return;
                }
            }
            None => self.model.providers.prefs.search = Some("off".into()),
        }
        self.reconnect();
        cx.notify();
    }

    fn set_effort(&mut self, e: Effort, cx: &mut Context<Self>) {
        self.model.composer.effort = Some(e);
        self.model.composer.close();
        self.save_ui_prefs(|p| p.effort = Some(e.id().to_owned()));
        cx.notify();
    }

    /// The thinking support of the open chat's model.
    fn thinking(&self) -> tern_agent::Thinking {
        match &self.model.connection {
            Some(c) => tern_agent::thinking_support(&self.provider_id(), &c.model),
            None => tern_agent::Thinking::Unsupported,
        }
    }

    /// The catalog id of the open chat's provider.
    pub(super) fn provider_id(&self) -> String {
        self.session
            .as_ref()
            .map(|s| s.provider_id().to_owned())
            .or_else(|| {
                self.model
                    .providers
                    .default_id(Kind::Model)
                    .map(str::to_owned)
            })
            .unwrap_or_default()
    }

    /// Opens a folder or a file as the chat's workspace.
    pub(super) fn pick_workspace(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: true,
            directories: true,
            multiple: false,
            prompt: Some("Open as workspace".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = picked.await
                && let Some(path) = paths.into_iter().next()
            {
                let _ = this.update(cx, |this, cx| {
                    this.set_workspace(Some(path), cx);
                });
            }
        })
        .detach();
    }

    pub(super) fn set_workspace(
        &mut self,
        path: Option<std::path::PathBuf>,
        cx: &mut Context<Self>,
    ) {
        let Some(s) = self.session.as_mut() else {
            self.model
                .notice_line("Wait for the chat to connect, then open the folder again.".into());
            cx.notify();
            return;
        };
        match s.set_workspace(path.as_deref()) {
            Ok(()) => {
                self.model.workspace = s.workspace().map(|w| (w.name(), w.path().to_path_buf()));
            }
            Err(e) => self.model.notice_line(e),
        }
        cx.notify();
    }

    /// The title bar's workspace picker.
    pub(super) fn workspace_button(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let label: SharedString = match &self.model.workspace {
            Some((name, _)) => name.clone().into(),
            None => "Open folder".into(),
        };
        div()
            .occlude()
            .child(
                Button::new("open-workspace")
                    .icon(AssetIcon::FolderOpen)
                    .label(label)
                    .ghost()
                    .small()
                    .disabled(self.model.is_busy())
                    .tooltip("Open a folder or a file as this chat's workspace: Tern's file tools read only inside it")
                    .on_click(cx.listener(|this, _, _, cx| this.pick_workspace(cx))),
            )
            .text_color(p.muted)
            .into_any_element()
    }

    /// The chip at the top of a chat with a workspace.
    pub(super) fn workspace_chip(&self, p: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (name, path) = self.model.workspace.as_ref()?;
        let full: SharedString = path.display().to_string().into();
        Some(
            div()
                .w_full()
                .max_w(px(super::COLUMN_MAX_W))
                .mx_auto()
                .px_4()
                .pt_2()
                .flex()
                .child(
                    div()
                        .id("workspace-chip")
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .max_w_full()
                        .pl_2()
                        .pr_1()
                        .py(px(2.))
                        .rounded_md()
                        .border_1()
                        .border_color(p.border)
                        .text_xs()
                        .text_color(p.muted)
                        .child(icon(AssetIcon::FolderOpen, p.muted))
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .child(SharedString::from(name.clone())),
                        )
                        .tooltip(move |window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(full.clone())
                                .build(window, cx)
                        })
                        .child(
                            Button::new("workspace-close")
                                .icon(IconName::Close)
                                .ghost()
                                .xsmall()
                                .disabled(self.model.is_busy())
                                .tooltip("Close the workspace")
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.set_workspace(None, cx)),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }

    /// The composer: menus, banners, chips, the text field, and the row of
    /// controls.
    pub(super) fn composer(&self, p: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = self.model.is_busy();
        let deep_running = busy && self.model.deep_mode;
        let action = if busy {
            Button::new("stop")
                .icon(IconName::Square)
                .label(if self.model.stopping {
                    "Stopping…"
                } else {
                    "Stop"
                })
                .primary()
                .small()
                .disabled(self.model.stopping)
                .tooltip("Stop the answer (Esc)")
                .on_click(cx.listener(|this, _, _, cx| this.stop(cx)))
        } else {
            Button::new("send")
                .icon(IconName::ArrowUp)
                .small()
                .disabled(!self.model.can_send())
                .tooltip("Send (Enter)")
                .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx)))
        };
        let pause = deep_running.then(|| {
            let paused = self.model.paused;
            Button::new("pause")
                .icon(if paused {
                    AssetIcon::Play
                } else {
                    AssetIcon::Pause
                })
                .label(if paused { "Resume" } else { "Pause" })
                .ghost()
                .small()
                .tooltip(if paused {
                    "Let the workers carry on"
                } else {
                    "Hold the workers at their next safe point"
                })
                .on_click(cx.listener(|this, _, _, cx| {
                    this.model.paused = !this.model.paused;
                    if this.model.paused {
                        this.deep_control.pause();
                    } else {
                        this.deep_control.unpause();
                    }
                    cx.notify();
                }))
        });
        let ready = self.model.is_ready() || busy;
        div().w_full().flex().justify_center().px_4().pb_4().child(
            div()
                .id("composer-box")
                .w_full()
                .max_w(px(super::COLUMN_MAX_W))
                .flex()
                .flex_col()
                .gap_2()
                .p_2()
                .rounded_xl()
                .border_1()
                .border_color(p.border)
                .bg(p.raised)
                .drag_over::<gpui_kit::ExternalPaths>(move |style, _, _, _| {
                    style.border_color(p.muted).bg(p.hover)
                })
                .on_drop(cx.listener(|this, paths: &gpui_kit::ExternalPaths, _, cx| {
                    this.attach_paths(paths.paths().to_vec(), cx);
                }))
                .capture_key_down(cx.listener(|this, e: &gpui_kit::KeyDownEvent, window, cx| {
                    this.composer_keys(e, window, cx);
                }))
                .children(self.composer_menu(p, cx))
                .children(self.link_field(p, cx))
                .children(self.composer_note(p))
                .children(self.slash_menu(p, cx))
                .children(self.mention_menu(p, cx))
                .children(self.arg_form_view(p, cx))
                .children(self.notices(p, cx))
                .children(self.reply_bar(p, cx))
                .children(self.steer_hint(p))
                .when(
                    !self.model.attachments.is_empty() || self.model.attaching > 0,
                    |d| d.child(self.attachment_chips(p, cx)),
                )
                .child(
                    gpui_kit::component::input::Textarea::new(&self.composer)
                        .appearance(false)
                        .disabled(!ready),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .justify_between()
                        .gap_1()
                        .px_1()
                        .child(self.left_controls(p, cx))
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .justify_end()
                                .gap_1()
                                .min_w_0()
                                .children(self.model_chip(p, cx))
                                .children(self.effort_chip(p, cx))
                                .children(self.search_chip(p, cx))
                                .child(self.context_ring(p, cx))
                                .children(pause)
                                .child(action),
                        ),
                ),
        )
    }

    /// Esc closes a menu or stops the run; Tab queues a message while one
    /// runs; the `@` and `/` menus take their keys first.
    fn composer_keys(
        &mut self,
        e: &gpui_kit::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = e.keystroke.key.as_str();
        if key == "escape" && self.cancel_recording() {
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if key == "escape" && (self.model.composer.menu.is_some() || self.model.composer.link_open)
        {
            self.model.composer.close();
            self.model.composer.link_open = false;
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if key == "tab" && self.model.is_busy() && !e.keystroke.modifiers.shift {
            let text = self.composer.read(cx).value().to_string();
            if !text.trim().is_empty() {
                self.queued = Some(text);
                self.composer
                    .update(cx, |c, cx| c.set_value("", window, cx));
                cx.stop_propagation();
                cx.notify();
                return;
            }
        }
        if self.effort_keys(key, cx) {
            cx.stop_propagation();
            cx.notify();
            return;
        }
        self.mention_keys(e, window, cx);
    }

    /// + · mode · @ · /
    fn left_controls(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let mode = self.model.composer.mode;
        let open = self.model.composer.menu;
        let busy = self.model.is_busy();
        div()
            .flex()
            .items_center()
            .gap_1()
            .min_w_0()
            .child(
                Button::new("plus")
                    .icon(AssetIcon::Plus)
                    .ghost()
                    .small()
                    .selected(open == Some(Menu::Plus))
                    .tooltip("Attach files, images, PDFs, a folder, a link, or a resource")
                    .on_click(
                        cx.listener(|this, _, window, cx| this.open_menu(Menu::Plus, window, cx)),
                    ),
            )
            .child(
                Button::new("mode")
                    .icon(mode_icon(mode))
                    .label(mode.name())
                    .ghost()
                    .small()
                    .dropdown_caret(true)
                    .selected(open == Some(Menu::Mode))
                    .disabled(busy)
                    .tooltip(mode.line())
                    .on_click(
                        cx.listener(|this, _, window, cx| this.open_menu(Menu::Mode, window, cx)),
                    ),
            )
            .child(
                Button::new("at")
                    .icon(AssetIcon::AtSign)
                    .ghost()
                    .small()
                    .tooltip("Reference a file, source, or resource (@)")
                    .on_click(
                        cx.listener(|this, _, window, cx| this.insert_trigger('@', window, cx)),
                    ),
            )
            .child(
                Button::new("slash")
                    .icon(AssetIcon::SquareSlash)
                    .ghost()
                    .small()
                    .tooltip("Commands, skills, and MCP prompts (/)")
                    .on_click(
                        cx.listener(|this, _, window, cx| this.insert_trigger('/', window, cx)),
                    ),
            )
            .child(self.mic_button(p, cx))
            .into_any_element()
    }

    /// The model chip: provider initial and model name; opens the picker.
    fn model_chip(&self, p: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (label, provider): (SharedString, String) = match &self.model.connection {
            Some(c) => (c.model.clone().into(), c.provider.clone()),
            None => (
                match &self.model.phase {
                    crate::model::Phase::Offline(_) => "No model".into(),
                    _ => "Connecting…".into(),
                },
                String::new(),
            ),
        };
        let open = self.model.composer.menu == Some(Menu::Model);
        let chip = div()
            .id("model-chip")
            .flex()
            .items_center()
            .gap_1()
            .min_w_0()
            .max_w(px(220.))
            .px_1p5()
            .py(px(3.))
            .rounded_md()
            .text_xs()
            .cursor_pointer()
            .text_color(if open { p.text } else { p.muted })
            .when(open, |d| d.bg(p.hover))
            .hover(|d| d.bg(p.hover).text_color(p.text))
            .when(!provider.is_empty(), |d| {
                d.child(super::brand_logo(&provider, 14., p))
            })
            .child(div().min_w_0().truncate().child(label))
            .child(icon(AssetIcon::ChevronDown, p.faint))
            .when(!open, |d| {
                d.tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new("Choose the model for this chat")
                        .build(window, cx)
                })
            })
            .on_click(cx.listener(|this, _, window, cx| this.open_menu(Menu::Model, window, cx)));
        // The picker hangs off the chip, drawn over the chat above it.
        Some(
            div()
                .relative()
                .min_w_0()
                .child(chip)
                .when(open, |d| {
                    d.child(gpui_kit::deferred(self.model_menu(p, cx)).with_priority(1))
                })
                .into_any_element(),
        )
    }

    /// The effort chip: a bar meter and the step. Always shown; it opens
    /// the effort slider, which says why when the model has no control.
    fn effort_chip(&self, p: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let chip = composer::effort_chip(self.thinking(), self.model.composer.effort);
        let open = self.model.composer.menu == Some(Menu::Effort);
        let tip: SharedString = chip.tooltip.clone().into();
        let enabled = chip.enabled;
        let el = div()
            .id("effort-chip")
            .flex()
            .items_center()
            .gap_1p5()
            .px_1p5()
            .py(px(3.))
            .rounded_md()
            .text_xs()
            .cursor_pointer()
            .text_color(if open { p.text } else { p.muted })
            .when(open, |d| d.bg(p.hover))
            .hover(|d| d.bg(p.hover).text_color(p.text))
            .when(!enabled, |d| d.opacity(0.6))
            .child(meter(chip.bars, p))
            .child(SharedString::from(chip.label))
            .when(!open, |d| {
                d.tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
                })
            })
            .on_click(cx.listener(|this, _, window, cx| this.open_menu(Menu::Effort, window, cx)));
        Some(
            div()
                .relative()
                .flex_none()
                .child(el)
                .when(open, |d| {
                    d.child(gpui_kit::deferred(self.effort_slider(p, cx)).with_priority(1))
                })
                .into_any_element(),
        )
    }

    /// Picks an effort stop from the slider (click, drag, or keys).
    fn set_effort_stop(&mut self, stop: usize, cx: &mut Context<Self>) {
        let e = composer::effort_at(stop);
        if self.model.composer.effort != Some(e) {
            self.set_effort(e, cx);
        }
    }

    /// The pointer at window x `x` over the slider's track: the handle
    /// follows it and the level under it is picked; release snaps.
    fn effort_pointer(&mut self, x: f32, cx: &mut Context<Self>) {
        let kind = composer::slider_kind(self.thinking());
        let local = x - self.effort_track.get();
        let Some(at) = composer::drag_stop(local, composer::TRACK_W, kind) else {
            return;
        };
        self.effort_drag = Some(at);
        if let Some(stop) = composer::snap_stop(local, composer::TRACK_W, kind) {
            self.set_effort_stop(stop, cx);
        }
        cx.notify();
    }

    /// Ends a drag: the handle snaps to its stop on the house curve.
    fn effort_release(&mut self, cx: &mut Context<Self>) {
        if self.effort_drag.take().is_some() {
            cx.notify();
        }
    }

    /// Left and Right on the open slider move one stop.
    pub(super) fn effort_keys(&mut self, key: &str, cx: &mut Context<Self>) -> bool {
        if self.model.composer.menu != Some(Menu::Effort) || !matches!(key, "left" | "right") {
            return false;
        }
        let thinking = self.thinking();
        let kind = composer::slider_kind(thinking);
        let from = composer::shown_stop(thinking, self.model.composer.effort);
        if let Some(stop) = composer::step_stop(from, key == "right", kind) {
            self.set_effort_stop(stop, cx);
        }
        true
    }

    /// The effort slider: the level's name in the accent and a plain line,
    /// a reset at the top right, and a rounded box holding about forty
    /// thin ticks with a slim glowing bar handle between six stops (None
    /// to Max). Ticks up to the level glow warm, brightest at the handle.
    /// While dragged the handle follows the pointer at once; on release
    /// (and for keys) it snaps on the house curve, at once with reduced
    /// motion. Only colours and the handle's x change from frame to frame:
    /// the row keeps the same elements and sizes, and nothing is collected.
    /// A model with no thinking control shows it greyed; one that always
    /// thinks shows one fixed state.
    fn effort_slider(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        use composer::{SliderKind, TICKS, TRACK_W};
        let thinking = self.thinking();
        let kind = composer::slider_kind(thinking);
        let effort = self.model.composer.effort;
        let stop = composer::shown_stop(thinking, effort);
        let (title, line) = composer::slider_title(thinking, effort);
        let live = matches!(kind, SliderKind::Adjustable { .. });
        let at = match self.effort_drag.filter(|_| live) {
            Some(drag) => {
                self.glides
                    .borrow_mut()
                    .entry("effort-handle")
                    .or_insert_with(|| crate::motion::Glide::new(drag))
                    .jump(drag);
                drag
            }
            None => self.glide("effort-handle", stop as f32),
        };
        let handle_x = composer::stop_x(at, TRACK_W);
        let is_default =
            composer::stop_of(effort.unwrap_or_default()) == composer::stop_of(Effort::default());
        // The slider's own warm ramp, soft amber to red-orange (none of the
        // accents is warm enough), a touch deeper on the light theme.
        let dark = p.canvas.l < 0.5;
        let amber = gpui_kit::hsla(38. / 360., 0.9, if dark { 0.62 } else { 0.5 }, 1.);
        let hot = gpui_kit::hsla(12. / 360., 0.86, if dark { 0.56 } else { 0.48 }, 1.);
        let rest = p.faint.opacity(0.4);
        let tick_h = 12.;
        let track_h = 26.;
        let ticks = (0..TICKS).map(move |i| {
            let x = composer::tick_x(i, TRACK_W);
            let color = match kind {
                SliderKind::Adjustable { .. } => {
                    let look = composer::tick_look(i, at);
                    let warm = amber.blend(hot.opacity(look.warmth));
                    rest.blend(warm.opacity(look.heat))
                }
                // Always thinks: the whole row is set, in quiet grey.
                SliderKind::Fixed => p.muted.opacity(0.7),
                SliderKind::NoControl => rest,
            };
            div()
                .absolute()
                .left(px(x - 1.))
                .top(px((track_h - tick_h) / 2.))
                .w(px(2.))
                .h(px(tick_h))
                .rounded(px(1.))
                .bg(color)
        });
        let track_origin = self.effort_track.clone();
        let handle_color = if live {
            hot.blend(gpui_kit::white().opacity(0.25))
        } else {
            p.muted
        };
        let track = div()
            .id("effort-track")
            .relative()
            .w(px(TRACK_W))
            .h(px(track_h))
            .when(live, |d| d.cursor_pointer())
            .child(
                gpui_kit::canvas(
                    |_, _, _: &mut gpui_kit::App| (),
                    move |b, _, _, _: &mut gpui_kit::App| {
                        track_origin.set(f32::from(b.origin.x));
                    },
                )
                .absolute()
                .size_full(),
            )
            .children(ticks)
            // The handle: a slim bar taller than the ticks, with a soft
            // halo of two wider, fainter bars behind it.
            .when(kind != SliderKind::NoControl, |d| {
                let bar = |w: f32, h: f32, c: gpui_kit::Hsla| {
                    div()
                        .absolute()
                        .left(px(handle_x - w / 2.))
                        .top(px((track_h - h) / 2.))
                        .w(px(w))
                        .h(px(h))
                        .rounded_full()
                        .bg(c)
                };
                d.when(live, |d| {
                    d.child(bar(14., track_h, hot.opacity(0.12))).child(bar(
                        8.,
                        24.,
                        hot.opacity(0.22),
                    ))
                })
                .child(bar(4., 22., handle_color))
            })
            .when(live, |d| {
                d.on_mouse_down(
                    gpui_kit::MouseButton::Left,
                    cx.listener(|this, e: &gpui_kit::MouseDownEvent, _, cx| {
                        this.effort_pointer(f32::from(e.position.x), cx);
                    }),
                )
            });
        // The tick box: an inset capsule a step off the popover's surface,
        // clipping the handle and its glow.
        let tick_box = div()
            .w_full()
            .flex()
            .justify_center()
            .py(px(8.))
            .px(px(12.))
            .rounded(px(11.))
            .border_1()
            .border_color(p.border)
            .bg(p.raised)
            .overflow_hidden()
            .child(track)
            .when(!live, |d| d.opacity(0.55));
        let reset = Button::new("effort-reset")
            .icon(AssetIcon::Undo2)
            .ghost()
            .xsmall()
            .tooltip("Back to the default (Medium)")
            .disabled(!live || is_default)
            .on_click(cx.listener(|this, _, _, cx| {
                this.set_effort_stop(composer::stop_of(Effort::default()), cx);
                cx.notify();
            }));
        div()
            .id("effort-slider")
            .absolute()
            .bottom(px(30.))
            .right_0()
            .w(px(composer::SLIDER_W))
            .p_3()
            .flex()
            .flex_col()
            .items_center()
            .gap_2()
            .rounded_xl()
            .border_1()
            .border_color(p.border)
            .bg(p.canvas)
            .shadow_lg()
            .occlude()
            .child(
                div()
                    .relative()
                    .w_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(if live { hot } else { p.muted })
                            .child(SharedString::from(title)),
                    )
                    .child(div().text_xs().text_color(p.muted).child(line))
                    .child(div().absolute().top(px(-4.)).right(px(-4.)).child(reset)),
            )
            .child(tick_box)
            // A drag keeps going when the pointer leaves the track, and
            // ends wherever the button comes up.
            .on_mouse_move(cx.listener(|this, e: &gpui_kit::MouseMoveEvent, _, cx| {
                if this.effort_drag.is_some() {
                    if e.pressed_button == Some(gpui_kit::MouseButton::Left) {
                        this.effort_pointer(f32::from(e.position.x), cx);
                    } else {
                        this.effort_release(cx);
                    }
                }
            }))
            .on_mouse_up(
                gpui_kit::MouseButton::Left,
                cx.listener(|this, _: &gpui_kit::MouseUpEvent, _, cx| this.effort_release(cx)),
            )
            .on_mouse_up_out(
                gpui_kit::MouseButton::Left,
                cx.listener(|this, _: &gpui_kit::MouseUpEvent, _, cx| this.effort_release(cx)),
            )
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                if this.model.composer.menu == Some(Menu::Effort) {
                    this.model.composer.close();
                    this.effort_drag = None;
                    cx.notify();
                }
            }))
            .with_animation(
                "effort-slider-in",
                gpui_kit::Animation::new(if self.reduced_motion {
                    std::time::Duration::ZERO
                } else {
                    crate::motion::UI
                })
                .with_easing(crate::motion::ease),
                |d, t| d.opacity(t),
            )
            .into_any_element()
    }

    /// The search chip: which service answers web searches, or off.
    fn search_chip(&self, p: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let c = self.model.connection.as_ref()?;
        let label = composer::search_label(c.search.as_deref());
        let open = self.model.composer.menu == Some(Menu::Search);
        let on = c.search.is_some();
        Some(
            div()
                .id("search-chip")
                .flex()
                .items_center()
                .gap_1()
                .min_w_0()
                .max_w(px(160.))
                .px_1p5()
                .py(px(3.))
                .rounded_md()
                .text_xs()
                .cursor_pointer()
                .text_color(if open { p.text } else { p.muted })
                .when(open, |d| d.bg(p.hover))
                .hover(|d| d.bg(p.hover).text_color(p.text))
                .child(match c.search.as_deref() {
                    Some(name) if on => super::brand_logo(name, 14., p),
                    _ => icon(AssetIcon::GlobeOff, p.faint),
                })
                .child(div().min_w_0().truncate().child(SharedString::from(label)))
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new("Where web searches go")
                        .build(window, cx)
                })
                .on_click(
                    cx.listener(|this, _, window, cx| this.open_menu(Menu::Search, window, cx)),
                )
                .into_any_element(),
        )
    }

    /// A one-line note under the menus.
    fn composer_note(&self, p: Palette) -> Option<AnyElement> {
        let n = self.model.composer.note.clone()?;
        Some(
            div()
                .px_1()
                .text_xs()
                .text_color(p.muted)
                .child(SharedString::from(n))
                .into_any_element(),
        )
    }

    /// The Link field from the + menu.
    fn link_field(&self, p: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.model.composer.link_open {
            return None;
        }
        Some(
            div()
                .flex()
                .items_center()
                .gap_2()
                .px_1()
                .child(icon(AssetIcon::Link, p.muted))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(Input::new(&self.link_input).small()),
                )
                .child(
                    Button::new("link-add")
                        .label("Attach")
                        .small()
                        .on_click(cx.listener(|this, _, window, cx| {
                            let t = this.link_input.read(cx).value().to_string();
                            this.attach_link(&t, window, cx);
                        })),
                )
                .child(
                    Button::new("link-cancel")
                        .icon(IconName::Close)
                        .ghost()
                        .xsmall()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.model.composer.link_open = false;
                            cx.notify();
                        })),
                )
                .into_any_element(),
        )
    }

    /// The open menu, drawn inside the composer above the text so it never
    /// leaves the window.
    fn composer_menu(&self, p: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self
            .model
            .composer
            .menu
            .filter(|m| !matches!(m, Menu::Model | Menu::Effort))?;
        let body = match menu {
            Menu::Plus => self.plus_menu(p, cx),
            Menu::Mode => self.mode_menu(p, cx),
            Menu::Model => return None,
            Menu::Effort => return None,
            Menu::Search => self.search_menu(p, cx),
        };
        Some(
            div()
                .id("composer-menu")
                .p_1()
                .rounded_lg()
                .border_1()
                .border_color(p.border)
                .bg(p.canvas)
                .child(body)
                .with_animation(
                    "composer-menu-in",
                    gpui_kit::Animation::new(if self.reduced_motion {
                        std::time::Duration::ZERO
                    } else {
                        crate::motion::UI
                    })
                    .with_easing(crate::motion::ease),
                    |d, t| d.opacity(t),
                )
                .into_any_element(),
        )
    }

    fn menu_row(
        id: impl Into<gpui_kit::ElementId>,
        lead: AnyElement,
        title: SharedString,
        line: SharedString,
        on: bool,
        enabled: bool,
        p: Palette,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        div()
            .id(id)
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .py_1()
            .rounded_md()
            .when(on, |d| d.bg(p.hover))
            .when(enabled, |d| {
                d.cursor_pointer().hover(move |d| d.bg(p.hover))
            })
            .when(!enabled, |d| d.opacity(0.5))
            .child(lead)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(div().text_sm().text_color(p.text).truncate().child(title))
                    .child(div().text_xs().text_color(p.muted).truncate().child(line)),
            )
            .when(on, |d| d.child(icon(AssetIcon::Check, p.text)))
    }

    fn plus_menu(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .children(PlusItem::ALL.into_iter().enumerate().map(|(i, item)| {
                Self::menu_row(
                    ("plus-item", i),
                    icon(plus_icon(item), p.muted),
                    item.label().into(),
                    item.hint().into(),
                    false,
                    true,
                    p,
                )
                .on_click(
                    cx.listener(move |this, _, window, cx| this.plus_picked(item, window, cx)),
                )
            }))
            .into_any_element()
    }

    fn mode_menu(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let current = self.model.composer.mode;
        let harness = self.model.harness;
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .px_2()
                    .pt_1()
                    .pb(px(2.))
                    .text_xs()
                    .text_color(p.faint)
                    .child(if harness == Harness::Study {
                        "Study modes"
                    } else {
                        "Research modes"
                    }),
            )
            .child({
                // The current mode's highlight slides to a newly picked row.
                const ROW_H: f32 = 44.;
                let modes = Mode::for_harness(harness);
                let at = self.glide(
                    "mode-menu",
                    modes.iter().position(|&m| m == current).unwrap_or(0) as f32,
                );
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .right_0()
                            .top(px(at * ROW_H))
                            .h(px(ROW_H))
                            .rounded_md()
                            .bg(p.hover),
                    )
                    .children(modes.iter().enumerate().map(|(i, &m)| {
                        Self::menu_row(
                            ("mode-item", i),
                            icon(mode_icon(m), p.muted),
                            m.name().into(),
                            m.line().into(),
                            false,
                            true,
                            p,
                        )
                        .h(px(ROW_H))
                        .when(m == current, |d| d.child(icon(AssetIcon::Check, p.text)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let chat = this
                                .session
                                .as_ref()
                                .filter(|s| !s.is_draft())
                                .map(|s| s.session_id().to_string());
                            this.model.set_mode(m, chat.as_deref());
                            cx.notify();
                        }))
                    }))
            })
            .into_any_element()
    }

    fn search_menu(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let st = &self.model.providers;
        let current = self
            .model
            .connection
            .as_ref()
            .and_then(|c| c.search.clone());
        let mut col = div().flex().flex_col().child(
            div()
                .px_2()
                .pt_1()
                .pb(px(2.))
                .text_xs()
                .text_color(p.faint)
                .child("Web search for this chat"),
        );
        if !st.loaded {
            col = col.child(
                div()
                    .px_2()
                    .py_1()
                    .text_xs()
                    .text_color(p.muted)
                    .child("Loading…"),
            );
        }
        for (i, r) in st.section(Kind::Search).enumerate() {
            let id = r.id;
            let on = current.as_deref() == Some(r.name);
            let ok = st.can_choose(r);
            let row = Self::menu_row(
                ("search-item", i),
                super::brand_logo(r.id, 16., p),
                r.name.into(),
                st.search_status(r).into(),
                on,
                ok,
                p,
            );
            col = col.child(if ok {
                row.on_click(cx.listener(move |this, _, _, cx| this.choose_search(Some(id), cx)))
            } else {
                row
            });
        }
        col.child(
            Self::menu_row(
                "search-off",
                icon(AssetIcon::GlobeOff, p.muted),
                "Off".into(),
                "Answer from the model alone".into(),
                current.is_none() && self.model.connection.is_some(),
                true,
                p,
            )
            .on_click(cx.listener(|this, _, _, cx| this.choose_search(None, cx))),
        )
        .child(
            div()
                .id("manage-search")
                .px_2()
                .py_1()
                .text_xs()
                .text_color(p.muted)
                .cursor_pointer()
                .hover(|d| d.text_color(p.text))
                .child("Manage search…")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.model.composer.close();
                    this.open_providers(cx);
                })),
        )
        .into_any_element()
    }

    /// The model picker: a compact popover over the model chip. Providers
    /// with their logos on the left; on the right a search field and one
    /// line per model (tick on the current one, star on hover or when
    /// starred, the window size muted on the right). Hovering a model shows
    /// a small card beside the popover.
    fn model_menu(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let c = &self.model.composer;
        let st = &self.model.providers;
        let shown = c.shown_provider(st);
        let current_model = self.model.connection.as_ref().map(|c| c.model.clone());
        let providers = c.providers(st);
        let (w, h) = composer::picker_size(self.centre_w.max(480.), 900.);
        let left = div()
            .id("picker-providers")
            .w(px(composer::PICKER_LEFT_W))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .gap(px(1.))
            .p_1()
            .border_r_1()
            .border_color(p.border)
            .overflow_y_scroll()
            .children(providers.into_iter().enumerate().map(|(i, e)| {
                let id = e.id;
                let on = Some(e.id) == shown;
                div()
                    .id(("picker-provider", i))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_1p5()
                    .py(px(5.))
                    .rounded_md()
                    .cursor_pointer()
                    .text_sm()
                    .text_color(if on { p.text } else { p.muted })
                    .when(on, |d| d.bg(p.hover))
                    .hover(move |d| d.bg(p.hover).text_color(p.text))
                    .child(super::brand_logo(e.id, 16., p))
                    .child(div().flex_1().min_w_0().truncate().child(e.name))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.model.composer.picker.provider = Some(id);
                        this.model.composer.picker.hovered = None;
                        let loaded = matches!(&this.model.providers.models,
                            Some(crate::providers::Models::Loaded(p, _)) if *p == id);
                        if !loaded {
                            this.model.providers.models_loading(id);
                            this.bridge.models(id);
                        }
                        cx.notify();
                    }))
            }))
            .child(div().flex_1())
            .child(
                div()
                    .id("manage-providers")
                    .px_1p5()
                    .py_1()
                    .text_xs()
                    .text_color(p.faint)
                    .cursor_pointer()
                    .hover(|d| d.text_color(p.text))
                    .child("Manage providers…")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.model.composer.close();
                        this.open_providers(cx);
                    })),
            );
        let hovered = c.picker.hovered.clone();
        let list: AnyElement = match (shown, c.picker_rows(st, current_model.as_deref())) {
            (None, _) => div()
                .p_2()
                .text_xs()
                .text_color(p.muted)
                .child("Add a provider key to choose a model.")
                .into_any_element(),
            (Some(_), None) => div()
                .p_2()
                .text_xs()
                .text_color(p.muted)
                .child(match &st.models {
                    Some(crate::providers::Models::Failed(_, e)) => {
                        SharedString::from(format!("Could not list models: {e}"))
                    }
                    _ => SharedString::from("Loading models…"),
                })
                .into_any_element(),
            (Some(_), Some(rows)) if rows.is_empty() => div()
                .p_2()
                .text_xs()
                .text_color(p.muted)
                .child("No model matches.")
                .into_any_element(),
            (Some(prov), Some(rows)) => {
                let mut items: Vec<AnyElement> = Vec::new();
                let mut section = None;
                for (i, r) in rows.into_iter().take(400).enumerate() {
                    if section != Some(r.category) {
                        section = Some(r.category);
                        items.push(picker_section(r.category, p));
                    }
                    items.push(self.picker_row(i, prov, r, hovered.as_deref(), p, cx));
                }
                div()
                    .id("picker-models")
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .overflow_y_scroll()
                    .children(items)
                    .into_any_element()
            }
        };
        let retired = c.retired_count(st);
        let show_retired = c.picker.show_retired;
        let retired_toggle = (retired > 0).then(|| {
            div()
                .id("picker-retired")
                .px_1p5()
                .text_xs()
                .text_color(p.faint)
                .cursor_pointer()
                .hover(|d| d.text_color(p.text))
                .child(SharedString::from(if show_retired {
                    "Hide retired".to_owned()
                } else {
                    format!("Show retired ({retired})")
                }))
                .on_click(cx.listener(|this, _, _, cx| {
                    let p = &mut this.model.composer.picker;
                    p.show_retired = !p.show_retired;
                    cx.notify();
                }))
        });
        // The small card beside the popover for the hovered model.
        let card = hovered.map(|m| {
            let hint = composer::model_hint(&m);
            let ctx = composer::Composer::context_of(st, &m);
            let info = composer::Composer::info_of(st, &m).cloned();
            let think = shown
                .map(|prov| tern_agent::thinking_support(prov, &m))
                .unwrap_or(tern_agent::Thinking::Unsupported);
            div()
                .absolute()
                .bottom_0()
                .right(px(w + 6.))
                .w(px(220.))
                .p_2()
                .rounded_md()
                .border_1()
                .border_color(p.border)
                .bg(p.raised)
                .shadow_md()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(p.text)
                        .truncate()
                        .child(SharedString::from(m.clone())),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(p.muted)
                        .child(SharedString::from(match ctx {
                            Some(n) => format!("{} token window", crate::model::compact(n)),
                            None => "Window size not known".to_owned(),
                        })),
                )
                .when(info.as_ref().is_none_or(|i| i.can_chat()), |d| {
                    d.child(scale("Speed", hint.speed, p))
                        .child(scale("Intelligence", hint.intelligence, p))
                        .child(
                            div()
                                .text_xs()
                                .text_color(p.faint)
                                .child(SharedString::from(
                                    match (think.visible(), think.adjustable()) {
                                        (false, _) => "No thinking control",
                                        (true, false) => "Always thinks",
                                        (true, true) => "Thinking effort adjustable",
                                    },
                                )),
                        )
                })
                .children(info.as_ref().map(|i| {
                    let caps: Vec<&str> = i.capabilities.iter().map(|c| c.badge()).collect();
                    div()
                        .text_xs()
                        .text_color(p.muted)
                        .child(SharedString::from(if caps.is_empty() {
                            "Not used by Tern".to_owned()
                        } else {
                            format!("Can: {}", caps.join(", "))
                        }))
                }))
                .children(info.as_ref().filter(|i| !i.aliases.is_empty()).map(|i| {
                    div()
                        .text_xs()
                        .text_color(p.faint)
                        .child(SharedString::from(format!(
                            "Also: {}",
                            i.aliases.join(", ")
                        )))
                }))
                .children(info.as_ref().and_then(|i| i.not_chat_reason()).map(|r| {
                    div()
                        .text_xs()
                        .text_color(p.muted)
                        .child(SharedString::from(r))
                }))
        });
        div()
            .id("model-picker")
            .absolute()
            .bottom(px(30.))
            .right_0()
            .w(px(w))
            .h(px(h))
            .child(
                div()
                    .id("model-picker-box")
                    .size_full()
                    .flex()
                    .rounded_lg()
                    .border_1()
                    .border_color(p.border)
                    .bg(p.canvas)
                    .shadow_lg()
                    .occlude()
                    .overflow_hidden()
                    .child(left)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .p_1()
                            .child(Input::new(&self.picker_input).small())
                            .child(list)
                            .children(retired_toggle),
                    ),
            )
            .children(card)
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                if this.model.composer.menu == Some(Menu::Model) {
                    this.model.composer.close();
                    cx.notify();
                }
            }))
            .into_any_element()
    }

    /// Web search failed its check: one line naming the service (and
    /// SearXNG's address), with Switch to Wikipedia and Set address.
    fn search_down_banner(&self, p: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let reason = self.model.search_down.clone()?;
        let service = self
            .model
            .connection
            .as_ref()
            .and_then(|c| c.search.clone())
            .unwrap_or_else(|| "Web search".into());
        let is_searx = service.eq_ignore_ascii_case("searxng");
        let address = self
            .model
            .providers
            .prefs
            .search_url
            .clone()
            .or_else(|| is_searx.then(|| "http://localhost:8080".to_owned()));
        let line = chatflow::search_down_line(&service, address.as_deref());
        let detail: SharedString = reason.into();
        Some(
            div()
                .id("search-down")
                .flex()
                .flex_wrap()
                .items_center()
                .gap_2()
                .px_2()
                .py_1p5()
                .rounded_lg()
                .border_1()
                .border_color(p.border)
                .bg(p.canvas)
                .child(icon(AssetIcon::GlobeOff, p.muted))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_sm()
                        .text_color(p.text)
                        .truncate()
                        .child(SharedString::from(line)),
                )
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(detail.clone()).build(window, cx)
                })
                .child(
                    Button::new("search-down-wiki")
                        .label("Switch to Wikipedia")
                        .small()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.model.search_down = None;
                            this.choose_search(Some("wikipedia"), cx);
                        })),
                )
                .child(
                    Button::new("search-down-set")
                        .label(if is_searx {
                            "Set address"
                        } else {
                            "Manage search"
                        })
                        .ghost()
                        .small()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.model.search_down = None;
                            this.open_providers(cx);
                        })),
                )
                .into_any_element(),
        )
    }

    /// The banners over the composer, one at a time with "N more" to open
    /// the rest.
    fn notices(&self, p: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let active = self.model.active_notices();
        if active.is_empty() {
            return None;
        }
        let (shown, folded) = chatflow::stack(active.clone(), self.model.notices_open);
        let mut col = div().flex().flex_col().gap_1();
        for n in shown {
            col = col.children(match n {
                Notice::SearchDown => self.search_down_banner(p, cx),
                Notice::Resume => self.resume_banner(p, cx),
                Notice::Continue => self.continue_bar(p, cx),
                Notice::Inserted => self.inserted_banner(p, cx),
            });
        }
        if folded > 0 || (self.model.notices_open && active.len() > 1) {
            let label = if folded > 0 {
                chatflow::more_notices(folded)
            } else {
                "Show fewer".to_owned()
            };
            col = col.child(
                div()
                    .id("notices-more")
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_1()
                    .text_xs()
                    .text_color(p.muted)
                    .cursor_pointer()
                    .hover(|d| d.text_color(p.text))
                    .child(SharedString::from(label))
                    .child(icon(
                        if folded > 0 {
                            AssetIcon::ChevronDown
                        } else {
                            AssetIcon::ChevronUp
                        },
                        p.faint,
                    ))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.model.notices_open = !this.model.notices_open;
                        cx.notify();
                    })),
            );
        }
        Some(col.into_any_element())
    }

    // ----- inside answers -----

    /// The Thinking section: live and open while the model thinks, folded
    /// to "Thought for Ns ›" once the answer starts.
    pub(super) fn thinking_live(&self, p: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let t = &self.model.thinking;
        if t.is_empty() || !self.model.is_busy() {
            return None;
        }
        let open = t.is_open();
        Some(thinking_view(
            "thinking-live".into(),
            t.label(),
            t.text.clone(),
            open,
            p,
            cx.listener(|this, _, _, cx| {
                this.model.thinking.toggled = !this.model.thinking.toggled;
                cx.notify();
            }),
        ))
    }

    /// A finished answer's Thinking section, folded unless opened.
    pub(super) fn thinking_done(
        &self,
        i: usize,
        th: &chatflow::Thought,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = self.model.thought_open.contains(&i);
        thinking_view(
            SharedString::from(format!("thought-{i}")),
            th.label(),
            th.text.clone(),
            open,
            p,
            cx.listener(move |this, _, _, cx| {
                this.model.toggle_thought(i);
                cx.notify();
            }),
        )
    }

    /// The working rollup: one line for the run's tool work, opening to
    /// Summary, Detailed, or Raw inline. `i` is the answer, `None` live.
    pub(super) fn rollup(
        &self,
        i: Option<usize>,
        run: &Run,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let line = chatflow::rollup(run)?;
        let open = match i {
            Some(i) => self.model.rollup_open.contains(&i),
            None => self.model.live_rollup_open,
        };
        let id: SharedString = match i {
            Some(i) => format!("rollup-{i}").into(),
            None => "rollup-live".into(),
        };
        let view = self.model.rollup_view;
        let mut col = div().flex().flex_col().gap_1().child(
            div()
                .id(id)
                .flex()
                .items_center()
                .gap_1()
                .text_xs()
                .text_color(p.muted)
                .cursor_pointer()
                .hover(|d| d.text_color(p.text))
                .child(icon(AssetIcon::ListTree, p.faint))
                .child(SharedString::from(line))
                .child(icon(
                    if open {
                        AssetIcon::ChevronDown
                    } else {
                        AssetIcon::ChevronRight
                    },
                    p.faint,
                ))
                .on_click(cx.listener(move |this, _, _, cx| {
                    match i {
                        Some(i) => this.model.toggle_rollup(i),
                        None => this.model.live_rollup_open = !this.model.live_rollup_open,
                    }
                    cx.notify();
                })),
        );
        if open {
            let tabs = div()
                .flex()
                .gap_1()
                .children(RollupView::ALL.into_iter().map(|v| {
                    Button::new(SharedString::from(format!(
                        "rollup-view-{}-{}",
                        v.label(),
                        i.unwrap_or(usize::MAX)
                    )))
                    .label(v.label())
                    .ghost()
                    .xsmall()
                    .selected(v == view)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.model.rollup_view = v;
                        cx.notify();
                    }))
                }));
            let body = match view {
                RollupView::Summary => self.run_summary(run, p, cx),
                RollupView::Detailed => self.run_detailed(run, p),
                RollupView::Raw => self.run_raw(run, p, cx),
            };
            col = col.child(
                div()
                    .pl_3()
                    .ml_1()
                    .border_l_1()
                    .border_color(p.border)
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(tabs)
                    .child(div().max_h(px(320.)).overflow_hidden().child(body)),
            );
        }
        // Workers of a running deep run, each with its own stop.
        if i.is_none()
            && let Some(d) = &run.deep
        {
            let live: Vec<_> = d
                .workers
                .iter()
                .filter(|w| !w.status.is_finished())
                .collect();
            if !live.is_empty() {
                col = col.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_1()
                        .children(live.into_iter().map(|w| {
                            let n = w.n;
                            div()
                                .flex()
                                .items_center()
                                .gap_1()
                                .max_w(px(260.))
                                .pl_2()
                                .pr(px(2.))
                                .py(px(1.))
                                .rounded_md()
                                .border_1()
                                .border_color(p.border)
                                .text_xs()
                                .text_color(p.muted)
                                .child(
                                    div()
                                        .min_w_0()
                                        .truncate()
                                        .child(SharedString::from(format!("{n}. {}", w.question))),
                                )
                                .child(
                                    Button::new(SharedString::from(format!(
                                        "inline-worker-stop-{n}"
                                    )))
                                    .icon(AssetIcon::X)
                                    .ghost()
                                    .xsmall()
                                    .tooltip("Stop this worker; the others carry on")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.deep_control.cancel_worker(n);
                                        cx.notify();
                                    })),
                                )
                        })),
                );
            }
        }
        Some(col.into_any_element())
    }

    /// The hover line beside a message or under an answer.
    pub(super) fn stamp(&self, i: usize, group: &'static str, p: Palette) -> Option<AnyElement> {
        let now = tern_agent::checkpoint::now_rfc3339();
        let label = self
            .model
            .stamp(i, &now, tern_agent::checkpoint::local_offset())?;
        let g = SharedString::from(format!("{group}-{i}"));
        Some(
            div()
                .text_xs()
                .text_color(p.faint)
                .opacity(0.)
                .group_hover(g, |s| s.opacity(1.))
                .child(SharedString::from(label))
                .into_any_element(),
        )
    }

    /// Copy · Reply · Branch ▾ · More ▾ on answers; Copy · Reply on messages.
    pub(super) fn grouped_actions(
        &self,
        i: usize,
        answer: bool,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let copied = self.model.copied == Some(i);
        let busy = self.model.is_busy();
        let open = self.model.action_menu;
        let action = move |id: &'static str, icon: AssetIcon, tip: &'static str| {
            div()
                .id((id, i))
                .flex()
                .items_center()
                .gap(px(2.))
                .p(px(3.))
                .rounded_md()
                .cursor_pointer()
                .text_color(p.faint)
                .hover(move |d| d.text_color(p.text).bg(p.hover))
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(tip).build(window, cx)
                })
                .child(gpui_kit::component::Icon::new(icon).xsmall())
        };
        let mut row = div()
            .flex()
            .items_center()
            .gap(px(2.))
            .child(
                action(
                    "copy",
                    if copied {
                        AssetIcon::Check
                    } else {
                        AssetIcon::Copy
                    },
                    "Copy",
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(t) = this.model.entry_text(i) {
                        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(t.to_owned()));
                        this.model.copied = Some(i);
                        cx.notify();
                    }
                })),
            )
            .child(
                action("reply", AssetIcon::Reply, "Reply").on_click(cx.listener(
                    move |this, _, window, cx| {
                        this.model.reply_to_entry(i);
                        this.composer.update(cx, |c, cx| c.focus(window, cx));
                        cx.notify();
                    },
                )),
            );
        if answer {
            row = row
                .when(!busy, |d| {
                    d.child(
                        action(
                            "branch-menu",
                            AssetIcon::GitBranch,
                            "Branch: sub-session, background branch, move, copy, side chat",
                        )
                        .when(open == Some((i, ActionMenu::Branch)), |d| {
                            d.bg(p.hover).text_color(p.text)
                        })
                        .child(gpui_kit::component::Icon::new(AssetIcon::ChevronDown).xsmall())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.model.toggle_action_menu(i, ActionMenu::Branch);
                            cx.notify();
                        })),
                    )
                })
                .child(
                    action(
                        "more-menu",
                        AssetIcon::Ellipsis,
                        "More: save, watch, Explain in 3D",
                    )
                    .when(open == Some((i, ActionMenu::More)), |d| {
                        d.bg(p.hover).text_color(p.text)
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.model.toggle_action_menu(i, ActionMenu::More);
                        cx.notify();
                    })),
                );
        }
        row.into_any_element()
    }

    /// The open Branch ▾ or More ▾ menu under answer `i`.
    pub(super) fn action_menu_view(
        &self,
        i: usize,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (at, which) = self.model.action_menu?;
        if at != i {
            return None;
        }
        let item = move |id: &'static str, icon_name: AssetIcon, label: SharedString| {
            div()
                .id((id, i))
                .flex()
                .items_center()
                .gap_2()
                .px_2()
                .py_1()
                .rounded_md()
                .text_sm()
                .cursor_pointer()
                .text_color(p.text)
                .hover(move |d| d.bg(p.hover))
                .child(icon(icon_name, p.muted))
                .child(label)
        };
        let menu = div()
            .flex()
            .flex_col()
            .w(px(300.))
            .max_w_full()
            .p_1()
            .rounded_lg()
            .border_1()
            .border_color(p.border)
            .bg(p.raised);
        let menu = match which {
            ActionMenu::Branch => menu
                .child(
                    item(
                        "am-sub",
                        AssetIcon::GitBranch,
                        "New sub-session from here".into(),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.model.action_menu = None;
                        this.model.cont.ids.branch_menu = Some(i);
                        cx.notify();
                    })),
                )
                .child(
                    item(
                        "am-copy",
                        AssetIcon::GitFork,
                        "Branch into a new chat (a copy up to here)".into(),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.model.action_menu = None;
                        this.branch_from(i, cx);
                    })),
                )
                .child(
                    item(
                        "am-side",
                        AssetIcon::MessageCircle,
                        "Ask in side chat (saves nothing)".into(),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.model.action_menu = None;
                        this.open_side(None, window, cx);
                    })),
                ),
            ActionMenu::More => {
                let text = self.model.entry_text(i).unwrap_or_default();
                let watched = self.model.watch_of(i).is_some();
                let saved = self.model.saved_answer == Some(i);
                let mut m = menu;
                for (k, label) in chatflow::more_items(text, watched) {
                    m = m.child(match k {
                        "save" => item(
                            "am-save",
                            if saved {
                                AssetIcon::Check
                            } else {
                                AssetIcon::NotebookPen
                            },
                            if saved {
                                "Saved to vault".into()
                            } else {
                                label.into()
                            },
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.model.action_menu = None;
                            this.save_answer(i, cx);
                        })),
                        "watch" => item(
                            "am-watch",
                            if watched {
                                AssetIcon::EyeOff
                            } else {
                                AssetIcon::Eye
                            },
                            label.into(),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.model.action_menu = None;
                            this.toggle_watch(i, cx);
                        })),
                        _ => item("am-3d", AssetIcon::Cuboid, label.into()).on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.model.action_menu = None;
                                let Some(text) = this.model.entry_text(i).map(str::to_owned) else {
                                    return;
                                };
                                let label = this
                                    .model
                                    .question_before(i)
                                    .unwrap_or_else(|| "this answer".into());
                                let ask = tern_agent::explain::Ask {
                                    concept: String::new(),
                                    topic: None,
                                    context: Some(text),
                                };
                                this.open_explain(ask, &label, window, cx);
                            },
                        )),
                    });
                }
                m
            }
        };
        Some(div().flex().justify_end().child(menu).into_any_element())
    }

    /// Watch, or stop watching with Undo.
    pub(super) fn toggle_watch(&mut self, i: usize, cx: &mut Context<Self>) {
        if let Some(id) = self.model.unwatch(i) {
            self.bridge.remove_watch(id);
            self.start_undo_timer(cx);
        } else {
            self.watch_answer(i, cx);
        }
        cx.notify();
    }

    /// Clears the Undo toast once it expires.
    pub(super) fn start_undo_timer(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(chatflow::UNDO_FOR).await;
            let _ = this.update(cx, |this, cx| {
                if this
                    .model
                    .undo
                    .as_ref()
                    .is_some_and(|u| u.expired(std::time::Instant::now()))
                {
                    this.model.undo = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// The Undo toast at the bottom of the window.
    pub(super) fn undo_toast(&self, p: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let u = self.model.undo.as_ref()?;
        Some(
            div()
                .absolute()
                .bottom(px(20.))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .pl_3()
                        .pr_1()
                        .py_1()
                        .rounded_lg()
                        .border_1()
                        .border_color(p.border)
                        .bg(p.raised)
                        .shadow_md()
                        .text_sm()
                        .text_color(p.text)
                        .child(SharedString::from(u.label.clone()))
                        .child(
                            Button::new("undo")
                                .icon(AssetIcon::Undo2)
                                .label("Undo")
                                .ghost()
                                .small()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    match this.model.take_undo() {
                                        Some(UndoKind::Watch {
                                            question,
                                            every_hours,
                                            deep,
                                        }) => this.bridge.add_watch(question, every_hours, deep),
                                        Some(UndoKind::Chat(id)) => this.bridge.restore_chat(id),
                                        None => {}
                                    }
                                    cx.notify();
                                })),
                        ),
                )
                .into_any_element(),
        )
    }

    /// The streaming draft: Thinking, the rollup, the answer so far, and
    /// Show all while the reveal is behind.
    pub(super) fn live_answer(&self, p: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let thinking = self.thinking_live(p, cx);
        let rollup = match &self.model.phase {
            crate::model::Phase::Working { run, .. } => self.rollup(None, run, p, cx),
            _ => None,
        };
        let draft = self.model.paced_draft();
        if thinking.is_none() && rollup.is_none() && draft.is_none() {
            return None;
        }
        let i = self.model.entries.len();
        let behind = self
            .model
            .draft()
            .is_some_and(|d| d.trim_end().len() > draft.map_or(0, str::len) + 80);
        Some(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .children(thinking)
                .children(rollup)
                .children(draft.map(|d| {
                    gpui_kit::component::text::TextView::markdown(("answer", i), d.to_owned())
                        .selectable(true)
                        .motion(super::stream_motion())
                }))
                .when(behind, |d| d.child(show_all(p, cx)))
                .into_any_element(),
        )
    }
}

/// "Show all" while the reveal is behind the stream.
/// A section title in the model picker ("Speech").
fn picker_section(c: tern_agent::providers::Category, p: Palette) -> AnyElement {
    div()
        .px_1p5()
        .pt_1p5()
        .pb(px(2.))
        .flex_none()
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .text_color(p.faint)
        .child(c.label())
        .into_any_element()
}

impl Workspace {
    /// One model line: tick, name (and id when the provider names it),
    /// badges, star, window size. A model that cannot chat is drawn muted;
    /// clicking it routes it to its own job instead.
    fn picker_row(
        &self,
        i: usize,
        prov: &'static str,
        r: composer::PickerRow,
        hovered: Option<&str>,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let show_star = r.favourite || hovered == Some(r.id.as_str());
        let pick = r.id.clone();
        let hover = r.id.clone();
        let fav = r.id.clone();
        let badges: Vec<&str> = r.badges.iter().take(2).copied().collect();
        div()
            .id(("picker-model", i))
            .flex()
            .items_center()
            .gap_1p5()
            .px_1p5()
            .h(px(26.))
            .flex_none()
            .rounded_md()
            .cursor_pointer()
            .text_sm()
            .text_color(if r.chat && !r.retired {
                p.text
            } else {
                p.muted
            })
            .hover(move |d| d.bg(p.hover))
            .child(
                div()
                    .w(px(14.))
                    .flex_none()
                    .when(r.current, |d| d.child(icon(AssetIcon::Check, p.text))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_baseline()
                    .gap_1()
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .child(SharedString::from(r.label.clone().unwrap_or(r.id.clone()))),
                    )
                    .when(r.label.is_some(), |d| {
                        d.child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(p.faint)
                                .child(SharedString::from(r.id.clone())),
                        )
                    }),
            )
            .children(badges.into_iter().map(|b| {
                div()
                    .flex_none()
                    .px_1()
                    .rounded_sm()
                    .border_1()
                    .border_color(p.border)
                    .text_xs()
                    .text_color(p.faint)
                    .child(b)
            }))
            .child(
                div()
                    .id(("picker-star", i))
                    .w(px(14.))
                    .flex_none()
                    .text_color(if r.favourite { p.text } else { p.faint })
                    .when(show_star, |d| {
                        d.child(
                            gpui_kit::component::Icon::new(if r.favourite {
                                AssetIcon::StarFill
                            } else {
                                AssetIcon::Star
                            })
                            .xsmall(),
                        )
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        let mut favs = this.model.providers.prefs.favourite_models.clone();
                        composer::toggle_favourite(&mut favs, prov, &fav);
                        this.save_ui_prefs(|p| p.favourite_models = favs.clone());
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .w(px(36.))
                    .flex_none()
                    .text_xs()
                    .text_right()
                    .text_color(p.faint)
                    .children(r.context.map(SharedString::from)),
            )
            .on_hover(cx.listener(move |this, h: &bool, _, cx| {
                let picker = &mut this.model.composer.picker;
                if *h {
                    picker.hovered = Some(hover.clone());
                } else if picker.hovered.as_deref() == Some(hover.as_str()) {
                    picker.hovered = None;
                }
                cx.notify();
            }))
            .on_click(cx.listener(move |this, _, _, cx| this.choose_model(prov, pick.clone(), cx)))
            .into_any_element()
    }
}

pub(super) fn show_all(p: Palette, cx: &mut Context<Workspace>) -> AnyElement {
    div()
        .id("show-all")
        .text_xs()
        .text_color(p.muted)
        .cursor_pointer()
        .hover(|d| d.text_color(p.text))
        .child("Show all")
        .on_click(cx.listener(|this, _, _, cx| {
            this.model.reveal_all();
            cx.notify();
        }))
        .into_any_element()
}

fn thinking_view(
    id: SharedString,
    label: String,
    text: String,
    open: bool,
    p: Palette,
    on_toggle: impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut gpui_kit::App) + 'static,
) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .id(id)
                .flex()
                .items_center()
                .gap_1()
                .text_xs()
                .text_color(p.muted)
                .cursor_pointer()
                .hover(|d| d.text_color(p.text))
                .child(icon(AssetIcon::Brain, p.faint))
                .child(SharedString::from(label))
                .child(icon(
                    if open {
                        AssetIcon::ChevronDown
                    } else {
                        AssetIcon::ChevronRight
                    },
                    p.faint,
                ))
                .on_click(on_toggle),
        )
        .when(open, |d| {
            d.child(
                div()
                    .pl_3()
                    .ml_1()
                    .border_l_1()
                    .border_color(p.border)
                    .text_xs()
                    .text_color(p.muted)
                    .max_h(px(260.))
                    .overflow_hidden()
                    .child(SharedString::from(text)),
            )
        })
        .into_any_element()
}
