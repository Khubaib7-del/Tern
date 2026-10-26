//! The Settings pieces added in the settings review: custom providers
//! (a form and a Paste config box), the Crawlers group, the Instructions
//! page, the MCP templates gallery with its Add and Paste forms, and the
//! skill editor with GitHub import and templates. The model logic lives in
//! `tern-agent` (and `crate::settings` for the screen's own state); this
//! file draws it and sends the work to the bridge.

use gpui_kit::assets::IconName as AssetIcon;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState, Textarea, TextareaState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{Disableable as _, Icon, Selectable as _, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, Entity, FontWeight, SharedString, Window, div, px};
use tern_agent::crawlers::Crawler;
use tern_agent::mcp::presets;
use tern_agent::prompts::{self, Scope};
use tern_agent::providers::{CustomProvider, CustomWire, custom};
use tern_agent::skill_author;
use tern_core::SecretString;

use super::Workspace;
use super::press::Capsule as _;
use crate::assets;
use crate::bridge::{McpChange, SetupMsg, SkillChange};
use crate::providers::Kind;
use crate::settings::{Line, Section};
use crate::theme::Palette;

/// The text fields of the review's Settings pieces.
pub(super) struct SetupInputs {
    pub crawler_key: Entity<InputState>,
    pub custom_name: Entity<InputState>,
    pub custom_url: Entity<InputState>,
    pub custom_key: Entity<InputState>,
    pub custom_headers: Entity<TextareaState>,
    pub custom_paste: Entity<TextareaState>,
    pub prompt_global: Entity<TextareaState>,
    pub prompt_research: Entity<TextareaState>,
    pub prompt_study: Entity<TextareaState>,
    pub mcp_name: Entity<InputState>,
    pub mcp_target: Entity<InputState>,
    pub mcp_pairs: Entity<TextareaState>,
    pub mcp_paste: Entity<TextareaState>,
    /// One field per value the open MCP template needs.
    pub template_fields: Vec<(&'static str, Entity<InputState>)>,
    pub skill_name: Entity<InputState>,
    pub skill_desc: Entity<InputState>,
    pub skill_body: Entity<TextareaState>,
    pub skill_github: Entity<InputState>,
}

impl SetupInputs {
    pub fn new(window: &mut Window, cx: &mut Context<Workspace>) -> Self {
        let input = |ph: &'static str, window: &mut Window, cx: &mut Context<Workspace>| {
            cx.new(|cx| InputState::new(window, cx).placeholder(ph))
        };
        let area =
            |ph: &'static str, rows: usize, window: &mut Window, cx: &mut Context<Workspace>| {
                cx.new(|cx| {
                    TextareaState::new(window, cx)
                        .auto_grow(rows, 14)
                        .placeholder(ph)
                })
            };
        Self {
            crawler_key: cx.new(|cx| {
                InputState::new(window, cx)
                    .masked(true)
                    .placeholder("Paste the API key")
            }),
            custom_name: input("Name, e.g. Office vLLM", window, cx),
            custom_url: input("Base URL, e.g. http://localhost:8000/v1", window, cx),
            custom_key: cx.new(|cx| {
                InputState::new(window, cx)
                    .masked(true)
                    .placeholder("API key (leave empty for a local server)")
            }),
            custom_headers: area(
                "Optional headers, one per line: X-Org: research",
                2,
                window,
                cx,
            ),
            custom_paste: area(
                "Paste a provider config: {\"name\": \"…\", \"baseURL\": \"…\", \"apiKey\": \"…\"}, a models.json, or an opencode provider map",
                4,
                window,
                cx,
            ),
            prompt_global: area("Your instructions for every chat", 3, window, cx),
            prompt_research: area("Your instructions for Research chats", 3, window, cx),
            prompt_study: area("Your instructions for Study chats", 3, window, cx),
            mcp_name: input("Name, e.g. zotero", window, cx),
            mcp_target: input(
                "Command and arguments (npx -y @scope/server) or an http(s) URL",
                window,
                cx,
            ),
            mcp_pairs: area(
                "Environment (KEY=value) for a command, or headers (Name: value) for a URL, one per line",
                2,
                window,
                cx,
            ),
            mcp_paste: area(
                "Paste {\"mcpServers\": {…}} (Claude, Cursor), {\"servers\": {…}} (VS Code), or {\"context_servers\": {…}} (Zed)",
                4,
                window,
                cx,
            ),
            template_fields: Vec::new(),
            skill_name: input("Name, e.g. study-summariser", window, cx),
            skill_desc: input("When Tern should use it, in one line", window, cx),
            skill_body: area("Instructions the model follows (Markdown)", 6, window, cx),
            skill_github: input(
                "https://github.com/owner/repo/tree/main/skills/name",
                window,
                cx,
            ),
        }
    }
}

fn tile(content: AnyElement, p: Palette) -> AnyElement {
    div()
        .size(px(28.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded_lg()
        .border_1()
        .border_color(p.border)
        .bg(p.raised)
        .child(content)
        .into_any_element()
}

/// A reader's logo: Tern Crawl's own mark, the hosted readers' brand
/// marks, and a globe for a plain fetch.
pub(super) fn crawler_logo(c: Crawler, size: f32, p: Palette) -> AnyElement {
    match c {
        Crawler::Tern => gpui_kit::img(assets::CRAWL_LOGO)
            .size(px(size))
            .into_any_element(),
        Crawler::Firecrawl | Crawler::Jina => super::brand_logo(c.id(), size, p),
        Crawler::Fetch => Icon::new(AssetIcon::Globe)
            .small()
            .text_color(super::mono_ink(p))
            .into_any_element(),
    }
}

fn line_view(l: &Line, p: Palette) -> AnyElement {
    let (text, working, failed) = match l {
        Line::Working(t) => (t.clone(), true, false),
        Line::Ok(t) => (t.clone(), false, false),
        Line::Failed(t) => (format!("Failed: {t}"), false, true),
    };
    div()
        .flex()
        .items_center()
        .gap_2()
        .text_xs()
        .text_color(if failed { p.text } else { p.muted })
        .when(working, |d| d.child(Spinner::new().xsmall().color(p.muted)))
        .child(SharedString::from(text))
        .into_any_element()
}

/// Problems listed under a form, one per line.
fn errors_view(errs: &[String], p: Palette) -> Option<AnyElement> {
    (!errs.is_empty()).then(|| {
        div()
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .rounded_md()
            .border_1()
            .border_color(p.border)
            .children(errs.iter().map(|e| {
                div()
                    .flex()
                    .items_start()
                    .gap_2()
                    .text_xs()
                    .text_color(p.text)
                    .child(
                        Icon::new(AssetIcon::TriangleAlert)
                            .xsmall()
                            .text_color(p.accent),
                    )
                    .child(div().flex_1().child(SharedString::from(e.clone())))
            }))
            .into_any_element()
    })
}

fn form_box(p: Palette) -> gpui_kit::Div {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .p_3()
        .rounded_lg()
        .border_1()
        .border_color(p.border)
        .bg(p.raised)
}

fn label(text: &'static str, p: Palette) -> AnyElement {
    div()
        .text_xs()
        .text_color(p.faint)
        .child(text)
        .into_any_element()
}

fn value_of(e: &Entity<InputState>, cx: &Context<Workspace>) -> String {
    e.read(cx).value().trim().to_owned()
}

fn area_of(e: &Entity<TextareaState>, cx: &Context<Workspace>) -> String {
    e.read(cx).value().to_string()
}

/// `Name: value` lines into a map; the first line that is not a pair is
/// the error.
pub(crate) fn header_lines(
    text: &str,
) -> Result<std::collections::BTreeMap<String, String>, String> {
    presets::parse_pairs(text, ':')
}

impl Workspace {
    // ----- messages -----

    pub(super) fn on_setup(&mut self, m: SetupMsg, window: &mut Window, cx: &mut Context<Self>) {
        let st = &mut self.model.settings;
        match m {
            SetupMsg::Crawlers(keys) => st.crawler_keys = keys,
            SetupMsg::CrawlerChecked(c, r) => {
                let ok = r.is_ok();
                st.set_crawler_note(
                    c,
                    match r {
                        Ok(t) => Line::Ok(t),
                        Err(e) => Line::Failed(e),
                    },
                );
                if ok && st.crawler_editing == Some(c) {
                    st.crawler_editing = None;
                    self.setup_inputs
                        .crawler_key
                        .update(cx, |i, cx| i.set_value("", window, cx));
                }
            }
            SetupMsg::CustomSaved(Ok(id)) => {
                st.custom_open = false;
                st.paste_open = false;
                st.custom_errors.clear();
                st.paste_errors.clear();
                self.model.providers.begin(Kind::Model, id, "Checking…");
                for e in [
                    self.setup_inputs.custom_name.clone(),
                    self.setup_inputs.custom_url.clone(),
                    self.setup_inputs.custom_key.clone(),
                ] {
                    e.update(cx, |i, cx| i.set_value("", window, cx));
                }
                for e in [
                    self.setup_inputs.custom_headers.clone(),
                    self.setup_inputs.custom_paste.clone(),
                ] {
                    e.update(cx, |i, cx| i.set_value("", window, cx));
                }
            }
            SetupMsg::CustomSaved(Err(e)) => st.custom_errors = vec![e],
            SetupMsg::CustomRemoved(id, r) => match r {
                Ok(()) => {
                    self.model.toast_info(format!("Removed “{id}”"));
                    if self.model.providers.default_id(Kind::Model).is_none()
                        && !self.model.is_busy()
                    {
                        self.reconnect();
                    }
                }
                Err(e) => self
                    .model
                    .toast_error(format!("Could not remove “{id}”: {e}"), None),
            },
            SetupMsg::LocalModels(statuses, root, warning) => {
                st.local.loaded(statuses, root, warning);
            }
            SetupMsg::LocalProgress(id, done, total) => st.local.progress(&id, done, total),
            SetupMsg::LocalVerifying(id, file) => st.local.verifying(&id, file),
            SetupMsg::LocalDone(id, r, cancelled) => {
                let ok = r.is_ok();
                st.local.finished(&id, r, cancelled);
                if ok {
                    self.model.toast_info(format!("Downloaded {id}"));
                }
                self.reload_local_models();
            }
            SetupMsg::LocalRunFailed(id, e) => {
                // The chat says so too, with the way out (#69).
                self.model.local_failed = st.local.failure(&id, &e);
                st.local.run_failed(
                    &id,
                    format!("Could not run this model: {e}. Try the alternative."),
                );
            }
            SetupMsg::LocalRemoved(id, r) => match r {
                Ok(()) => {
                    st.local.errors.remove(&id);
                    let forget = id.clone();
                    self.save_ui_prefs(move |p| p.local_models.forget(&forget));
                    self.model.toast_info(format!("Deleted {id}"));
                    self.reload_local_models();
                }
                Err(e) => {
                    st.local.errors.insert(id, e);
                }
            },
        }
        cx.notify();
    }

    // ----- Providers: custom providers -----

    fn custom_taken(&self, except: Option<&str>) -> Vec<String> {
        self.model
            .providers
            .prefs
            .custom_providers
            .iter()
            .filter(|c| Some(c.id.as_str()) != except)
            .map(|c| c.id.clone())
            .collect()
    }

    /// Checks the Add custom provider form and saves and tests it.
    fn save_custom(&mut self, cx: &mut Context<Self>) {
        let i = &self.setup_inputs;
        let name = value_of(&i.custom_name, cx);
        let url = value_of(&i.custom_url, cx);
        let key = value_of(&i.custom_key, cx);
        let headers_text = area_of(&i.custom_headers, cx);
        let mut errs = Vec::new();
        let headers = match header_lines(&headers_text) {
            Ok(h) => h,
            Err(e) => {
                errs.push(e);
                Default::default()
            }
        };
        let existing_key = custom::find(&custom::slug(&name)).is_some();
        let p = CustomProvider {
            id: custom::slug(&name),
            name: name.clone(),
            wire: self.model.settings.custom_wire,
            base_url: url,
            headers,
            needs_key: !key.is_empty() || existing_key,
            models: Vec::new(),
        };
        let taken = self.custom_taken(Some(&p.id));
        let taken: Vec<&str> = taken.iter().map(String::as_str).collect();
        errs.extend(custom::validate(&p, &taken));
        if !errs.is_empty() {
            self.model.settings.custom_errors = errs;
            cx.notify();
            return;
        }
        self.model.settings.custom_errors = vec![];
        self.bridge
            .custom_save(p, (!key.is_empty()).then(|| SecretString::new(key)));
        cx.notify();
    }

    /// Reads the Paste config box; every provider in it is saved and
    /// tested, or nothing is when any is wrong.
    fn paste_custom(&mut self, cx: &mut Context<Self>) {
        let text = area_of(&self.setup_inputs.custom_paste, cx);
        let taken = self.custom_taken(None);
        let taken: Vec<&str> = taken.iter().map(String::as_str).collect();
        match custom::parse_config(&text, &taken) {
            Err(errs) => self.model.settings.paste_errors = errs,
            Ok(list) => {
                self.model.settings.paste_errors.clear();
                for parsed in list {
                    self.bridge.custom_save(
                        parsed.provider,
                        parsed
                            .key
                            .filter(|k| !k.trim().is_empty())
                            .map(SecretString::new),
                    );
                }
            }
        }
        cx.notify();
    }

    /// Below the provider list: Add custom provider and Paste config.
    pub(super) fn custom_provider_block(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let st = &self.model.settings;
        let buttons = div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                Button::new("custom-add")
                    .capsule()
                    .icon(AssetIcon::Plus)
                    .label("Add custom provider")
                    .small()
                    .selected(st.custom_open)
                    .on_click(cx.listener(|this, _, _, cx| {
                        let s = &mut this.model.settings;
                        s.custom_open = !s.custom_open;
                        s.paste_open = false;
                        cx.notify();
                    })),
            )
            .child(
                Button::new("custom-paste")
                    .capsule()
                    .icon(AssetIcon::ClipboardPaste)
                    .label("Paste config")
                    .ghost()
                    .small()
                    .selected(st.paste_open)
                    .on_click(cx.listener(|this, _, _, cx| {
                        let s = &mut this.model.settings;
                        s.paste_open = !s.paste_open;
                        s.custom_open = false;
                        cx.notify();
                    })),
            );
        let mut col = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(label(
                "Any server that speaks the OpenAI or Anthropic API: vLLM, llama.cpp, LiteLLM, a company proxy.",
                p,
            ))
            .child(buttons);
        if st.custom_open {
            let wire = st.custom_wire;
            let wire_btn = |w: CustomWire, cx: &mut Context<Self>| {
                Button::new(SharedString::from(format!("custom-wire-{w:?}")))
                    .capsule()
                    .label(w.label())
                    .ghost()
                    .xsmall()
                    .selected(wire == w)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.model.settings.custom_wire = w;
                        cx.notify();
                    }))
            };
            col = col.child(
                form_box(p)
                    .child(label("Type", p))
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .child(wire_btn(CustomWire::OpenAi, cx))
                            .child(wire_btn(CustomWire::Anthropic, cx)),
                    )
                    .child(Input::new(&self.setup_inputs.custom_name).small())
                    .child(Input::new(&self.setup_inputs.custom_url).small())
                    .child(Input::new(&self.setup_inputs.custom_key).small().mask_toggle())
                    .child(Textarea::new(&self.setup_inputs.custom_headers))
                    .children(errors_view(&st.custom_errors, p))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                Button::new("custom-save").capsule()
                                    .label("Save and test")
                                    .small()
                                    .on_click(cx.listener(|this, _, _, cx| this.save_custom(cx))),
                            )
                            .child(
                                Button::new("custom-cancel").capsule()
                                    .label("Cancel")
                                    .ghost()
                                    .small()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        let s = &mut this.model.settings;
                                        s.custom_open = false;
                                        s.custom_errors.clear();
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(label(
                        "The key is tested before it is saved, and kept only in the system keychain. After it works, the row's Use lists its models.",
                        p,
                    )),
            );
        }
        if st.paste_open {
            col = col.child(
                form_box(p)
                    .child(Textarea::new(&self.setup_inputs.custom_paste))
                    .children(errors_view(&st.paste_errors, p))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                Button::new("custom-paste-go")
                                    .capsule()
                                    .label("Check and add")
                                    .small()
                                    .on_click(cx.listener(|this, _, _, cx| this.paste_custom(cx))),
                            )
                            .child(
                                Button::new("custom-paste-cancel")
                                    .capsule()
                                    .label("Cancel")
                                    .ghost()
                                    .small()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        let s = &mut this.model.settings;
                                        s.paste_open = false;
                                        s.paste_errors.clear();
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(label(
                        "A key in the config goes to the keychain, never to Tern's settings file.",
                        p,
                    )),
            );
        }
        col.into_any_element()
    }

    // ----- Search & Crawlers -----

    /// Measures the page cache (on opening the page and after a clear).
    pub(super) fn measure_cache(&mut self) {
        let size = tern_agent::platform::AppPaths::discover()
            .ok()
            .map(|a| super::settings_view::dir_size(&a.crawl_cache()))
            .unwrap_or(0);
        self.model.settings.cache_size = Some(size);
    }

    fn set_default_crawler(&mut self, c: Crawler, cx: &mut Context<Self>) {
        self.save_ui_prefs(|p| {
            p.crawler = (c != Crawler::Tern).then(|| c.id().to_owned());
            p.crawlers_off.retain(|o| o != c.id());
        });
        if c.needs_key() && !self.model.settings.crawler_has_key(c) {
            self.model.settings.set_crawler_note(
                c,
                Line::Failed("Add a key; until then Tern Crawl reads pages instead.".into()),
            );
        }
        cx.notify();
    }

    fn save_crawler_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(c) = self.model.settings.crawler_editing else {
            return;
        };
        let key = value_of(&self.setup_inputs.crawler_key, cx);
        if key.is_empty() {
            return;
        }
        self.model
            .settings
            .set_crawler_note(c, Line::Working("Reading a page with the key…".into()));
        self.bridge.crawler_add_key(c, SecretString::new(key));
        self.setup_inputs
            .crawler_key
            .update(cx, |i, cx| i.set_value("", window, cx));
        cx.notify();
    }

    /// The Crawlers group: each page reader with its logo, a line on what
    /// it is, its key and Test, an enable switch, and Default.
    pub(super) fn crawlers_card(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let prefs = &self.model.providers.prefs;
        let st = &self.model.settings;
        let saved_default = prefs
            .crawler
            .as_deref()
            .and_then(Crawler::parse)
            .unwrap_or_default();
        let rows: Vec<AnyElement> = Crawler::ALL
            .into_iter()
            .map(|c| {
                let id = c.id();
                let on = c == Crawler::Tern || !prefs.crawlers_off.iter().any(|o| o == id);
                let is_default = saved_default == c;
                let has_key = st.crawler_has_key(c);
                let editing = st.crawler_editing == Some(c);
                let working = matches!(st.crawler_note(c), Some(Line::Working(_)));
                let status = if c.takes_key() {
                    if has_key {
                        "Key saved"
                    } else if c.needs_key() {
                        "Needs a key"
                    } else {
                        "Works without a key"
                    }
                } else {
                    "No key needed"
                };
                let bid = |what: &str| SharedString::from(format!("crawler-{what}-{id}"));
                let mut actions = div().flex().items_center().gap_1().flex_none();
                if working {
                    actions = actions.child(Spinner::new().small().color(p.muted));
                }
                if c.takes_key() && !editing {
                    actions = actions.child(
                        Button::new(bid("key"))
                            .capsule()
                            .label(if has_key { "Replace key" } else { "Add key" })
                            .ghost()
                            .small()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.model.settings.crawler_editing = Some(c);
                                this.setup_inputs.crawler_key.update(cx, |i, cx| {
                                    i.set_value("", window, cx);
                                    i.focus(window, cx);
                                });
                                cx.notify();
                            })),
                    );
                }
                if has_key {
                    actions = actions.child(
                        Button::new(bid("rmkey"))
                            .capsule()
                            .label("Remove key")
                            .ghost()
                            .small()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.model
                                    .settings
                                    .set_crawler_note(c, Line::Working("Removing…".into()));
                                this.bridge.crawler_remove_key(c);
                                cx.notify();
                            })),
                    );
                }
                actions = actions
                    .child(
                        Button::new(bid("test"))
                            .capsule()
                            .label("Test")
                            .ghost()
                            .small()
                            .disabled(working)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.model.settings.set_crawler_note(
                                    c,
                                    Line::Working("Reading example.com…".into()),
                                );
                                this.bridge.crawler_test(c);
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new(bid("default"))
                            .capsule()
                            .label("Default")
                            .ghost()
                            .small()
                            .selected(is_default)
                            .disabled(!on)
                            .tooltip("Read every page with this one")
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.set_default_crawler(c, cx)),
                            ),
                    )
                    .child(
                        Switch::new(bid("on"))
                            .checked(on)
                            .small()
                            .disabled(c == Crawler::Tern)
                            .tooltip(if c == Crawler::Tern {
                                "Tern Crawl is always there as the fallback"
                            } else {
                                "Off: never used, even as the default"
                            })
                            .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                                let checked = *checked;
                                this.save_ui_prefs(|p| {
                                    p.crawlers_off.retain(|o| o != id);
                                    if !checked {
                                        p.crawlers_off.push(id.to_owned());
                                    }
                                });
                                cx.notify();
                            })),
                    );
                let mut info = div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(c.name()),
                            )
                            .when(is_default, |d| {
                                d.child(
                                    div()
                                        .px_1p5()
                                        .rounded_md()
                                        .border_1()
                                        .border_color(p.border)
                                        .text_xs()
                                        .text_color(p.muted)
                                        .child("Default"),
                                )
                            })
                            .child(div().text_xs().text_color(p.faint).child(status)),
                    )
                    .child(div().text_xs().text_color(p.faint).child(c.blurb()));
                if let Some(l) = st.crawler_note(c) {
                    info = info.child(line_view(l, p));
                }
                let key_url = c.key_url();
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .px_3()
                    .py_2p5()
                    .border_b_1()
                    .border_color(p.border)
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .gap_3()
                            .child(tile(crawler_logo(c, 16., p), p))
                            .child(info),
                    )
                    .child(div().pl(px(40.)).flex().flex_wrap().child(actions))
                    .when(editing, |d| {
                        d.child(
                            div()
                                .pl(px(40.))
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(div().flex_1().child(
                                    Input::new(&self.setup_inputs.crawler_key).mask_toggle(),
                                ))
                                .child(
                                    Button::new(bid("save"))
                                        .capsule()
                                        .label("Test and save")
                                        .small()
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.save_crawler_key(window, cx)
                                        })),
                                )
                                .when(!key_url.is_empty(), |d| {
                                    d.child(
                                        Button::new(bid("get"))
                                            .capsule()
                                            .label("Get a key")
                                            .ghost()
                                            .small()
                                            .on_click(move |_, _, cx| cx.open_url(key_url)),
                                    )
                                })
                                .child(
                                    Button::new(bid("cancel"))
                                        .capsule()
                                        .label("Cancel")
                                        .ghost()
                                        .small()
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.model.settings.crawler_editing = None;
                                            cx.notify();
                                        })),
                                ),
                        )
                    })
                    .into_any_element()
            })
            .collect();
        Self::settings_card("Crawlers", p, rows)
    }

    // ----- Instructions -----

    pub(super) fn open_instructions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_settings(Section::Instructions);
        let saved = tern_agent::Preferences::load().system_prompts;
        for scope in Scope::ALL {
            let text = saved.get(scope.key()).cloned().unwrap_or_default();
            self.prompt_input(scope)
                .update(cx, |i, cx| i.set_value(text, window, cx));
        }
    }

    fn prompt_input(&self, scope: Scope) -> Entity<TextareaState> {
        match scope {
            Scope::Global => self.setup_inputs.prompt_global.clone(),
            Scope::Research => self.setup_inputs.prompt_research.clone(),
            Scope::Study => self.setup_inputs.prompt_study.clone(),
        }
    }

    fn save_prompt(
        &mut self,
        scope: Scope,
        reset: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = self.prompt_input(scope);
        let text = if reset {
            input.update(cx, |i, cx| i.set_value("", window, cx));
            String::new()
        } else {
            input.read(cx).value().to_string()
        };
        self.save_ui_prefs(|p| prompts::save(&mut p.system_prompts, scope, &text));
        prompts::set_user(&self.model.providers.prefs.system_prompts);
        self.model.toast_info(if reset {
            format!("{} instructions back to Tern's defaults", scope.label())
        } else {
            format!(
                "Saved your {} instructions; new chats use them",
                scope.label()
            )
        });
        cx.notify();
    }

    pub(super) fn instructions_page(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let saved = &self.model.providers.prefs.system_prompts;
        let editors: Vec<AnyElement> =
            Scope::ALL
                .into_iter()
                .map(|scope| {
                    let has = saved.contains_key(scope.key());
                    let input = self.prompt_input(scope);
                    let n = input.read(cx).value().chars().count();
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .px_3()
                        .py_3()
                        .border_b_1()
                        .border_color(p.border)
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(FontWeight::MEDIUM)
                                        .child(scope.label()),
                                )
                                .child(div().text_xs().text_color(p.faint).child(
                                    SharedString::from(format!(
                                        "{} · {n}/{}",
                                        if has { "Yours" } else { "Tern's defaults" },
                                        prompts::MAX_CHARS
                                    )),
                                )),
                        )
                        .child(Textarea::new(&input))
                        .child(
                            div()
                                .flex()
                                .gap_2()
                                .child(
                                    Button::new(SharedString::from(format!(
                                        "prompt-save-{}",
                                        scope.key()
                                    )))
                                    .capsule()
                                    .label("Save")
                                    .small()
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.save_prompt(scope, false, window, cx)
                                    })),
                                )
                                .child(
                                    Button::new(SharedString::from(format!(
                                        "prompt-reset-{}",
                                        scope.key()
                                    )))
                                    .capsule()
                                    .label("Reset to default")
                                    .ghost()
                                    .small()
                                    .disabled(!has)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.save_prompt(scope, true, window, cx)
                                    })),
                                ),
                        )
                        .into_any_element()
                })
                .collect();
        let mut builtins: Vec<AnyElement> = Vec::new();
        for scope in [Scope::Research, Scope::Study] {
            for (k, b) in prompts::builtins(scope).into_iter().enumerate() {
                let text = b.text.clone();
                builtins.push(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .px_3()
                        .py_2p5()
                        .border_b_1()
                        .border_color(p.border)
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap_2()
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .min_w_0()
                                        .child(div().text_sm().child(SharedString::from(format!(
                                            "{} · {}",
                                            scope.label(),
                                            b.title
                                        ))))
                                        .child(div().text_xs().text_color(p.faint).child(b.when)),
                                )
                                .child(
                                    Button::new(SharedString::from(format!(
                                        "builtin-copy-{}-{k}",
                                        scope.key()
                                    )))
                                    .capsule()
                                    .icon(AssetIcon::Copy)
                                    .label("Copy")
                                    .ghost()
                                    .xsmall()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(
                                            text.clone(),
                                        ));
                                        this.model.toast_info("Copied");
                                        cx.notify();
                                    })),
                                ),
                        )
                        .child(
                            div()
                                .max_h(px(160.))
                                .overflow_hidden()
                                .p_2()
                                .rounded_md()
                                .bg(p.raised)
                                .text_xs()
                                .font_family(assets::MONO_FONT)
                                .text_color(p.muted)
                                .child(SharedString::from(b.text)),
                        )
                        .into_any_element(),
                );
            }
        }
        let note = div()
            .flex()
            .items_center()
            .justify_between()
            .gap_3()
            .text_xs()
            .text_color(p.faint)
            .child(div().flex_1().child(
                "Yours are added after Tern's own prompt, never instead of it. A project's TERN.md and its instructions still apply on top. The full text a chat sends is under MCP & Skills › Instructions.",
            ))
            .child(
                Button::new("instructions-effective").capsule()
                    .label("See what is sent")
                    .ghost()
                    .small()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_settings(Section::Extensions, window, cx);
                        this.set_customize_tab(crate::extensions::CustomizeTab::Instructions, cx);
                    })),
            )
            .into_any_element();
        self.page(
            Section::Instructions,
            p,
            None,
            vec![
                Self::settings_card("Your instructions", p, editors),
                note,
                Self::settings_card("Built in (read-only)", p, builtins),
            ],
        )
    }

    // ----- MCP: templates, Add, Paste -----

    fn open_mcp_template(&mut self, id: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(t) = presets::template(id) else {
            return;
        };
        let st = &mut self.model.settings;
        st.mcp_errors.clear();
        if st.mcp_template == Some(id) {
            st.mcp_template = None;
            cx.notify();
            return;
        }
        st.mcp_template = Some(id);
        st.mcp_paste_open = false;
        self.model.customize.adding = false;
        self.setup_inputs.template_fields = t
            .needs
            .iter()
            .map(|n| {
                let e = cx.new(|cx| {
                    InputState::new(window, cx)
                        .masked(n.secret)
                        .placeholder(SharedString::from(format!("{} ({})", n.key, n.hint)))
                });
                (n.key, e)
            })
            .collect();
        cx.notify();
    }

    fn add_mcp_template(&mut self, cx: &mut Context<Self>) {
        let Some(t) = self.model.settings.mcp_template.and_then(presets::template) else {
            return;
        };
        let values: std::collections::BTreeMap<String, String> = self
            .setup_inputs
            .template_fields
            .iter()
            .map(|(k, e)| ((*k).to_owned(), value_of(e, cx)))
            .collect();
        match t.config(&values) {
            Ok(cfg) => {
                let name = t.id.to_owned();
                self.bridge.mcp_change(McpChange::Add(name, Box::new(cfg)));
                let st = &mut self.model.settings;
                st.mcp_template = None;
                st.mcp_errors.clear();
                self.setup_inputs.template_fields.clear();
            }
            Err(e) => self.model.settings.mcp_errors = vec![e],
        }
        cx.notify();
    }

    /// The Add server form: a command line (stdio) or a URL (HTTP), with
    /// environment variables or headers.
    pub(super) fn add_mcp_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let i = &self.setup_inputs;
        let name = value_of(&i.mcp_name, cx);
        let target = value_of(&i.mcp_target, cx);
        let pairs = area_of(&i.mcp_pairs, cx);
        let mut errs = Vec::new();
        if name.is_empty() {
            errs.push("Give the server a name.".to_owned());
        }
        if target.is_empty() {
            errs.push("Give its command (with arguments) or its URL.".to_owned());
        }
        let cfg = if errs.is_empty() {
            presets::from_form(&target, &pairs)
                .map_err(|e| errs.push(e))
                .ok()
        } else {
            None
        };
        match cfg {
            Some(cfg) if errs.is_empty() => {
                self.bridge.mcp_change(McpChange::Add(name, Box::new(cfg)));
                self.model.customize.adding = false;
                self.model.settings.mcp_errors.clear();
                for e in [
                    self.setup_inputs.mcp_name.clone(),
                    self.setup_inputs.mcp_target.clone(),
                ] {
                    e.update(cx, |i, cx| i.set_value("", window, cx));
                }
                self.setup_inputs
                    .mcp_pairs
                    .update(cx, |i, cx| i.set_value("", window, cx));
            }
            _ => self.model.settings.mcp_errors = errs,
        }
        cx.notify();
    }

    fn paste_mcp(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = area_of(&self.setup_inputs.mcp_paste, cx);
        match presets::parse_pasted(&text) {
            Err(errs) => self.model.settings.mcp_errors = errs,
            Ok(list) => {
                for (name, cfg) in list {
                    self.bridge.mcp_change(McpChange::Add(name, Box::new(cfg)));
                }
                let st = &mut self.model.settings;
                st.mcp_errors.clear();
                st.mcp_paste_open = false;
                self.setup_inputs
                    .mcp_paste
                    .update(cx, |i, cx| i.set_value("", window, cx));
            }
        }
        cx.notify();
    }

    /// The templates gallery, then the open template's form, the Add
    /// server form, or the Paste box.
    pub(super) fn mcp_extras(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let st = &self.model.settings;
        let adding = self.model.customize.adding;
        let have = &self.model.customize.mcp.servers;
        let cards = presets::TEMPLATES.iter().map(|t| {
            let id = t.id;
            let added = have.contains_key(id);
            let open = st.mcp_template == Some(id);
            div()
                .id(SharedString::from(format!("mcp-tpl-{id}")))
                .w(px(200.))
                .flex()
                .items_start()
                .gap_2()
                .p_2()
                .rounded_lg()
                .border_1()
                .border_color(if open { p.accent } else { p.border })
                .cursor_pointer()
                .hover(|d| d.bg(p.hover))
                .child(tile(super::brand_logo(t.logo, 16., p), p))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .min_w_0()
                        .flex_1()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1()
                                .child(div().text_sm().child(t.name))
                                .when(added, |d| {
                                    d.child(div().text_xs().text_color(p.faint).child("· added"))
                                }),
                        )
                        .child(div().text_xs().text_color(p.faint).child(t.blurb)),
                )
                .when(!added, |d| {
                    d.on_click(cx.listener(move |this, _, window, cx| {
                        this.open_mcp_template(id, window, cx)
                    }))
                })
        });
        let mut col = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(label("Templates", p))
            .child(div().flex().flex_wrap().gap_2().children(cards))
            .child(
                div().flex().gap_2().child(
                    Button::new("mcp-paste")
                        .capsule()
                        .icon(AssetIcon::ClipboardPaste)
                        .label("Paste JSON")
                        .ghost()
                        .small()
                        .selected(st.mcp_paste_open)
                        .on_click(cx.listener(|this, _, _, cx| {
                            let s = &mut this.model.settings;
                            s.mcp_paste_open = !s.mcp_paste_open;
                            s.mcp_template = None;
                            s.mcp_errors.clear();
                            this.model.customize.adding = false;
                            cx.notify();
                        })),
                ),
            );
        if let Some(t) = st.mcp_template.and_then(presets::template) {
            let docs = t.docs;
            col = col.child(
                form_box(p)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(super::brand_logo(t.logo, 16., p))
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(t.name),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .font_family(assets::MONO_FONT)
                            .text_color(p.faint)
                            .child(SharedString::from(match (t.command, t.url) {
                                (Some(c), _) => format!("{c} {}", t.args.join(" ")),
                                (None, Some(u)) => u.to_owned(),
                                _ => String::new(),
                            })),
                    )
                    .children(
                        self.setup_inputs
                            .template_fields
                            .iter()
                            .map(|(_, e)| Input::new(e).small()),
                    )
                    .children(errors_view(&st.mcp_errors, p))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                Button::new("mcp-tpl-add")
                                    .capsule()
                                    .label("Add")
                                    .small()
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.add_mcp_template(cx)),
                                    ),
                            )
                            .child(
                                Button::new("mcp-tpl-docs")
                                    .capsule()
                                    .label("Docs")
                                    .ghost()
                                    .small()
                                    .on_click(move |_, _, cx| cx.open_url(docs)),
                            ),
                    )
                    .child(label(
                        "Added servers stay off until you review their tools in the trust preview.",
                        p,
                    )),
            );
        }
        if adding {
            col = col.child(
                form_box(p)
                    .child(Input::new(&self.setup_inputs.mcp_name).small())
                    .child(Input::new(&self.setup_inputs.mcp_target).small())
                    .child(Textarea::new(&self.setup_inputs.mcp_pairs))
                    .children(errors_view(&st.mcp_errors, p))
                    .child(
                        Button::new("mcp-add-go").capsule()
                            .label("Add")
                            .small()
                            .on_click(cx.listener(|this, _, window, cx| this.add_mcp_form(window, cx))),
                    )
                    .child(label(
                        "A URL must be https unless it is on this computer. Added servers stay off until you review them.",
                        p,
                    )),
            );
        }
        if st.mcp_paste_open {
            col = col.child(
                form_box(p)
                    .child(Textarea::new(&self.setup_inputs.mcp_paste))
                    .children(errors_view(&st.mcp_errors, p))
                    .child(
                        Button::new("mcp-paste-go")
                            .capsule()
                            .label("Check and add")
                            .small()
                            .on_click(
                                cx.listener(|this, _, window, cx| this.paste_mcp(window, cx)),
                            ),
                    ),
            );
        }
        col.into_any_element()
    }

    // ----- Skills: editor, GitHub, templates -----

    fn save_new_skill(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let i = &self.setup_inputs;
        let name = value_of(&i.skill_name, cx);
        let desc = value_of(&i.skill_desc, cx);
        let body = area_of(&i.skill_body, cx);
        let errs = skill_author::check_new(&name, &desc, &body);
        if !errs.is_empty() {
            self.model.settings.skill_errors = errs;
            cx.notify();
            return;
        }
        self.bridge
            .skills_change(SkillChange::Create(name, desc, body));
        let st = &mut self.model.settings;
        st.skill_errors.clear();
        st.skill_new_open = false;
        for e in [
            self.setup_inputs.skill_name.clone(),
            self.setup_inputs.skill_desc.clone(),
        ] {
            e.update(cx, |i, cx| i.set_value("", window, cx));
        }
        self.setup_inputs
            .skill_body
            .update(cx, |i, cx| i.set_value("", window, cx));
        cx.notify();
    }

    fn use_skill_template(&mut self, k: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(t) = skill_author::TEMPLATES.get(k) else {
            return;
        };
        self.model.settings.skill_new_open = true;
        self.model.settings.skill_github_open = false;
        self.setup_inputs
            .skill_name
            .update(cx, |i, cx| i.set_value(t.name, window, cx));
        self.setup_inputs
            .skill_desc
            .update(cx, |i, cx| i.set_value(t.description, window, cx));
        self.setup_inputs
            .skill_body
            .update(cx, |i, cx| i.set_value(t.body, window, cx));
        cx.notify();
    }

    fn import_skill(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let url = value_of(&self.setup_inputs.skill_github, cx);
        match skill_author::GithubRef::parse(&url) {
            Err(e) => self.model.settings.skill_errors = vec![e],
            Ok(r) => {
                self.model.customize.note = Some(crate::extensions::Note::Working(format!(
                    "Downloading {}…",
                    r.label()
                )));
                self.bridge.skills_import_github(r);
                let st = &mut self.model.settings;
                st.skill_errors.clear();
                st.skill_github_open = false;
                self.setup_inputs
                    .skill_github
                    .update(cx, |i, cx| i.set_value("", window, cx));
            }
        }
        cx.notify();
    }

    /// New skill, Import from GitHub, and templates, above the list.
    pub(super) fn skill_extras(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let st = &self.model.settings;
        let buttons = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .child(
                Button::new("skill-new")
                    .capsule()
                    .icon(AssetIcon::Plus)
                    .label("New skill")
                    .small()
                    .selected(st.skill_new_open)
                    .on_click(cx.listener(|this, _, _, cx| {
                        let s = &mut this.model.settings;
                        s.skill_new_open = !s.skill_new_open;
                        s.skill_github_open = false;
                        s.skill_errors.clear();
                        cx.notify();
                    })),
            )
            .child(
                Button::new("skill-github")
                    .capsule()
                    .icon(AssetIcon::Github)
                    .label("Import from GitHub")
                    .ghost()
                    .small()
                    .selected(st.skill_github_open)
                    .on_click(cx.listener(|this, _, _, cx| {
                        let s = &mut this.model.settings;
                        s.skill_github_open = !s.skill_github_open;
                        s.skill_new_open = false;
                        s.skill_errors.clear();
                        cx.notify();
                    })),
            );
        let templates = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_1()
            .child(label("Start from:", p))
            .children(skill_author::TEMPLATES.iter().enumerate().map(|(k, t)| {
                Button::new(("skill-tpl", k))
                    .capsule()
                    .label(t.name)
                    .ghost()
                    .xsmall()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.use_skill_template(k, window, cx)
                    }))
            }));
        let mut col = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(buttons)
            .child(templates);
        if st.skill_new_open {
            col = col.child(
                form_box(p)
                    .child(Input::new(&self.setup_inputs.skill_name).small())
                    .child(Input::new(&self.setup_inputs.skill_desc).small())
                    .child(Textarea::new(&self.setup_inputs.skill_body))
                    .children(errors_view(&st.skill_errors, p))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                Button::new("skill-new-save").capsule()
                                    .label("Save skill")
                                    .small()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.save_new_skill(window, cx)
                                    })),
                            )
                            .child(
                                Button::new("skill-new-cancel").capsule()
                                    .label("Cancel")
                                    .ghost()
                                    .small()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.model.settings.skill_new_open = false;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(label(
                        "Saved as skills/<name>/SKILL.md in Tern's data folder. Only the name and description are in the prompt until a task needs the rest.",
                        p,
                    )),
            );
        }
        if st.skill_github_open {
            col = col.child(
                form_box(p)
                    .child(Input::new(&self.setup_inputs.skill_github).small())
                    .children(errors_view(&st.skill_errors, p))
                    .child(
                        Button::new("skill-github-go").capsule()
                            .label("Import")
                            .small()
                            .on_click(cx.listener(|this, _, window, cx| this.import_skill(window, cx))),
                    )
                    .child(label(
                        "A folder with a SKILL.md (or a repo of them). Files are read through GitHub's API; nothing runs.",
                        p,
                    )),
            );
        }
        col.into_any_element()
    }
}
