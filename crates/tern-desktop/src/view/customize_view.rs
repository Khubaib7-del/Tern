//! Customize (Skills, MCP servers, Providers, and the effective
//! instructions), the MCP trust preview sheet with health dots, and the
//! sidebar's project folders: Projects → sessions → ↳ sub-sessions, with
//! the Inbox for chats in no project, drag a chat onto a folder (or Move
//! to…), and collapse state that is saved.

use gpui_kit::assets::IconName as AssetIcon;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{Disableable as _, Selectable as _, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, Context, Entity, FontWeight, Hsla, Render, SharedString, Subscription, Window, div,
    px,
};
use tern_agent::mcp::{HealthState, ServerConfig, Severity, ToolSetting, TrustPreview};
use tern_agent::policy::Mode;
use tern_core::{ProjectId, SessionId};

use super::Workspace;
use super::press::Capsule as _;
use crate::assets::MONO_FONT;
use crate::bridge::{ExtMsg, McpChange, ProjectChange, SkillChange};
use crate::extensions::{CustomizeTab, Note, ServerGlyph, TreeRow, server_glyph};
use crate::theme::Palette;

const PAGE_MAX_W: f32 = 820.;

/// A chat being dragged onto a project folder.
#[derive(Clone, Debug)]
pub struct DraggedChat {
    pub id: SessionId,
    pub title: String,
    pub palette: Palette,
}

impl Render for DraggedChat {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.palette;
        div()
            .px_2()
            .py_1()
            .max_w(px(220.))
            .truncate()
            .rounded_md()
            .border_1()
            .border_color(p.border)
            .bg(p.raised)
            .shadow_md()
            .text_sm()
            .text_color(p.text)
            .child(SharedString::from(self.title.clone()))
    }
}

/// The text fields on Customize and in the sidebar's project tree.
#[derive(Debug)]
pub struct ExtInputs {
    /// A new project's name, or a project being renamed.
    pub project: Entity<InputState>,
}

impl ExtInputs {
    pub fn new(window: &mut Window, cx: &mut Context<Workspace>) -> (Self, Vec<Subscription>) {
        let project = cx.new(|cx| InputState::new(window, cx).placeholder("Project name"));
        let sub = cx.subscribe_in(
            &project,
            window,
            |this: &mut Workspace, _, e: &InputEvent, _, cx| match e {
                InputEvent::PressEnter { .. } => this.finish_project_edit(cx),
                InputEvent::Blur => {
                    this.model.projects.editing = None;
                    cx.notify();
                }
                _ => {}
            },
        );
        (Self { project }, vec![sub])
    }
}

fn icon(name: AssetIcon, color: Hsla) -> AnyElement {
    gpui_kit::component::Icon::new(name)
        .small()
        .text_color(color)
        .into_any_element()
}

/// The square tile at the start of a row: a service logo or a glyph.
fn tile(content: AnyElement, p: Palette) -> AnyElement {
    div()
        .size(px(28.))
        .flex_shrink_0()
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

fn server_tile(name: &str, s: &ServerConfig, p: Palette) -> AnyElement {
    let http = s.url.is_some() && s.command.is_none();
    // A server named for a brand (a template's id) gets its coloured mark.
    if matches!(crate::brands::logo(name), crate::brands::Logo::Mark(..)) {
        return tile(super::brand_logo(name, 15., p), p);
    }
    let inner = match server_glyph(name, &s.target(), http) {
        ServerGlyph::Service(k) => super::service_logo(k, 15., p, p.text),
        ServerGlyph::Terminal => icon(AssetIcon::SquareTerminal, p.text),
        ServerGlyph::Globe => icon(AssetIcon::Globe, p.text),
    };
    tile(inner, p)
}

/// The health dot: filled for working, half for slow, a cross for down,
/// a ring for off or waiting for review. Colour stays in the palette.
fn dot(state: HealthState, p: Palette) -> AnyElement {
    let base = div().size(px(8.)).flex_none().rounded_full();
    match state {
        HealthState::Ok => base.bg(p.accent).into_any_element(),
        HealthState::Slow => base
            .border_1()
            .border_color(p.accent)
            .bg(p.accent.opacity(0.35))
            .into_any_element(),
        HealthState::Down => div()
            .flex_none()
            .text_xs()
            .text_color(p.text)
            .child("✕")
            .into_any_element(),
        HealthState::Off | HealthState::NeedsReview => {
            base.border_1().border_color(p.faint).into_any_element()
        }
    }
}

fn mode_label(m: Mode) -> &'static str {
    match m {
        Mode::Ask => "Ask",
        Mode::Allow => "Allow",
        Mode::Deny => "Refuse",
    }
}

fn flag_line(reason: &str, excerpt: &str, danger: bool, p: Palette) -> AnyElement {
    div()
        .flex()
        .items_start()
        .gap_1p5()
        .text_xs()
        .text_color(if danger { p.text } else { p.muted })
        .child(icon(
            if danger {
                AssetIcon::TriangleAlert
            } else {
                AssetIcon::Info
            },
            if danger { p.text } else { p.faint },
        ))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(SharedString::from(if excerpt.is_empty() {
                    reason.to_owned()
                } else {
                    format!("{reason}: “{excerpt}”")
                })),
        )
        .into_any_element()
}

impl Workspace {
    // ----- Customize -----

    pub(super) fn open_customize(&mut self, cx: &mut Context<Self>) {
        self.show_settings(crate::settings::Section::Extensions);
        if !self.model.customize.loaded {
            self.bridge.ext_load();
        }
        if self.model.customize.tab == CustomizeTab::Instructions {
            self.refresh_instructions(cx);
        }
        cx.notify();
    }

    pub(super) fn set_customize_tab(&mut self, tab: CustomizeTab, cx: &mut Context<Self>) {
        self.model.customize.tab = tab;
        match tab {
            CustomizeTab::Providers if !self.model.providers.loaded => self.bridge.load_providers(),
            CustomizeTab::Instructions => self.refresh_instructions(cx),
            _ => {}
        }
        cx.notify();
    }

    /// The open chat's instructions come from its session (exactly what
    /// its next turn sends); with no chat open, a preview for a new one.
    fn refresh_instructions(&mut self, cx: &mut Context<Self>) {
        let c = &mut self.model.customize;
        c.open_pieces.clear();
        match &self.session {
            Some(s) => {
                c.instructions = Some(s.effective_instructions());
                c.instructions_for_chat = true;
                c.loading_instructions = false;
            }
            None => {
                c.loading_instructions = true;
                c.instructions_for_chat = false;
                self.bridge.instructions(self.model.projects.new_in.clone());
            }
        }
        cx.notify();
    }

    pub(super) fn on_ext(&mut self, m: ExtMsg, cx: &mut Context<Self>) {
        let note = |n: Option<String>| {
            n.map(|n| match n.strip_prefix("! ") {
                Some(e) => Note::Failed(e.to_owned()),
                None => Note::Ok(n),
            })
        };
        if matches!(m, ExtMsg::Loaded(..)) {
            // Servers or their switches may have changed.
            self.bridge.mcp_menu();
        }
        let c = &mut self.model.customize;
        match m {
            ExtMsg::Loaded(r, n) => {
                c.loaded = true;
                match r {
                    Ok((mcp, skills)) => {
                        c.mcp = mcp;
                        c.skills = skills;
                    }
                    Err(e) => c.note = Some(Note::Failed(e)),
                }
                if n.is_some() {
                    c.note = note(n);
                }
                c.skill_preview = None;
            }
            ExtMsg::Menu(m) => {
                self.model.mcp_offers = m.items;
            }
            ExtMsg::Resource(r) => self.model.resource_ready(*r),
            // Handled with the window in `on_msg`.
            ExtMsg::Prompt(_) => {}
            ExtMsg::Health(h) => c.checked(h),
            ExtMsg::Preview(p) => {
                c.previewing = None;
                c.previews.insert(p.server.clone(), (*p).clone());
                c.preview = Some(*p);
            }
            ExtMsg::SkillPreview(r) => match r {
                Ok(p) => c.skill_preview = Some(p),
                Err(e) => c.note = Some(Note::Failed(e)),
            },
            ExtMsg::Instructions(r) => {
                c.loading_instructions = false;
                match r {
                    Ok(e) => c.instructions = Some(e),
                    Err(e) => c.note = Some(Note::Failed(e)),
                }
            }
            // Handled with the window (`Workspace::folder_opened`).
            ExtMsg::Folder(_) => {}
            ExtMsg::Projects(r, recent, n) => {
                let t = &mut self.model.projects;
                match r {
                    Ok(list) => {
                        t.projects = list;
                        t.loaded = true;
                    }
                    Err(e) => self.model.customize.note = Some(Note::Failed(e)),
                }
                if !recent.is_empty() {
                    self.model.set_sessions(recent);
                }
                if let Some(n) = note(n) {
                    self.model.notice_line(match n {
                        Note::Ok(t) | Note::Working(t) => t,
                        Note::Failed(e) => format!("Tern could not do that: {e}"),
                    });
                }
            }
        }
        cx.notify();
    }

    fn pick_skill_folder(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Install skills from".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(mut paths))) = picked.await
                && let Some(dir) = paths.pop()
            {
                let _ = this.update(cx, |this, cx| {
                    this.model.customize.note = Some(Note::Working("Reading the skills…".into()));
                    this.bridge.skills_preview(dir);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn preview_server(&mut self, name: String, cx: &mut Context<Self>) {
        let c = &mut self.model.customize;
        c.previewing = Some(name.clone());
        c.note = Some(Note::Working(format!(
            "Starting “{name}” to read its tools. Nothing is enabled yet."
        )));
        self.bridge.mcp_preview(name);
        cx.notify();
    }

    fn check_server(&mut self, name: String, restart: bool, cx: &mut Context<Self>) {
        self.model.customize.checking.insert(name.clone());
        self.bridge.mcp_health(name, restart);
        cx.notify();
    }

    pub(super) fn customize_screen(
        &self,
        p: Palette,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &self.model.customize;
        let header = div()
            .flex()
            .items_start()
            .justify_between()
            .gap_4()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_xl()
                            .font_weight(FontWeight::MEDIUM)
                            .child("MCP & Skills"),
                    )
                    .child(div().text_sm().text_color(p.muted).child(
                        "Skills Tern loads when a task needs them, MCP servers it may call, and exactly what the model is told. Providers have their own page.",
                    )),
            );
        let tabs = div().flex().items_center().gap_1().children(
            CustomizeTab::ALL
                .into_iter()
                .filter(|t| *t != CustomizeTab::Providers)
                .map(|t| {
                    Button::new(SharedString::from(format!("customize-tab-{}", t.label())))
                        .capsule()
                        .label(t.label())
                        .ghost()
                        .small()
                        .selected(c.tab == t)
                        .on_click(cx.listener(move |this, _, _, cx| this.set_customize_tab(t, cx)))
                }),
        );
        let note = c.note.as_ref().map(|n| {
            let (text, working, failed) = match n {
                Note::Working(t) => (t.clone(), true, false),
                Note::Ok(t) => (t.clone(), false, false),
                Note::Failed(t) => (t.clone(), false, true),
            };
            div()
                .flex()
                .items_center()
                .gap_2()
                .text_xs()
                .text_color(if failed { p.text } else { p.muted })
                .when(working, |d| d.child(Spinner::new().small().color(p.muted)))
                .when(failed, |d| d.child(icon(AssetIcon::TriangleAlert, p.text)))
                .child(SharedString::from(text))
        });
        let body: AnyElement = match c.tab {
            CustomizeTab::Skills | CustomizeTab::Providers => self.skills_tab(p, cx),
            CustomizeTab::Mcp => self.mcp_tab(p, cx),
            CustomizeTab::Instructions => self.instructions_tab(p, cx),
        };
        div()
            .relative()
            .size_full()
            .child(
                div()
                    .size_full()
                    .child(
                        div()
                            .w_full()
                            .max_w(px(PAGE_MAX_W))
                            .mx_auto()
                            .px_6()
                            .py_6()
                            .flex()
                            .flex_col()
                            .gap_4()
                            .child(header)
                            .child(tabs)
                            .children(note)
                            .child(body),
                    )
                    .overflow_y_scrollbar()
                    .id("customize-scroll"),
            )
            .children(self.trust_sheet(p, cx))
            .into_any_element()
    }

    fn section_title(text: &'static str, p: Palette) -> AnyElement {
        div()
            .text_xs()
            .text_color(p.faint)
            .child(text)
            .into_any_element()
    }

    fn skills_tab(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let c = &self.model.customize;
        let mut col = div().flex().flex_col().gap_2().child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(Self::section_title(
                    "Only each skill's name and description are in the prompt; the model loads the rest with load_skill when a task matches.",
                    p,
                ))
                .child(
                    Button::new("skill-install").capsule()
                        .icon(AssetIcon::FolderInput)
                        .label("Install from folder…")
                        .small()
                        .on_click(cx.listener(|this, _, _, cx| this.pick_skill_folder(cx))),
                ),
        );
        col = col.child(self.skill_extras(p, cx));
        if let Some((from, previews)) = &c.skill_preview {
            let from = from.clone();
            col = col.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_3()
                    .rounded_lg()
                    .border_1()
                    .border_color(p.border)
                    .bg(p.raised)
                    .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(
                        SharedString::from(format!("Install from {}?", from.display())),
                    ))
                    .children(previews.iter().map(|s| {
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(div().text_sm().child(SharedString::from(format!(
                                "{} — {}",
                                s.name, s.description
                            ))))
                            .when(!s.files.is_empty(), |d| {
                                d.child(div().text_xs().text_color(p.faint).child(
                                    SharedString::from(format!("Files: {}", s.files.join(", "))),
                                ))
                            })
                            .children(s.flags.iter().map(|f| {
                                flag_line(&f.reason, &f.excerpt, f.severity == Severity::Danger, p)
                            }))
                    }))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                Button::new("skill-install-go")
                                    .capsule()
                                    .label("Install")
                                    .small()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.bridge
                                            .skills_change(SkillChange::Install(from.clone()));
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("skill-install-cancel")
                                    .capsule()
                                    .label("Cancel")
                                    .ghost()
                                    .small()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.model.customize.skill_preview = None;
                                        this.model.customize.note = None;
                                        cx.notify();
                                    })),
                            ),
                    ),
            );
        }
        if c.skills.skills.is_empty() {
            col = col.child(div().text_sm().text_color(p.muted).child(
                "No skills yet. A skill is a folder with a SKILL.md (a name and a description at the top, instructions below). Put yours in Tern's data folder under skills/, or a project's under .tern/skills/.",
            ));
        }
        for (i, s) in c.skills.skills.iter().enumerate() {
            let name = s.name.clone();
            let on = s.enabled;
            let user = s.source == tern_agent::skills::SkillSource::User;
            col = col.child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .py_2()
                    .border_b_1()
                    .border_color(p.border)
                    .child(tile(icon(AssetIcon::ScrollText, p.text), p))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div().text_sm().child(SharedString::from(s.name.clone())),
                                    )
                                    .child(
                                        div().text_xs().text_color(p.faint).child(s.source.label()),
                                    ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(p.muted)
                                    .truncate()
                                    .child(SharedString::from(s.description.clone())),
                            ),
                    )
                    .child(
                        Switch::new(("skill-switch", i))
                            .checked(on)
                            .small()
                            .on_click(cx.listener({
                                let name = name.clone();
                                move |this, checked: &bool, _, cx| {
                                    this.bridge
                                        .skills_change(SkillChange::Switch(name.clone(), *checked));
                                    cx.notify();
                                }
                            })),
                    )
                    .when(user, |d| {
                        d.child(
                            Button::new(("skill-rm", i))
                                .capsule()
                                .icon(AssetIcon::Trash)
                                .ghost()
                                .xsmall()
                                .tooltip("Remove this skill")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.bridge.skills_change(SkillChange::Remove(name.clone()));
                                    cx.notify();
                                })),
                        )
                    }),
            );
        }
        for problem in &c.skills.problems {
            col = col.child(flag_line(problem, "", false, p));
        }
        col.into_any_element()
    }

    fn mcp_tab(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let c = &self.model.customize;
        let mut col = div().flex().flex_col().gap_2().child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(Self::section_title(
                    "Servers stay off until you read their trust preview. What their tools return is treated as data, never as instructions.",
                    p,
                ))
                .child(
                    Button::new("mcp-add").capsule()
                        .icon(AssetIcon::Plus)
                        .label("Add server")
                        .small()
                        .selected(c.adding)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.model.customize.adding = !this.model.customize.adding;
                            cx.notify();
                        })),
                ),
        );
        col = col.child(self.mcp_extras(p, cx));
        if c.mcp.servers.is_empty() && !c.adding {
            col = col.child(div().text_sm().text_color(p.muted).child(
                "No MCP servers yet. Add one with its command (stdio) or its address (Streamable HTTP).",
            ));
        }
        for (i, (name, s)) in c.mcp.servers.iter().enumerate() {
            let state = c.state_of(name);
            let checking = c.checking.contains(name);
            let previewing = c.previewing.as_deref() == Some(name.as_str());
            let health = c.health.get(name);
            let n = name.clone();
            let on = s.active();
            let trusted = s.tern.trusted.is_some();
            let detail = match health {
                Some(h) if h.error.is_some() => h.error.clone().unwrap_or_default(),
                Some(h) => format!(
                    "{} · start {} · ping {} · {} tools · {} prompts · {} resources{}",
                    h.state.label(),
                    h.start_ms
                        .map(|v| format!("{v} ms"))
                        .unwrap_or_else(|| "—".into()),
                    h.ping_ms
                        .map(|v| format!("{v} ms"))
                        .unwrap_or_else(|| "—".into()),
                    h.tools,
                    h.prompts,
                    h.resources,
                    h.protocol
                        .as_deref()
                        .map(|p| format!(" · MCP {p}"))
                        .unwrap_or_default()
                ),
                None => state.label().to_owned(),
            };
            let row = div()
                .flex()
                .items_center()
                .gap_3()
                .child(server_tile(name, s, p))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(dot(state, p))
                                .child(div().text_sm().child(SharedString::from(name.clone())))
                                .when(checking, |d| {
                                    d.child(Spinner::new().xsmall().color(p.muted))
                                }),
                        )
                        .child(
                            div()
                                .text_xs()
                                .font_family(MONO_FONT)
                                .text_color(p.faint)
                                .truncate()
                                .child(SharedString::from(s.target())),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(p.muted)
                                .truncate()
                                .child(SharedString::from(detail)),
                        ),
                )
                .child(
                    Button::new(("mcp-review", i))
                        .capsule()
                        .label(if trusted { "Review" } else { "Review…" })
                        .ghost()
                        .xsmall()
                        .disabled(previewing)
                        .tooltip("Start it and read its trust preview")
                        .on_click(cx.listener({
                            let n = n.clone();
                            move |this, _, _, cx| this.preview_server(n.clone(), cx)
                        })),
                )
                .child(
                    Button::new(("mcp-check", i))
                        .capsule()
                        .icon(AssetIcon::Activity)
                        .ghost()
                        .xsmall()
                        .disabled(checking)
                        .tooltip("Health check: start, ping, list tools")
                        .on_click(cx.listener({
                            let n = n.clone();
                            move |this, _, _, cx| this.check_server(n.clone(), false, cx)
                        })),
                )
                .child(
                    Button::new(("mcp-restart", i))
                        .capsule()
                        .icon(AssetIcon::RotateCw)
                        .ghost()
                        .xsmall()
                        .disabled(checking)
                        .tooltip("Restart and check")
                        .on_click(cx.listener({
                            let n = n.clone();
                            move |this, _, _, cx| this.check_server(n.clone(), true, cx)
                        })),
                )
                .child(
                    Switch::new(("mcp-switch", i))
                        .checked(on)
                        .small()
                        .disabled(!trusted)
                        .tooltip(if trusted {
                            "On: new chats may call its tools"
                        } else {
                            "Review its tools first"
                        })
                        .on_click(cx.listener({
                            let n = n.clone();
                            move |this, checked: &bool, _, cx| {
                                this.bridge
                                    .mcp_change(McpChange::Switch(n.clone(), *checked));
                                cx.notify();
                            }
                        })),
                )
                .child(
                    Button::new(("mcp-rm", i))
                        .capsule()
                        .icon(AssetIcon::Trash)
                        .ghost()
                        .xsmall()
                        .tooltip("Remove this server")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.model.customize.previews.remove(&n);
                            this.bridge.mcp_change(McpChange::Remove(n.clone()));
                            cx.notify();
                        })),
                );
            let tools = c.previews.get(name).filter(|_| trusted).map(|pv| {
                div().flex().flex_col().gap_1().pl(px(40.)).children(
                    pv.tools.iter().enumerate().map(|(j, t)| {
                        let set = s.tool(&t.name);
                        let server = name.clone();
                        let tool = t.name.clone();
                        let mode_button = |m: Mode, cx: &mut Context<Self>| {
                            let (server, tool) = (server.clone(), tool.clone());
                            Button::new(SharedString::from(format!(
                                "mcp-tool-{i}-{j}-{}",
                                mode_label(m)
                            )))
                            .capsule()
                            .label(mode_label(m))
                            .ghost()
                            .xsmall()
                            .selected(set.enabled && set.approval == m)
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.bridge.mcp_change(McpChange::Tool(
                                        server.clone(),
                                        tool.clone(),
                                        ToolSetting {
                                            enabled: true,
                                            approval: m,
                                        },
                                    ));
                                    cx.notify();
                                },
                            ))
                        };
                        let off = {
                            let (server, tool) = (server.clone(), tool.clone());
                            Button::new(SharedString::from(format!("mcp-tool-{i}-{j}-off")))
                                .capsule()
                                .label("Off")
                                .ghost()
                                .xsmall()
                                .selected(!set.enabled)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.bridge.mcp_change(McpChange::Tool(
                                        server.clone(),
                                        tool.clone(),
                                        ToolSetting {
                                            enabled: false,
                                            approval: set.approval,
                                        },
                                    ));
                                    cx.notify();
                                }))
                        };
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_family(MONO_FONT)
                                            .child(SharedString::from(t.name.clone())),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(p.faint)
                                            .child(SharedString::from(t.effect.clone())),
                                    )
                                    .when(
                                        t.flags.iter().any(|f| f.severity == Severity::Danger),
                                        |d| d.child(icon(AssetIcon::TriangleAlert, p.text)),
                                    ),
                            )
                            .child(off)
                            .child(mode_button(Mode::Ask, cx))
                            .child(mode_button(Mode::Allow, cx))
                            .child(mode_button(Mode::Deny, cx))
                    }),
                )
            });
            col = col.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .py_2()
                    .border_b_1()
                    .border_color(p.border)
                    .child(row)
                    .children(tools)
                    .when(trusted && !c.previews.contains_key(name), |d| {
                        d.child(
                            div()
                                .pl(px(40.))
                                .text_xs()
                                .text_color(p.faint)
                                .child("Review it to set each tool to ask, allow, or refuse."),
                        )
                    }),
            );
        }
        col.into_any_element()
    }

    /// The trust preview, over the page.
    fn trust_sheet(&self, p: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let pv: &TrustPreview = self.model.customize.preview.as_ref()?;
        let name = pv.server.clone();
        let can_trust = pv.error.is_none();
        let mut card = div()
            .id("trust-sheet")
            .w(px(640.))
            .max_h(px(620.))
            .overflow_y_scrollbar()
            .flex()
            .flex_col()
            .gap_3()
            .p_5()
            .rounded_xl()
            .border_1()
            .border_color(p.border)
            .bg(p.canvas)
            .shadow_lg()
            .on_mouse_down(gpui_kit::MouseButton::Left, |_, _, cx| {
                cx.stop_propagation()
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(icon(AssetIcon::ShieldCheck, p.text))
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::MEDIUM)
                            .child(SharedString::from(format!("Review “{}”", pv.server))),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .font_family(MONO_FONT)
                    .text_color(p.muted)
                    .child(SharedString::from(format!(
                        "{} · {}",
                        pv.transport, pv.target
                    ))),
            );
        for (k, v) in pv.env.iter().chain(pv.headers.iter()) {
            card = card.child(
                div()
                    .text_xs()
                    .font_family(MONO_FONT)
                    .text_color(p.faint)
                    .child(SharedString::from(format!("{k} = {v}"))),
            );
        }
        if let Some(i) = &pv.info {
            card = card.child(
                div()
                    .text_xs()
                    .text_color(p.muted)
                    .child(SharedString::from(format!(
                        "Says it is {} {} · MCP {} (self-reported)",
                        if i.name.is_empty() {
                            "unnamed"
                        } else {
                            &i.name
                        },
                        i.version,
                        i.protocol
                    ))),
            );
            if let Some(text) = &i.instructions {
                card = card.child(
                    div()
                        .text_xs()
                        .text_color(p.faint)
                        .child(SharedString::from(format!(
                            "Its instructions (shown here, never sent to the model): {}",
                            text.trim()
                        ))),
                );
            }
        }
        for f in &pv.flags {
            card = card.child(flag_line(
                &f.reason,
                &f.excerpt,
                f.severity == Severity::Danger,
                p,
            ));
        }
        if let Some(e) = &pv.error {
            card = card.child(flag_line("Could not read its tools", e, true, p));
        }
        if pv.changed {
            card = card.child(flag_line(
                "Its tools, prompts, or resources changed since you trusted it; it is off until you trust it again",
                "",
                true,
                p,
            ));
        }
        for t in &pv.tools {
            card = card.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_2()
                    .rounded_lg()
                    .border_1()
                    .border_color(p.border)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_sm()
                                    .font_family(MONO_FONT)
                                    .child(SharedString::from(t.name.clone())),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(p.faint)
                                    .child(SharedString::from(t.effect.clone())),
                            )
                            .when(!t.params.is_empty(), |d| {
                                d.child(
                                    div()
                                        .text_xs()
                                        .text_color(p.faint)
                                        .child(SharedString::from(t.params.join(", "))),
                                )
                            }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(p.muted)
                            .child(SharedString::from(t.description.clone())),
                    )
                    .children(t.flags.iter().map(|f| {
                        flag_line(&f.reason, &f.excerpt, f.severity == Severity::Danger, p)
                    })),
            );
        }
        if !pv.prompts.is_empty() {
            card = card.child(
                div()
                    .pt_1()
                    .text_xs()
                    .text_color(p.faint)
                    .child("Prompts (offered in the composer's / menu; you read their text before anything is sent)"),
            );
        }
        for x in &pv.prompts {
            card = card.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_2()
                    .rounded_lg()
                    .border_1()
                    .border_color(p.border)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_sm()
                                    .font_family(MONO_FONT)
                                    .child(SharedString::from(format!("/{}", x.name))),
                            )
                            .when(!x.args.is_empty(), |d| {
                                d.child(
                                    div()
                                        .text_xs()
                                        .text_color(p.faint)
                                        .child(SharedString::from(x.args.join(", "))),
                                )
                            }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(p.muted)
                            .child(SharedString::from(x.description.clone())),
                    )
                    .children(x.flags.iter().map(|f| {
                        flag_line(&f.reason, &f.excerpt, f.severity == Severity::Danger, p)
                    })),
            );
        }
        if !pv.resources.is_empty() {
            card = card.child(
                div()
                    .pt_1()
                    .text_xs()
                    .text_color(p.faint)
                    .child("Resources (attached with @ as Context, wrapped as untrusted data)"),
            );
        }
        for r in &pv.resources {
            card = card.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .px_2()
                    .py_1()
                    .rounded_lg()
                    .border_1()
                    .border_color(p.border)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().text_sm().child(SharedString::from(r.name.clone())))
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_xs()
                                    .font_family(MONO_FONT)
                                    .text_color(p.faint)
                                    .child(SharedString::from(if r.template {
                                        format!("template {}", r.uri)
                                    } else {
                                        r.uri.clone()
                                    })),
                            ),
                    )
                    .when(!r.description.trim().is_empty(), |d| {
                        d.child(
                            div()
                                .text_xs()
                                .text_color(p.muted)
                                .child(SharedString::from(r.description.clone())),
                        )
                    })
                    .children(r.flags.iter().map(|f| {
                        flag_line(&f.reason, &f.excerpt, f.severity == Severity::Danger, p)
                    })),
            );
        }
        let dangers = pv.dangers();
        card = card.child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_3()
                .child(div().text_xs().text_color(p.muted).child(if dangers > 0 {
                    SharedString::from(format!(
                        "{dangers} warning{}. Tools with a warning start off; each tool asks before it runs.",
                        if dangers == 1 { "" } else { "s" }
                    ))
                } else {
                    SharedString::from("Trusting switches it on. Each tool asks before it runs until you change that.")
                }))
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            Button::new("trust-cancel").capsule()
                                .label("Not now")
                                .ghost()
                                .small()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.model.customize.preview = None;
                                    this.model.customize.note = None;
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("trust-go").capsule()
                                .label("Trust and switch on")
                                .small()
                                .disabled(!can_trust)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(pv) = this.model.customize.preview.take() {
                                        this.bridge
                                            .mcp_change(McpChange::Trust(name.clone(), Box::new(pv)));
                                    }
                                    cx.notify();
                                })),
                        ),
                ),
        );
        Some(
            div()
                .id("trust-scrim")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui_kit::black().opacity(0.45))
                .on_mouse_down(
                    gpui_kit::MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.model.customize.preview = None;
                        cx.notify();
                    }),
                )
                .child(card)
                .into_any_element(),
        )
    }

    fn instructions_tab(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let c = &self.model.customize;
        let head = div()
            .flex()
            .items_center()
            .justify_between()
            .child(Self::section_title(
                if c.instructions_for_chat {
                    "Exactly what the open chat's next turn sends, piece by piece."
                } else {
                    "What a new chat with your saved choices would be told, piece by piece."
                },
                p,
            ))
            .child(
                div()
                    .flex()
                    .gap_1()
                    .child(
                        Button::new("instr-copy")
                            .capsule()
                            .icon(AssetIcon::Copy)
                            .label("Copy prompt")
                            .ghost()
                            .small()
                            .disabled(c.instructions.is_none())
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(e) = &this.model.customize.instructions {
                                    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(
                                        e.prompt.clone(),
                                    ));
                                }
                            })),
                    )
                    .child(
                        Button::new("instr-refresh")
                            .capsule()
                            .icon(AssetIcon::RefreshCw)
                            .ghost()
                            .small()
                            .tooltip("Read again")
                            .on_click(cx.listener(|this, _, _, cx| this.refresh_instructions(cx))),
                    ),
            );
        let mut col = div().flex().flex_col().gap_2().child(head);
        if c.loading_instructions {
            col = col.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_xs()
                    .text_color(p.muted)
                    .child(Spinner::new().small().color(p.muted))
                    .child("Reading rules, skills, and tools…"),
            );
        }
        let Some(e) = &c.instructions else {
            return col.into_any_element();
        };
        let chars = e.prompt.chars().count();
        col = col.child(
            div()
                .text_xs()
                .text_color(p.faint)
                .child(SharedString::from(format!(
                    "{} pieces · {chars} characters (about {} tokens)",
                    e.pieces.len(),
                    chars.div_ceil(4)
                ))),
        );
        for (i, piece) in e.pieces.iter().enumerate() {
            let open = c.open_pieces.contains(&i);
            col = col.child(
                div()
                    .id(("piece", i))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_2()
                    .rounded_lg()
                    .border_1()
                    .border_color(p.border)
                    .cursor_pointer()
                    .hover(|d| d.bg(p.hover))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let s = &mut this.model.customize.open_pieces;
                        if !s.remove(&i) {
                            s.insert(i);
                        }
                        cx.notify();
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(icon(
                                if open {
                                    AssetIcon::ChevronDown
                                } else {
                                    AssetIcon::ChevronRight
                                },
                                p.faint,
                            ))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(p.faint)
                                    .child(piece.kind.label()),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .child(SharedString::from(piece.title.clone())),
                            )
                            .when_some(piece.source.clone(), |d, s| {
                                d.child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_xs()
                                        .font_family(MONO_FONT)
                                        .text_color(p.faint)
                                        .child(SharedString::from(s)),
                                )
                            }),
                    )
                    .when(open, |d| {
                        d.child(
                            div()
                                .text_xs()
                                .font_family(MONO_FONT)
                                .text_color(p.muted)
                                .whitespace_normal()
                                .child(SharedString::from(piece.text.clone())),
                        )
                    }),
            );
        }
        if !e.offers.is_empty() {
            col = col
                .child(
                    div()
                        .pt_2()
                        .text_sm()
                        .text_color(p.text)
                        .child(SharedString::from(format!(
                            "MCP prompts and resources ({})",
                            e.offers.len()
                        ))),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(p.faint)
                        .child("Not in the prompt. You add one to a message with / or @; the model sees it only then."),
                )
                .children(e.offers.iter().map(|o| {
                    div()
                        .text_xs()
                        .font_family(MONO_FONT)
                        .text_color(p.muted)
                        .whitespace_normal()
                        .child(SharedString::from(o.clone()))
                }));
        }
        for n in &e.notes {
            col = col.child(flag_line(n, "", false, p));
        }
        col.into_any_element()
    }

    // ----- Project folders in the sidebar -----

    pub(super) fn projects_header(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .items_center()
            .justify_between()
            .px_3()
            .pt_3()
            .pb_1()
            .child(div().text_xs().text_color(p.faint).child("Projects"))
            .child(
                div().flex().items_center().child(
                    Button::new("project-open-folder")
                        .capsule()
                        .icon(AssetIcon::FolderPlus)
                        .ghost()
                        .xsmall()
                        .tooltip("Open folder: it becomes a project, its chats grouped under it")
                        .on_click(cx.listener(|this, _, _, cx| this.open_folder_dialog(cx))),
                ),
            )
            .into_any_element()
    }

    /// The name field for a new project (`which` is `None`) or a rename.
    pub(super) fn project_name_field(
        &self,
        which: Option<ProjectId>,
        _p: Palette,
    ) -> Option<AnyElement> {
        (self.model.projects.editing == Some(which)).then(|| {
            div()
                .px_2()
                .py(px(2.))
                .child(Input::new(&self.ext_inputs.project).small())
                .into_any_element()
        })
    }

    pub(super) fn start_project_edit(
        &mut self,
        which: Option<ProjectId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = which
            .and_then(|id| self.model.projects.find(id))
            .map(|p| p.row.name.clone())
            .unwrap_or_default();
        self.model.projects.editing = Some(which);
        self.ext_inputs.project.update(cx, |i, cx| {
            i.set_value(name, window, cx);
            i.focus(window, cx);
        });
        cx.notify();
    }

    pub(super) fn finish_project_edit(&mut self, cx: &mut Context<Self>) {
        let Some(which) = self.model.projects.editing.take() else {
            return;
        };
        let name = self.ext_inputs.project.read(cx).value().trim().to_owned();
        if !name.is_empty() {
            self.bridge.projects(match which {
                None => ProjectChange::New(name),
                Some(id) => ProjectChange::Rename(id, name),
            });
        }
        cx.notify();
    }

    pub(super) fn toggle_move_menu(&mut self, id: SessionId, cx: &mut Context<Self>) {
        let t = &mut self.model.projects;
        t.move_menu = if t.move_menu == Some(id) {
            None
        } else {
            Some(id)
        };
        if !t.loaded {
            self.bridge.projects(ProjectChange::Reload);
        }
        cx.notify();
    }

    fn pick_project_root(&mut self, id: ProjectId, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Link folder".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(mut paths))) = picked.await
                && let Some(dir) = paths.pop()
            {
                let _ = this.update(cx, |this, _| {
                    this.bridge.projects(ProjectChange::Root(id, Some(dir)));
                });
            }
        })
        .detach();
    }

    pub(super) fn new_chat_in(
        &mut self,
        name: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.model.is_busy() {
            return;
        }
        self.model.projects.new_in = name.clone();
        self.model.new_chat();
        self.open_sources.clear();
        self.tabs.reset_active();
        self.session = None;
        self.model.phase = crate::model::Phase::Connecting;
        self.bridge.connect_in(name);
        self.composer.update(cx, |c, cx| {
            c.set_value("", window, cx);
            c.focus(window, cx);
        });
        cx.notify();
    }

    /// One row of the tree: a project header (a drop target) or a chat
    /// with its Move to… menu.
    pub(super) fn tree_row(
        &self,
        row: TreeRow,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        match row {
            TreeRow::Session(ix) => {
                let mut out = vec![self.chat_row(ix, p, cx)];
                let id = self.model.sessions[ix].id;
                if self.model.projects.move_menu == Some(id) {
                    out.push(self.move_menu(id, p, cx));
                }
                out
            }
            TreeRow::More { id, hidden } => vec![self.show_more_row(id, hidden, p, cx)],
            TreeRow::Project {
                id,
                name,
                inbox,
                collapsed,
                shown,
                has_rules,
                root,
            } => {
                if self.model.projects.editing == Some(Some(id)) {
                    return self.project_name_field(Some(id), p).into_iter().collect();
                }
                if let Some(root) = root {
                    return self.folder_project_row(id, name, root, collapsed, has_rules, p, cx);
                }
                let confirming = self.model.projects.confirm_delete == Some(id);
                let chat_name = (!inbox).then(|| name.clone());
                let header = div()
                    .id(SharedString::from(format!("project-{id}")))
                    .group("project-row")
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_1()
                    .py(px(3.))
                    .rounded_md()
                    .text_sm()
                    .text_color(p.muted)
                    .cursor_pointer()
                    .hover(|d| d.bg(p.hover).text_color(p.text))
                    .drag_over::<DraggedChat>(move |s, _, _, _| {
                        s.bg(p.hover).border_1().border_color(p.accent)
                    })
                    .on_drop(cx.listener(move |this, d: &DraggedChat, _, cx| {
                        this.bridge.projects(ProjectChange::Move(d.id, id));
                        cx.notify();
                    }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let saved = this.model.projects.toggle(id);
                        let mut prefs = tern_agent::Preferences::load();
                        prefs.collapsed_projects = saved;
                        this.bridge.save_prefs(prefs);
                        cx.notify();
                    }))
                    .child(icon(
                        if collapsed {
                            AssetIcon::ChevronRight
                        } else {
                            AssetIcon::ChevronDown
                        },
                        p.faint,
                    ))
                    .child(icon(
                        if inbox {
                            AssetIcon::Inbox
                        } else if collapsed {
                            AssetIcon::FolderClosed
                        } else {
                            AssetIcon::FolderOpen
                        },
                        p.muted,
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(SharedString::from(name.clone())),
                    )
                    .when(has_rules, |d| {
                        d.child(div().text_xs().text_color(p.faint).child("TERN.md"))
                    })
                    .child(
                        div()
                            .text_xs()
                            .text_color(p.faint)
                            .child(SharedString::from(shown.to_string())),
                    )
                    .child(
                        div()
                            .flex()
                            .opacity(if confirming { 1. } else { 0. })
                            .group_hover("project-row", |d| d.opacity(1.))
                            .child(
                                Button::new(SharedString::from(format!("project-chat-{id}")))
                                    .capsule()
                                    .icon(AssetIcon::MessageSquarePlus)
                                    .ghost()
                                    .xsmall()
                                    .tooltip("New chat here")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        cx.stop_propagation();
                                        this.new_chat_in(chat_name.clone(), window, cx);
                                    })),
                            )
                            .when(!inbox, |d| {
                                d.child(
                                    Button::new(SharedString::from(format!("project-root-{id}")))
                                        .capsule()
                                        .icon(AssetIcon::FolderInput)
                                        .ghost()
                                        .xsmall()
                                        .tooltip(
                                            "Link a folder (its TERN.md and .tern/skills apply)",
                                        )
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.pick_project_root(id, cx);
                                        })),
                                )
                                .child(
                                    Button::new(SharedString::from(format!("project-rename-{id}")))
                                        .capsule()
                                        .icon(AssetIcon::Pencil)
                                        .ghost()
                                        .xsmall()
                                        .tooltip("Rename project")
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            cx.stop_propagation();
                                            this.start_project_edit(Some(id), window, cx);
                                        })),
                                )
                                .child(
                                    Button::new(SharedString::from(format!("project-rm-{id}")))
                                        .capsule()
                                        .icon(AssetIcon::Trash)
                                        .ghost()
                                        .xsmall()
                                        .selected(confirming)
                                        .tooltip(if confirming {
                                            "Click again to delete; its chats move to the Inbox"
                                        } else {
                                            "Delete project (its chats move to the Inbox)"
                                        })
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            let t = &mut this.model.projects;
                                            if t.confirm_delete == Some(id) {
                                                t.confirm_delete = None;
                                                this.bridge.projects(ProjectChange::Delete(id));
                                            } else {
                                                t.confirm_delete = Some(id);
                                            }
                                            cx.notify();
                                        })),
                                )
                            }),
                    );
                vec![header.into_any_element()]
            }
        }
    }

    fn move_menu(&self, session: SessionId, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let here = self
            .model
            .sessions
            .iter()
            .find(|s| s.id == session)
            .and_then(|s| s.project);
        div()
            .ml(px(22.))
            .mr_1()
            .flex()
            .flex_col()
            .gap(px(2.))
            .p_1()
            .rounded_lg()
            .border_1()
            .border_color(p.border)
            .bg(p.raised)
            .shadow_md()
            .child(div().px_2().text_xs().text_color(p.faint).child("Move to"))
            .children(self.model.projects.projects.iter().map(|pr| {
                let id = pr.row.id;
                let current = here == Some(id);
                div()
                    .id(SharedString::from(format!("move-{session}-{id}")))
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .text_sm()
                    .text_color(if current { p.faint } else { p.text })
                    .when(!current, |d| {
                        d.cursor_pointer()
                            .hover(move |d| d.bg(p.hover))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.model.projects.move_menu = None;
                                this.bridge.projects(ProjectChange::Move(session, id));
                                cx.notify();
                            }))
                    })
                    .child(SharedString::from(pr.row.name.clone()))
            }))
            .into_any_element()
    }
}
