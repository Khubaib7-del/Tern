//! The agent's work surface (#75): colour-coded tool cards in the
//! transcript and the run panel's Tools view, the Terminal tab, the live
//! preview of a file being written (Files tab), the diff on a write's
//! approval card, and the poll that follows the workbench
//! (`tern_agent::live`). The GPUI-free parts are in `crate::toolcards`.

use std::collections::HashMap;

use gpui_kit::assets::IconName as AssetIcon;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{Selectable as _, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnimationExt as _, AnyElement, Context, FontWeight, Hsla, SharedString, div, hsla, px,
};
use tern_agent::Run;
use tern_agent::live::{self, RunState, Stream};

use super::Workspace;
use super::press::Capsule as _;
use crate::assets;
use crate::theme::Palette;
use crate::toolcards::{self, CardState, Family, StepGroup, ToolCard};

/// Rows a diff shows before "N more".
const DIFF_ROWS: usize = 400;
/// Characters of a file being written the preview shows (its end).
const PREVIEW_CHARS: usize = 60_000;
/// Characters of a command's output the Terminal shows per run (its end).
const TERM_CHARS: usize = 40_000;

/// How the Files tab shows a file being written.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum PreviewMode {
    /// The text with syntax colours.
    #[default]
    Source,
    /// Markdown rendered.
    Rendered,
    /// What changes on disk.
    Diff,
}

/// The work surface's own state.
#[derive(Default)]
pub(super) struct WorkUi {
    /// Groups and cards the user opened or closed, by key; the rest
    /// follow their default.
    pub open: HashMap<String, bool>,
    /// The board version last drawn.
    pub seen: u64,
    /// The last `show_in_panel` request handled.
    pub panel_seq: u64,
    pub preview: PreviewMode,
    /// The file whose write preview opened the Files tab.
    pub preview_path: Option<String>,
    /// The diff last computed, by (path, text hash).
    pub diff_cache: std::cell::RefCell<Option<(String, u64, tern_agent::diff::Diff)>>,
    /// Settings › Permissions: the new rule's kind and action (indices).
    pub rule_kind: usize,
    pub rule_action: usize,
}

impl std::fmt::Debug for WorkUi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkUi").field("seen", &self.seen).finish()
    }
}

/// A family's colour on this theme.
pub(super) fn family_ink(fam: Family, p: Palette) -> Hsla {
    let (h, s) = fam.hue();
    hsla(h, s, if p.canvas.l < 0.5 { 0.62 } else { 0.40 }, 1.)
}

/// The tool cards' done green and failed red, for the plan card (#77).
pub(super) fn done_green(p: Palette) -> Hsla {
    green(p)
}

pub(super) fn fail_red(p: Palette) -> Hsla {
    red(p)
}

fn red(p: Palette) -> Hsla {
    hsla(0.0, 0.65, if p.canvas.l < 0.5 { 0.62 } else { 0.45 }, 1.)
}

fn green(p: Palette) -> Hsla {
    hsla(
        145. / 360.,
        0.45,
        if p.canvas.l < 0.5 { 0.55 } else { 0.35 },
        1.,
    )
}

/// The approval yellow: an outline and a pulse, never a fill.
pub(super) fn approval_yellow(p: Palette) -> Hsla {
    hsla(
        46. / 360.,
        0.9,
        if p.canvas.l < 0.5 { 0.58 } else { 0.45 },
        1.,
    )
}

fn state_ink(s: CardState, fam: Family, p: Palette) -> Hsla {
    match s {
        CardState::Queued | CardState::Cancelled => p.faint,
        CardState::Running => family_ink(fam, p),
        CardState::Waiting => approval_yellow(p),
        CardState::Done => green(p),
        CardState::Failed => red(p),
    }
}

fn state_icon(s: CardState, p: Palette, fam: Family) -> AnyElement {
    let ink = state_ink(s, fam, p);
    let icon = |i: AssetIcon| {
        div()
            .flex_none()
            .text_color(ink)
            .child(gpui_kit::component::Icon::new(i).xsmall())
            .into_any_element()
    };
    match s {
        CardState::Running => div()
            .flex_none()
            .child(Spinner::new().xsmall().color(ink))
            .into_any_element(),
        CardState::Queued => icon(AssetIcon::CircleDashed),
        CardState::Waiting => icon(AssetIcon::Hourglass),
        CardState::Done => icon(AssetIcon::CircleCheck),
        CardState::Failed => icon(AssetIcon::CircleX),
        CardState::Cancelled => icon(AssetIcon::CircleStop),
    }
}

/// Lines of a tool's output a card shows before and after the fold.
const CARD_HEAD_LINES: usize = 8;
const CARD_TAIL_LINES: usize = 4;

fn mono(text: impl Into<SharedString>, p: Palette) -> gpui_kit::Div {
    div()
        .font_family(assets::MONO_FONT)
        .text_xs()
        .text_color(p.muted)
        .child(text.into())
}

fn hash_of(s: &str) -> u64 {
    use std::hash::{Hash as _, Hasher as _};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

/// The last `max` characters of `s`, on a line start when it can.
fn tail_chars(s: &str, max: usize) -> (&str, bool) {
    let n = s.chars().count();
    if n <= max {
        return (s, false);
    }
    let start = s.char_indices().nth(n - max).map(|(i, _)| i).unwrap_or(0);
    let start = s[start..]
        .find('\n')
        .map(|j| start + j + 1)
        .unwrap_or(start);
    (&s[start..], true)
}

/// The Markdown fence language for a file name.
fn fence_lang(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "rs" => "rust",
        "py" => "python",
        "js" | "mjs" | "cjs" => "javascript",
        "ts" | "tsx" => "typescript",
        "json" => "json",
        "toml" => "toml",
        "yaml" | "yml" => "yaml",
        "md" | "markdown" => "markdown",
        "html" | "htm" => "html",
        "css" => "css",
        "tex" | "bib" => "latex",
        "csv" | "tsv" | "txt" => "text",
        "sh" | "bash" => "bash",
        "ps1" => "powershell",
        "r" => "r",
        "sql" => "sql",
        "typ" => "typst",
        "xml" | "svg" => "xml",
        _ => "text",
    }
}

impl Workspace {
    // ----- Tool cards -----

    fn is_open(&self, key: &str, default: bool) -> bool {
        self.work.open.get(key).copied().unwrap_or(default)
    }

    fn toggle_open(&mut self, key: String, default: bool) {
        let now = !self.is_open(&key, default);
        self.work.open.insert(key, now);
    }

    /// A card's logo: the program's or file's logo, the page's icon, the
    /// MCP server's or search service's mark.
    fn card_logo(&self, c: &ToolCard, size: f32, p: Palette) -> AnyElement {
        use crate::fileicon::{FileIcon, logo};
        let dark = p.text.l > 0.5;
        let s = |k: &str| c.args.get(k).and_then(|v| v.as_str()).unwrap_or_default();
        match c.tool.as_str() {
            "run_command" => logo(
                FileIcon::of(toolcards::program_logo_name(s("command"))),
                size,
                dark,
            ),
            "run_python" => logo(FileIcon::of("x.py"), size, dark),
            "file_write" | "file_read" | "vault_write" | "vault_read" => {
                logo(FileIcon::of(s("path")), size, dark)
            }
            "file_rename" => logo(FileIcon::of(s("to")), size, dark),
            "files_list" => crate::fileicon::folder_logo(s("path"), size),
            "read_url" => {
                let host = tern_agent::permissions::host_of(s("url")).unwrap_or_default();
                let page = tern_agent::PageRef {
                    title: String::new(),
                    url: s("url").to_owned(),
                    site: host,
                    builtin: None,
                    favicon: None,
                };
                self.site_icon(&page, size, p)
            }
            "web_search" => super::service_logo(
                self.model
                    .providers
                    .prefs
                    .search
                    .as_deref()
                    .unwrap_or("wikipedia"),
                size,
                p,
                p.text,
            ),
            t if t.starts_with("mcp__") => {
                let server = t
                    .trim_start_matches("mcp__")
                    .split("__")
                    .next()
                    .unwrap_or(t);
                super::brand_logo(server, size, p)
            }
            "show_in_panel" => div()
                .flex_none()
                .text_color(family_ink(c.family, p))
                .child(gpui_kit::component::Icon::new(AssetIcon::PanelRightClose).size(px(size)))
                .into_any_element(),
            _ => div()
                .flex_none()
                .text_color(family_ink(c.family, p))
                .child(gpui_kit::component::Icon::new(AssetIcon::Wrench).size(px(size)))
                .into_any_element(),
        }
    }

    /// The transcript's tool cards for an answer (`entry`) or the run in
    /// progress (`None`): one foldable group per model step.
    pub(super) fn tool_cards(
        &self,
        entry: Option<usize>,
        run: &Run,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let groups = toolcards::cards_of(run);
        if groups.is_empty() {
            return None;
        }
        let base = match entry {
            Some(i) => format!("e{i}"),
            None => "live".to_owned(),
        };
        Some(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .children(
                    groups
                        .iter()
                        .map(|g| self.tool_group(format!("{base}-s{}", g.step), g, p, cx)),
                )
                .into_any_element(),
        )
    }

    /// One step's cards: a folded row that opens into the cards.
    fn tool_group(
        &self,
        key: String,
        g: &StepGroup,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let default = g.opens_by_itself();
        let open = self.is_open(&key, default);
        let state = g.state();
        let fam = g.cards.first().map(|c| c.family).unwrap_or(Family::Other);
        let k2 = key.clone();
        let head = div()
            .id(SharedString::from(format!("tg-{key}")))
            .flex()
            .items_center()
            .gap_1p5()
            .py(px(2.))
            .text_xs()
            .text_color(p.faint)
            .cursor_pointer()
            .hover(|d| d.text_color(p.text))
            .child(
                gpui_kit::component::Icon::new(if open {
                    AssetIcon::ChevronDown
                } else {
                    AssetIcon::ChevronRight
                })
                .xsmall(),
            )
            .child(state_icon(state, p, fam))
            .child(SharedString::from(format!(
                "Step {} · {} {}",
                g.step,
                g.cards.len(),
                if g.cards.len() == 1 {
                    "tool call"
                } else {
                    "tool calls"
                }
            )))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_color(p.faint)
                    .child(SharedString::from(g.tools_line())),
            )
            .when_some(g.cached_note(), |d, note| {
                d.child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap_1()
                        .child(gpui_kit::component::Icon::new(AssetIcon::Clock).xsmall())
                        .child(SharedString::from(note)),
                )
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                this.toggle_open(k2.clone(), default);
                cx.notify();
            }));
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(head)
            .when(open, |d| {
                d.children(
                    g.cards
                        .iter()
                        .map(|c| self.tool_card(format!("{key}-c{}", c.call), c, p, cx)),
                )
            })
            .into_any_element()
    }

    /// One card: a 3 px family stripe, the logo, the tool and its title,
    /// the state; opened, the arguments and the output's head and tail.
    pub(super) fn tool_card(
        &self,
        key: String,
        c: &ToolCard,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let default = matches!(
            c.state,
            CardState::Waiting | CardState::Failed | CardState::Running
        ) || matches!(c.family, Family::Write | Family::Command);
        let open = self.is_open(&key, default);
        let stripe = family_ink(c.family, p);
        let waiting = c.state == CardState::Waiting;
        let yellow = approval_yellow(p);
        let k2 = key.clone();
        let took = c
            .took()
            .map(|d| format!("{:.1} s", d.as_secs_f32()))
            .unwrap_or_default();
        let head = div()
            .id(SharedString::from(format!("tc-{key}")))
            .flex()
            .items_center()
            .gap_2()
            .min_w_0()
            .cursor_pointer()
            .child(self.card_logo(c, 14., p))
            .child(
                div()
                    .flex_none()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(p.text)
                    .child(SharedString::from(c.tool.clone())),
            )
            .children(c.task.as_ref().map(|t| {
                div()
                    .flex_none()
                    .px_1()
                    .rounded_sm()
                    .border_1()
                    .border_color(p.border)
                    .text_xs()
                    .text_color(p.faint)
                    .child(SharedString::from(format!("task {t}")))
            }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_family(assets::MONO_FONT)
                    .text_xs()
                    .text_color(p.muted)
                    .child(SharedString::from(c.title.clone())),
            )
            // #79: +N −M on a write's card, asked or not.
            .children(c.diff.map(|(a, r)| diff_counts(a, r, p).flex_none()))
            .children(c.cached.map(|age| {
                // #77: served from the tool cache, not run again.
                div()
                    .id(SharedString::from(format!("tc-cached-{key}")))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_1()
                    .rounded_sm()
                    .border_1()
                    .border_color(p.border)
                    .text_xs()
                    .text_color(p.faint)
                    .child(gpui_kit::component::Icon::new(AssetIcon::Clock).xsmall())
                    .child("cached")
                    .tooltip(move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(SharedString::from(format!(
                            "Same call as before: {} (not run again)",
                            tern_agent::toolcache::age_line(std::time::Duration::from_secs(age))
                        )))
                        .build(window, cx)
                    })
            }))
            .child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(p.faint)
                    .child(SharedString::from(took)),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_xs()
                    .text_color(state_ink(c.state, c.family, p))
                    .child(state_icon(c.state, p, c.family))
                    .child(c.state.label()),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.toggle_open(k2.clone(), default);
                cx.notify();
            }));
        let body = open.then(|| self.card_body(&key, c, p, cx));
        let inner = (!c.children.is_empty()).then(|| self.inner_cards(&key, c, p, cx));
        let card = div()
            .id(SharedString::from(format!("tcard-{key}")))
            .relative()
            .flex()
            .flex_col()
            .gap_1p5()
            .pl(px(10.))
            .pr_2()
            .py_1p5()
            .rounded_md()
            .border_1()
            .border_color(if waiting { yellow } else { p.border })
            .bg(p.raised)
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .bottom_0()
                    .w(px(3.))
                    .rounded_l_md()
                    .bg(if c.state == CardState::Failed {
                        red(p)
                    } else {
                        stripe
                    }),
            )
            .child(head)
            .children(body)
            .children(inner);
        if waiting && !self.reduced_motion {
            card.with_animation(
                SharedString::from(format!("tcard-pulse-{key}")),
                gpui_kit::Animation::new(std::time::Duration::from_millis(1400)).repeat(),
                move |d, t| {
                    let a = 0.45 + 0.55 * (0.5 + 0.5 * (t * std::f32::consts::TAU).cos());
                    d.border_color(yellow.opacity(a))
                },
            )
            .into_any_element()
        } else {
            card.into_any_element()
        }
    }

    /// The calls a code-mode script or a task graph made (#77): one row
    /// that opens into their cards, open by itself when one waits for
    /// you or failed.
    fn inner_cards(
        &self,
        key: &str,
        c: &ToolCard,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ikey = format!("{key}-in");
        let default = c.opens_inner();
        let open = self.is_open(&ikey, default);
        let state = c.inner_state().unwrap_or(CardState::Done);
        let fam = c
            .children
            .first()
            .map(|k| k.family)
            .unwrap_or(Family::Other);
        let k2 = ikey.clone();
        let row = div()
            .id(SharedString::from(format!("tin-{key}")))
            .flex()
            .items_center()
            .gap_1p5()
            .min_w_0()
            .text_xs()
            .text_color(p.faint)
            .cursor_pointer()
            .hover(|d| d.text_color(p.text))
            .child(
                gpui_kit::component::Icon::new(if open {
                    AssetIcon::ChevronDown
                } else {
                    AssetIcon::ChevronRight
                })
                .xsmall(),
            )
            .child(state_icon(state, p, fam))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .child(SharedString::from(c.inner_line())),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.toggle_open(k2.clone(), default);
                cx.notify();
            }));
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(row)
            .when(open, |d| {
                d.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .pl_3()
                        .border_l_1()
                        .border_color(p.border)
                        .children(
                            c.children
                                .iter()
                                .map(|k| self.tool_card(format!("{key}-k{}", k.call), k, p, cx)),
                        ),
                )
            })
            .into_any_element()
    }

    fn card_body(&self, key: &str, c: &ToolCard, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let mut col = div().flex().flex_col().gap_1().min_w_0();
        for (k, v) in c.arg_lines() {
            let v = if v.chars().count() > 600 {
                format!("{}…", v.chars().take(600).collect::<String>())
            } else {
                v
            };
            col = col.child(
                div()
                    .flex()
                    .gap_2()
                    .min_w_0()
                    .child(
                        div()
                            .flex_none()
                            .w(px(64.))
                            .text_xs()
                            .text_color(p.faint)
                            .child(SharedString::from(k)),
                    )
                    .child(mono(v, p).flex_1().min_w_0().line_clamp(6)),
            );
        }
        if let Some(why) = &c.asked_because
            && c.state == CardState::Waiting
        {
            col = col.child(
                div()
                    .text_xs()
                    .text_color(approval_yellow(p))
                    .child(SharedString::from(crate::toolcards::waiting_line(why))),
            );
        }
        if let Some(why) = &c.allowed_because {
            col = col.child(
                div()
                    .text_xs()
                    .text_color(approval_yellow(p))
                    .child(SharedString::from(why.clone())),
            );
        }
        if let Some(e) = &c.error {
            col = col.child(
                div()
                    .text_xs()
                    .text_color(red(p))
                    .child(SharedString::from(e.clone())),
            );
        }
        if !c.output.is_empty() {
            // The card keeps the first and last lines; the rest is in the
            // Terminal tab or Full output.
            let (shown, _) = tail_chars(&c.output, 3200);
            let shown = crate::toolcards::preview_lines(shown, CARD_HEAD_LINES, CARD_TAIL_LINES);
            col = col.child(
                div()
                    .mt_1()
                    .p_2()
                    .rounded_sm()
                    .bg(p.canvas)
                    .border_1()
                    .border_color(p.border)
                    .min_w_0()
                    .child(mono(shown, p)),
            );
        }
        let output = c.output.clone();
        let saved = c.saved_as.clone();
        let session = self.current;
        let term = c.terminal_run;
        let mut actions = div().flex().items_center().gap_1();
        if !output.is_empty() {
            actions = actions.child(
                Button::new(SharedString::from(format!("tc-copy-{key}")))
                    .capsule()
                    .icon(AssetIcon::Copy)
                    .label("Copy output")
                    .ghost()
                    .xsmall()
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(output.clone()));
                    })),
            );
        }
        if let (Some(id), Some(session)) = (saved, session) {
            actions = actions.child(
                Button::new(SharedString::from(format!("tc-full-{key}")))
                    .capsule()
                    .icon(AssetIcon::FileText)
                    .label("Full output")
                    .ghost()
                    .xsmall()
                    .tooltip("Open the whole output, saved with the chat")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let path = tern_agent::platform::AppPaths::discover().ok().map(|a| {
                            tern_agent::results::ResultStore::for_session(&a.data_dir, session)
                                .dir()
                                .join(format!("{id}.txt"))
                        });
                        match path {
                            Some(p) if p.exists() => {
                                if let Err(e) = crate::reveal::open(&p) {
                                    this.model
                                        .toast_error(format!("Couldn't open it: {e}"), None);
                                }
                            }
                            _ => this.model.toast_error(
                                "The full output is no longer saved.".to_owned(),
                                None,
                            ),
                        }
                        cx.notify();
                    })),
            );
        }
        if term.is_some() {
            actions = actions.child(
                Button::new(SharedString::from(format!("tc-term-{key}")))
                    .capsule()
                    .icon(AssetIcon::SquareTerminal)
                    .label("Terminal")
                    .ghost()
                    .xsmall()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_rail(crate::model::RunView::Terminal);
                        cx.notify();
                    })),
            );
        }
        col.child(actions).into_any_element()
    }

    /// The run panel's Tools view: every card, grouped by step.
    pub(super) fn run_tools(&self, run: &Run, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        match self.tool_cards(None, run, p, cx) {
            Some(cards) => div()
                .flex()
                .flex_col()
                .gap_2()
                .child(self.family_legend(p))
                .child(cards)
                .into_any_element(),
            None => div()
                .text_sm()
                .text_color(p.faint)
                .child("Each tool call shows here as a card: queued, running, waiting for you, done, failed or cancelled.")
                .into_any_element(),
        }
    }

    fn family_legend(&self, p: Palette) -> AnyElement {
        div()
            .flex()
            .flex_wrap()
            .gap_x_3()
            .gap_y_1()
            .text_xs()
            .text_color(p.faint)
            .children(
                [
                    Family::Search,
                    Family::Read,
                    Family::Write,
                    Family::Command,
                    Family::LocalModel,
                ]
                .into_iter()
                .map(|f| {
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(div().size(px(7.)).rounded_full().bg(family_ink(f, p)))
                        .child(f.label())
                }),
            )
            .into_any_element()
    }

    /// Opens the rail on a view.
    pub(super) fn show_rail(&mut self, view: crate::model::RunView) {
        self.model.browser.visible = false;
        self.model.run_panel_open = true;
        self.model.set_run_view(view);
    }

    // ----- Terminal -----

    /// The Terminal tab: each command the workbench ran, its output as it
    /// streams, the exit code, the time, and a kill button while it runs.
    pub(super) fn terminal_panel(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let runs = live::runs();
        let mut col = div().flex().flex_col().gap_2().child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_xs()
                        .text_color(p.faint)
                        .child("Commands the research agent ran on its workbench, sandboxed in the chat's folder."),
                )
                .when(runs.iter().any(|r| r.state != RunState::Running), |d| {
                    d.child(
                        Button::new("term-clear")
                            .capsule()
                            .label("Clear")
                            .ghost()
                            .xsmall()
                            .on_click(cx.listener(|_, _, _, cx| {
                                live::clear_finished();
                                cx.notify();
                            })),
                    )
                }),
        );
        if runs.is_empty() {
            return col
                .child(
                    div()
                        .text_sm()
                        .text_color(p.faint)
                        .child("Nothing has run yet. In Act mode Tern can convert files (pandoc, typst), run a script over your data or tidy citations here; each command shows its output live."),
                )
                .into_any_element();
        }
        for r in runs.iter().rev() {
            col = col.child(self.term_run(r, p, cx));
        }
        col.into_any_element()
    }

    fn term_run(&self, r: &live::TermRun, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        use crate::fileicon::{FileIcon, logo};
        let id = r.id;
        let running = r.state == RunState::Running;
        let state_text = match (r.state, r.exit) {
            (RunState::Exited, Some(c)) => format!("exit {c}"),
            (s, _) => s.label().to_owned(),
        };
        let ink = match (r.state, r.exit) {
            (RunState::Running, _) => family_ink(Family::Command, p),
            (RunState::Exited, Some(0)) => green(p),
            _ => red(p),
        };
        let head = div()
            .flex()
            .items_center()
            .gap_2()
            .min_w_0()
            .child(logo(
                FileIcon::of(toolcards::program_logo_name(&r.command)),
                14.,
                p.text.l > 0.5,
            ))
            .child(
                mono(format!("$ {}", r.command), p)
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(p.text),
            )
            .child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(p.faint)
                    .child(SharedString::from(format!(
                        "{:.1} s",
                        r.elapsed().as_secs_f32()
                    ))),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_xs()
                    .text_color(ink)
                    .when(running, |d| d.child(Spinner::new().xsmall().color(ink)))
                    .child(SharedString::from(state_text)),
            )
            .when(running, |d| {
                d.child(
                    Button::new(SharedString::from(format!("term-kill-{id}")))
                        .capsule()
                        .icon(AssetIcon::CircleStop)
                        .label("Kill")
                        .ghost()
                        .xsmall()
                        .tooltip("Stop this command and everything it started")
                        .on_click(cx.listener(move |_, _, _, cx| {
                            live::kill(id);
                            cx.notify();
                        })),
                )
            });
        let mut text = String::new();
        let mut out = div().flex().flex_col();
        let mut lines: Vec<(Stream, String)> = Vec::new();
        for (s, t) in &r.output {
            text.push_str(t);
            lines.push((*s, t.clone()));
        }
        // Keep the end of long output.
        let total: usize = lines.iter().map(|(_, t)| t.chars().count()).sum();
        if total > TERM_CHARS {
            let mut drop = total - TERM_CHARS;
            while drop > 0 && !lines.is_empty() {
                let n = lines[0].1.chars().count();
                if n <= drop {
                    drop -= n;
                    lines.remove(0);
                } else {
                    lines[0].1 = lines[0].1.chars().skip(drop).collect();
                    drop = 0;
                }
            }
            out = out.child(mono("… earlier output cut", p).text_color(p.faint));
        }
        for (s, t) in lines {
            let color = match s {
                Stream::Stdout => p.text,
                Stream::Stderr => hsla(
                    25. / 360.,
                    0.6,
                    if p.canvas.l < 0.5 { 0.65 } else { 0.40 },
                    1.,
                ),
                Stream::Tern => p.faint,
            };
            out = out.child(mono(t.trim_end_matches('\n').to_owned(), p).text_color(color));
        }
        if r.dropped > 0 {
            out = out.child(mono(format!("[{} bytes not kept]", r.dropped), p).text_color(p.faint));
        }
        let copy = format!("$ {}\n{text}", r.command);
        div()
            .id(SharedString::from(format!("term-run-{id}")))
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .rounded_md()
            .border_1()
            .border_color(p.border)
            .bg(p.canvas)
            .child(head)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_xs()
                    .text_color(p.faint)
                    .gap_1()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(SharedString::from(format!("{} · {}", r.cwd, r.sandbox))),
                    )
                    .child(
                        Button::new(SharedString::from(format!("term-copy-{id}")))
                            .capsule()
                            .icon(AssetIcon::Copy)
                            .ghost()
                            .xsmall()
                            .tooltip("Copy the command and its output")
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(
                                    copy.clone(),
                                ));
                            })),
                    ),
            )
            .when(!r.output.is_empty(), |d| d.child(out))
            .into_any_element()
    }

    // ----- Live write preview and diff -----

    /// The diff of a proposed write against the file on disk, cached.
    fn diff_for(&self, path: &str, text: &str) -> tern_agent::diff::Diff {
        let h = hash_of(text);
        if let Some((p, k, d)) = self.work.diff_cache.borrow().as_ref()
            && p == path
            && *k == h
        {
            return d.clone();
        }
        let old = self
            .model
            .files
            .root
            .as_ref()
            .map(|r| r.join(tern_agent::workspace::unroot_path(r, path)))
            .and_then(|f| std::fs::read_to_string(f).ok());
        let d = tern_agent::diff::line_diff(old.as_deref(), text, 3);
        *self.work.diff_cache.borrow_mut() = Some((path.to_owned(), h, d.clone()));
        d
    }

    /// Diff rows in monospace: removed lines red, added green, gaps.
    pub(super) fn diff_view(&self, path: &str, text: &str, p: Palette) -> AnyElement {
        use tern_agent::diff::RowKind;
        let d = self.diff_for(path, text);
        // GitHub-style counts (#76): the name, +N in green and −M in red,
        // then what it is compared with.
        let name = path.rsplit(['/', '\\']).next().unwrap_or(path).to_owned();
        let note = if d.new_file {
            "new file"
        } else if d.rows.is_empty() {
            "no change: the text matches the file on disk"
        } else {
            "against the file on disk"
        };
        let mut col = div().flex().flex_col().min_w_0().child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .pb_1()
                .text_xs()
                .child(mono(name, p).min_w_0().truncate().text_color(p.text))
                .child(diff_counts(d.added, d.removed, p))
                .child(div().text_color(p.faint).child(note)),
        );
        let (add_bg, del_bg) = (green(p).opacity(0.14), red(p).opacity(0.14));
        for r in d.rows.iter().take(DIFF_ROWS) {
            let (sign, bg, ink) = match r.kind {
                RowKind::Added => ("+", Some(add_bg), p.text),
                RowKind::Removed => ("−", Some(del_bg), p.text),
                RowKind::Same => (" ", None, p.muted),
                RowKind::Gap => ("⋯", None, p.faint),
            };
            let num = r.new.or(r.old).map(|n| n.to_string()).unwrap_or_default();
            col = col.child(
                div()
                    .flex()
                    .gap_2()
                    .min_w_0()
                    .when_some(bg, |d, bg| d.bg(bg))
                    .child(
                        mono(num, p)
                            .flex_none()
                            .w(px(34.))
                            .text_color(p.faint)
                            .text_right(),
                    )
                    .child(mono(sign, p).flex_none().w(px(10.)).text_color(ink))
                    .child(mono(r.text.clone(), p).flex_1().min_w_0().text_color(ink)),
            );
        }
        if d.rows.len() > DIFF_ROWS {
            col = col.child(mono(format!("… {} more rows", d.rows.len() - DIFF_ROWS), p));
        }
        col.into_any_element()
    }

    /// The top of the Files tab while the model writes a file: the text as
    /// it arrives (syntax colours, or Markdown rendered), or the diff.
    pub(super) fn write_preview(&self, p: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let w = live::preview()?;
        let is_md = matches!(fence_lang(&w.path), "markdown");
        let mode = self.work.preview;
        let waiting = matches!(&self.model.phase, crate::model::Phase::Approval { request, .. } if request.tool == "file_write");
        let modes: Vec<(PreviewMode, &str)> = [
            Some((PreviewMode::Source, "Source")),
            is_md.then_some((PreviewMode::Rendered, "Preview")),
            Some((PreviewMode::Diff, "Diff")),
        ]
        .into_iter()
        .flatten()
        .collect();
        let switch = div()
            .flex()
            .items_center()
            .gap_1()
            .children(modes.into_iter().map(|(m, label)| {
                Button::new(SharedString::from(format!("wp-mode-{label}")))
                    .capsule()
                    .label(label)
                    .ghost()
                    .xsmall()
                    .selected(mode == m)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.work.preview = m;
                        cx.notify();
                    }))
            }));
        // #77: never read as saved before it is. The file on disk is the
        // proof: "Saved" only once it holds exactly this text.
        let saved = !waiting && w.done && self.preview_saved(&w.path, &w.text);
        // Short, so the file's name keeps its room in the header.
        let status = if waiting {
            "Pending approval"
        } else if saved {
            "Saved"
        } else if w.done {
            "Not written"
        } else {
            "writing…"
        };
        let (shown, cut) = tail_chars(&w.text, PREVIEW_CHARS);
        let body: AnyElement = match mode {
            PreviewMode::Diff => self.diff_view(&w.path, &w.text, p),
            PreviewMode::Rendered if is_md => {
                super::bidi_markdown("write-preview", 0, shown.to_owned(), |t| {
                    crate::richmd::rich(t.selectable(true))
                })
            }
            _ => {
                let fence = if shown.contains("```") { "````" } else { "```" };
                super::bidi_markdown(
                    "write-preview",
                    1,
                    format!("{fence}{}\n{shown}\n{fence}", fence_lang(&w.path)),
                    |t| crate::richmd::rich(t.selectable(true)),
                )
            }
        };
        Some(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .mb_2()
                .p_2()
                .rounded_md()
                .border_1()
                .border_color(if waiting {
                    approval_yellow(p)
                } else {
                    p.border
                })
                .bg(p.canvas)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .min_w_0()
                        .child(crate::fileicon::logo(
                            crate::fileicon::FileIcon::of(&w.path),
                            14.,
                            p.text.l > 0.5,
                        ))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(64.))
                                .truncate()
                                .text_xs()
                                .font_weight(FontWeight::MEDIUM)
                                .child(SharedString::from(w.path.clone())),
                        )
                        // Once saved the file on disk is the text, so a
                        // diff against it would read +0 −0.
                        .when(!saved, |row| {
                            let d = self.diff_for(&w.path, &w.text);
                            row.child(diff_counts(d.added, d.removed, p).flex_none())
                        })
                        .child(
                            div()
                                .flex_none()
                                .text_xs()
                                .text_color(if waiting { approval_yellow(p) } else { p.faint })
                                .child(status),
                        ),
                )
                .child(switch)
                .when(cut, |d| {
                    d.child(mono("… the start is above; showing the end", p))
                })
                .child(div().min_w_0().child(body))
                .into_any_element(),
        )
    }

    /// Whether the previewed file is on disk with exactly `text` (#77).
    fn preview_saved(&self, rel: &str, text: &str) -> bool {
        let Some((_, root)) = self.model.workspace.as_ref() else {
            return false;
        };
        // #79: `New/review.md` in a folder named New is `review.md`.
        let path = root.join(tern_agent::workspace::unroot_path(root, rel));
        match std::fs::metadata(&path) {
            Ok(m) if m.len() == text.len() as u64 => {
                std::fs::read(&path).is_ok_and(|b| b == text.as_bytes())
            }
            _ => false,
        }
    }

    // ----- The live poll -----

    /// Called every [`LIVE_POLL`]: redraws when the workbench changed, and
    /// follows `show_in_panel` requests.
    pub(super) fn poll_live(&mut self, cx: &mut gpui_kit::Context<Self>) {
        if let Some(req) = live::panel_request(self.work.panel_seq) {
            self.work.panel_seq = req.seq;
            let view = match req.tab.as_str() {
                "terminal" => Some(crate::model::RunView::Terminal),
                "files" | "outputs" => Some(crate::model::RunView::Files),
                "sources" => Some(crate::model::RunView::Ledger),
                "report" => Some(crate::model::RunView::Report),
                _ => None,
            };
            if let Some(v) = view {
                self.show_rail(v);
                if v == crate::model::RunView::Files {
                    self.model.files.reload();
                }
                tern_agent::diag::note(&format!("panel: show_in_panel opened {}", req.tab));
            }
            cx.notify();
        }
        let v = live::version();
        if v != self.work.seen {
            self.work.seen = v;
            // A write being previewed opens the Files tab once per file,
            // when the rail is open or opens by itself.
            let path = live::preview().map(|w| w.path).filter(|p| !p.is_empty());
            if path.is_some() && path != self.work.preview_path {
                if self.model.run_panel_open || self.model.run_panel_auto {
                    self.show_rail(crate::model::RunView::Files);
                }
                self.work.preview_path = path;
            }
            cx.notify();
        }
    }
}

/// "+41 −0": lines added in green, removed in red, as GitHub shows them.
pub(super) fn diff_counts(added: usize, removed: usize, p: Palette) -> gpui_kit::Div {
    div()
        .flex()
        .items_center()
        .gap_1()
        .font_family(crate::assets::MONO_FONT)
        .text_xs()
        .child(
            div()
                .text_color(green(p))
                .child(SharedString::from(format!("+{added}"))),
        )
        .child(
            div()
                .text_color(red(p))
                .child(SharedString::from(format!("−{removed}"))),
        )
}

/// How often the window checks the workbench board.
pub(super) const LIVE_POLL: std::time::Duration = std::time::Duration::from_millis(120);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tails_and_languages() {
        let s = "a\nb\nc\nd\n";
        assert_eq!(tail_chars(s, 100), (s, false));
        let (t, cut) = tail_chars(s, 5);
        assert!(cut);
        assert_eq!(t, "c\nd\n");
        assert_eq!(fence_lang("notes/a.md"), "markdown");
        assert_eq!(fence_lang("x.PY"), "python");
        assert_eq!(fence_lang("noext"), "text");
    }
}
