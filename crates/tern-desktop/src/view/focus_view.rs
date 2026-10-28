//! The block focus view (#76): a rich block opened large over the chat,
//! the chat dimmed and frosted behind it. One overlay for every kind
//! (`crate::focus` holds the state and says what each kind offers):
//!
//! - code: an editor with line numbers, wrap, Find, Copy · Export ▾ and
//!   "Ask about this";
//! - table: sort by a column, filter, edit cells and headers, add and
//!   remove rows and columns, CSV · Markdown · Excel;
//! - math: the LaTeX beside its live drawing, zoom;
//! - Mermaid: the source beside its live drawing, zoom, pan, fit, and a
//!   flowchart turned left-to-right or top-to-bottom;
//! - a picture: zoom, pan, fit, save.
//!
//! "Save to answer" (Ctrl+Enter) puts the edited block back into its
//! answer as a new version (the original is always kept, and any version
//! can be loaded again); the edits are kept beside the chat
//! (`focus::Edits`) so a reopened chat shows them. Esc or a click outside
//! closes it.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::assets::IconName as AssetIcon;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::input::{
    Editor as CodeEditor, EditorState as CodeState, Input, InputEvent, InputState, Search,
};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, Context, Entity, Focusable as _, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, ScrollWheelEvent, SharedString, Subscription, Window, canvas, div, img, px,
};

use super::Workspace;
use crate::focus::{self, Block, Dir, Focus, Kind};
use crate::model::Entry;
use crate::theme::Palette;

/// How long the drawing waits after a keystroke before it follows.
const PREVIEW_WAIT: Duration = Duration::from_millis(220);
/// A formula's drawing, in pixels per SVG unit (formulas are typeset at
/// 16 units per em, so about 40 px per em at zoom 1).
const MATH_SCALE: f32 = 2.5;

/// The focus view while it is open.
pub(super) struct FocusUi {
    pub(super) model: Focus,
    /// The source editor (code, LaTeX, Mermaid).
    editor: Option<Entity<CodeState>>,
    /// The table cell being edited, and its field.
    cell: Entity<InputState>,
    editing: Option<(Option<usize>, usize)>,
    filter: Entity<InputState>,
    /// A raster picture's image.
    picture: Option<Arc<gpui_kit::Image>>,
    /// The source the drawing shows (follows the draft after a pause).
    preview: String,
    generation: u64,
    /// The drawing's area on screen: x, y, width, height.
    stage: Rc<Cell<[f32; 4]>>,
    /// Where a pan drag last was.
    drag: Option<(f32, f32)>,
    /// The answer the block is in: its entry, and its place among answers.
    answer: Option<(usize, usize)>,
    /// A line under the header: "Saved to the answer as version 2".
    note: Option<SharedString>,
    _subs: Vec<Subscription>,
}

impl std::fmt::Debug for FocusUi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FocusUi")
            .field("kind", &self.model.block.kind)
            .field("versions", &self.model.versions.count())
            .finish()
    }
}

/// The drawing of a source, and its size at zoom 1.
struct Drawing {
    image: Arc<gpui_kit::Image>,
    size: (f32, f32),
    svg: Option<String>,
}

impl Workspace {
    /// Where a chat's block edits are kept.
    fn block_edits_path(&self) -> Option<std::path::PathBuf> {
        let id = self.current?;
        let dir = tern_agent::trash::data_dir().ok()?.join("block-edits");
        Some(focus::Edits::path(&dir, &id.to_string()))
    }

    /// The chat's answers by entry index, in order.
    fn answer_entries(&self) -> Vec<usize> {
        self.model
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| matches!(e, Entry::Assistant { .. }))
            .map(|(i, _)| i)
            .collect()
    }

    /// After a chat's turns come back: its saved block edits go back into
    /// its answers.
    pub(super) fn apply_block_edits(&mut self) {
        let Some(path) = self.block_edits_path() else {
            return;
        };
        let edits = focus::Edits::load(&path);
        if edits.list.is_empty() {
            return;
        }
        let at = self.answer_entries();
        let mut answers: Vec<String> = at
            .iter()
            .map(|&i| match &self.model.entries[i] {
                Entry::Assistant { text, .. } => text.clone(),
                _ => String::new(),
            })
            .collect();
        edits.apply(&mut answers);
        for (&i, new) in at.iter().zip(answers) {
            if let Some(Entry::Assistant { text, .. }) = self.model.entries.get_mut(i) {
                *text = new;
            }
        }
    }

    /// Opens a block from an answer in the focus view.
    pub(super) fn open_focus(&mut self, block: Block, window: &mut Window, cx: &mut Context<Self>) {
        // Which answer it is in: the newest that has it.
        let at = self.answer_entries();
        let answer = at.iter().enumerate().rev().find_map(|(n, &i)| {
            let Entry::Assistant { text, .. } = &self.model.entries[i] else {
                return None;
            };
            focus::replace_block(text, &block.kind, &block.source, &block.source).map(|_| (i, n))
        });
        let history = match (answer, self.block_edits_path()) {
            (Some((_, n)), Some(path)) => focus::Edits::load(&path).history(n, &block.source),
            _ => Vec::new(),
        };
        self.focus_with(
            Focus::with_history(block, history),
            answer,
            None,
            window,
            cx,
        );
    }

    /// Opens a sent picture in the focus view.
    pub(super) fn open_picture(
        &mut self,
        name: &str,
        image: Arc<gpui_kit::Image>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let block = Block {
            kind: Kind::Image { svg: false },
            source: name.to_owned(),
            table: None,
        };
        self.focus_with(Focus::new(block), None, Some(image), window, cx);
    }

    fn focus_with(
        &mut self,
        model: Focus,
        answer: Option<(usize, usize)>,
        picture: Option<Arc<gpui_kit::Image>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let caps = model.caps();
        let mut subs = Vec::new();
        let editor = caps.edit.then(|| {
            let lang = model.block.kind.editor_language().to_owned();
            let text = model.draft.clone();
            let e = cx.new(|cx| {
                let mut s = CodeState::new(window, cx)
                    .language(lang)
                    .line_number(true)
                    .searchable(true)
                    .soft_wrap(!matches!(model.block.kind, Kind::Code { .. }));
                s.set_value(text, window, cx);
                s
            });
            subs.push(cx.subscribe_in(
                &e,
                window,
                |this: &mut Self, state, event: &InputEvent, _, cx| {
                    if let InputEvent::Change = event {
                        let text = state.read(cx).value().to_string();
                        this.focus_draft(text, cx);
                    }
                },
            ));
            e
        });
        let cell = cx.new(|cx| InputState::new(window, cx));
        subs.push(cx.subscribe_in(
            &cell,
            window,
            |this: &mut Self, _, event: &InputEvent, _, cx| match event {
                InputEvent::PressEnter { .. } | InputEvent::Blur => this.focus_cell_done(cx),
                _ => {}
            },
        ));
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter rows"));
        subs.push(cx.subscribe_in(
            &filter,
            window,
            |this: &mut Self, state, event: &InputEvent, _, cx| {
                if let InputEvent::Change = event
                    && let Some(f) = this.focus.as_mut()
                {
                    f.model.filter = state.read(cx).value().to_string();
                    cx.notify();
                }
            },
        ));
        if let Some(e) = &editor {
            e.update(cx, |e, cx| e.focus(window, cx));
        }
        tern_agent::diag::note(&format!(
            "focus: opened a {} block",
            model.block.kind.label().to_lowercase()
        ));
        self.focus = Some(Box::new(FocusUi {
            preview: model.draft.clone(),
            model,
            editor,
            cell,
            editing: None,
            filter,
            picture,
            generation: 0,
            stage: Rc::new(Cell::new([0.; 4])),
            drag: None,
            answer,
            note: None,
            _subs: subs,
        }));
        cx.notify();
    }

    /// The editor changed: keep the draft, and let the drawing follow it
    /// after a pause.
    fn focus_draft(&mut self, text: String, cx: &mut Context<Self>) {
        let Some(f) = self.focus.as_mut() else {
            return;
        };
        if f.model.draft == text {
            return;
        }
        f.model.draft = text;
        f.note = None;
        if !f.model.caps().preview {
            cx.notify();
            return;
        }
        f.generation += 1;
        let generation = f.generation;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(PREVIEW_WAIT).await;
            let _ = this.update(cx, |this, cx| {
                if let Some(f) = this.focus.as_mut()
                    && f.generation == generation
                {
                    f.preview = f.model.draft.clone();
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn close_focus(&mut self, cx: &mut Context<Self>) {
        if self.focus.take().is_some() {
            cx.notify();
        }
    }

    /// Esc closes (or ends a cell edit first); Ctrl+Enter saves to the
    /// answer. Returns whether the key was taken.
    pub(super) fn focus_keys(
        &mut self,
        e: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(f) = self.focus.as_mut() else {
            return false;
        };
        let k = &e.keystroke;
        match k.key.as_str() {
            "escape" => {
                if f.editing.take().is_some() {
                    cx.notify();
                } else {
                    self.close_focus(cx);
                }
                cx.stop_propagation();
                true
            }
            "enter" if k.modifiers.control || k.modifiers.platform => {
                self.focus_cell_done(cx);
                self.focus_save(window, cx);
                cx.stop_propagation();
                true
            }
            _ => false,
        }
    }

    /// "Save to answer": the edits become the block's newest version and
    /// the answer shows it.
    fn focus_save(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let path = self.block_edits_path();
        let Some(f) = self.focus.as_mut() else {
            return;
        };
        let Some((entry, ordinal)) = f.answer else {
            f.note = Some(
                "This block isn't in an answer of this chat, so it can only be exported.".into(),
            );
            cx.notify();
            return;
        };
        let kind = f.model.block.kind.clone();
        let Some((old, new)) = f.model.commit() else {
            f.note = Some("Nothing changed yet.".into());
            cx.notify();
            return;
        };
        let replaced = match self.model.entries.get_mut(entry) {
            Some(Entry::Assistant { text, .. }) => {
                match focus::replace_block(text, &kind, &old, &new) {
                    Some(t) => {
                        *text = t;
                        true
                    }
                    None => false,
                }
            }
            _ => false,
        };
        let Some(f) = self.focus.as_mut() else {
            return;
        };
        if !replaced {
            f.note =
                Some("Couldn't find this block in its answer any more; export it instead.".into());
            cx.notify();
            return;
        }
        let versions = f.model.versions.all().to_vec();
        let n = versions.len();
        if let Some(path) = path {
            let mut edits = focus::Edits::load(&path);
            edits.record(ordinal, &kind, &versions);
            if let Err(e) = edits.save(&path) {
                crate::crashlog::log_line(&format!("focus: couldn't keep the edit: {e}"));
            }
        }
        f.note = Some(SharedString::from(format!(
            "Saved to the answer as version {n}. The original is kept."
        )));
        self.model.toasts.info(
            format!("{} saved to the answer", kind.label()),
            std::time::Instant::now(),
        );
        cx.notify();
    }

    /// Loads version `i` into the editor (Save to answer applies it).
    fn focus_pick(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = self.focus.as_mut() else {
            return;
        };
        f.model.pick_version(i);
        f.preview = f.model.draft.clone();
        f.model.needs_fit = true;
        f.note = Some(if i == 0 {
            "The original is loaded. Save to answer to put it back.".into()
        } else {
            SharedString::from(format!("Version {} is loaded.", i + 1))
        });
        let text = f.model.draft.clone();
        if let Some(e) = f.editor.clone() {
            e.update(cx, |e, cx| e.set_value(text, window, cx));
        }
        cx.notify();
    }

    /// "Ask about this": the block rides along with the next message, as
    /// a reply quote; the composer takes the focus.
    fn focus_ask(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = self.focus.take() else {
            return;
        };
        self.model.reply_to = Some(tern_agent::reply::excerpt(&f.model.markdown()));
        self.composer.update(cx, |c, cx| c.focus(window, cx));
        cx.notify();
    }

    fn focus_cell_edit(
        &mut self,
        row: Option<usize>,
        col: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_cell_done(cx);
        let Some(f) = self.focus.as_mut() else {
            return;
        };
        let Some(t) = &f.model.table else {
            return;
        };
        let value = match row {
            None => t.headers.get(col).cloned().unwrap_or_default(),
            Some(r) => t.cell(r, col).to_owned(),
        };
        f.editing = Some((row, col));
        f.cell.update(cx, |c, cx| {
            c.set_value(value, window, cx);
            c.focus(window, cx);
        });
        cx.notify();
    }

    fn focus_cell_done(&mut self, cx: &mut Context<Self>) {
        let Some(f) = self.focus.as_mut() else {
            return;
        };
        let Some((row, col)) = f.editing.take() else {
            return;
        };
        let value = f.cell.read(cx).value().to_string();
        if let Some(t) = f.model.table.as_mut() {
            t.set(row, col, &value);
        }
        f.note = None;
        cx.notify();
    }

    fn with_focus(&mut self, cx: &mut Context<Self>, act: impl FnOnce(&mut FocusUi)) {
        if let Some(f) = self.focus.as_mut() {
            act(f);
            f.note = None;
            cx.notify();
        }
    }

    /// The overlay, while a block is open.
    pub(super) fn focus_overlay(
        &mut self,
        p: Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let me = cx.entity_id();
        let dark = p.canvas.l < 0.5;
        let f = self.focus.as_mut()?;
        let caps = f.model.caps();
        let kind = f.model.block.kind.clone();
        // The drawing, and the view fitted to it the first time.
        let drawing = drawing(&kind, &f.preview, f.picture.clone(), dark);
        if f.model.needs_fit
            && let Some(d) = &drawing
        {
            let [_, _, w, h] = f.stage.get();
            if w > 0. && h > 0. {
                f.model.view.fit(d.size, (w, h));
                f.model.needs_fit = false;
            } else {
                super::frame::next_frame(window, cx);
            }
        }
        let f = self.focus.as_ref()?;
        let edited = f.model.edited();
        let header = self.focus_header(&kind, caps, edited, drawing.as_ref(), p, window, cx);
        // Each kind's body, by what it offers.
        let body = if caps.grid {
            self.focus_table(p, cx)
        } else if caps.zoom {
            self.focus_drawing(&kind, drawing, p, me, cx)
        } else {
            self.focus_code(p)
        };
        let f = self.focus.as_ref()?;
        let note = f.note.clone();
        let footer = div()
            .flex()
            .items_center()
            .gap_3()
            .px_4()
            .py_2()
            .border_t_1()
            .border_color(p.border)
            .text_xs()
            .text_color(p.faint)
            .child(SharedString::from(
                match (caps.saves && f.answer.is_some(), caps.edit || caps.grid) {
                    (true, _) => "Esc closes · Ctrl+Enter saves to the answer",
                    (false, true) => "Esc closes · edits can be exported",
                    (false, false) => "Esc closes · scroll to zoom, drag to move",
                },
            ))
            .child(div().flex_1())
            .children(note.map(|n| div().text_color(p.muted).child(n)));
        Some(
            div()
                .id("focus-backdrop")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                // The chat stays faintly visible behind a frosted veil.
                .bg(p.canvas.opacity(0.82))
                .occlude()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, _, cx| this.close_focus(cx)),
                )
                .child(
                    div()
                        .id("focus-panel")
                        .absolute()
                        .top(px(36.))
                        .bottom(px(36.))
                        .left(px(48.))
                        .right(px(48.))
                        .flex()
                        .flex_col()
                        .rounded_xl()
                        .border_1()
                        .border_color(p.border)
                        .bg(p.canvas)
                        .shadow_lg()
                        .overflow_hidden()
                        .occlude()
                        .child(header)
                        .child(div().flex_1().min_h_0().flex().flex_col().child(body))
                        .child(footer),
                )
                .into_any_element(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn focus_header(
        &self,
        kind: &Kind,
        caps: focus::Caps,
        edited: bool,
        drawing: Option<&Drawing>,
        p: Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let f = self.focus.as_ref().expect("focus is open");
        let text = f.model.text();
        let (stem, formats) = match kind {
            Kind::Code { lang } => crate::blockexport::code_formats(lang, &text),
            Kind::Table => {
                let t = f.model.table.clone().unwrap_or_default();
                crate::blockexport::table_formats(&t.headers, &t.rows, &text)
            }
            Kind::Math => {
                crate::blockexport::math_formats(&text, drawing.and_then(|d| d.svg.as_deref()))
            }
            Kind::Mermaid => {
                crate::blockexport::mermaid_formats(&text, drawing.and_then(|d| d.svg.as_deref()))
            }
            Kind::Image { .. } => picture_formats(&text, drawing.and_then(|d| d.svg.as_deref())),
        };
        let copy: Arc<str> = Arc::from(if matches!(kind, Kind::Image { .. }) {
            ""
        } else {
            text.as_str()
        });
        let tools = crate::blockexport::toolbar("focus", copy, stem, formats, None, window, cx);
        let versions = f.model.versions.count();
        let current = f.model.versions.index();
        let btn = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .px_2()
                .py(px(3.))
                .rounded_md()
                .text_xs()
                .cursor_pointer()
                .text_color(p.muted)
                .hover(move |d| d.bg(p.hover).text_color(p.text))
                .child(label)
        };
        let version_chip = (versions > 1).then(|| {
            let mut row = div().flex().items_center().gap_1();
            for i in 0..versions {
                let lit = i == current;
                let label = if i == 0 {
                    SharedString::from("Original")
                } else {
                    SharedString::from(format!("v{}", i + 1))
                };
                row = row.child(
                    div()
                        .id(("focus-version", i))
                        .px_1p5()
                        .py(px(1.))
                        .rounded(px(4.))
                        .text_xs()
                        .cursor_pointer()
                        .text_color(if lit { p.text } else { p.faint })
                        .when(lit, |d| d.bg(p.hover))
                        .hover(move |d| d.text_color(p.text))
                        .child(label)
                        .on_click(
                            cx.listener(move |this, _, window, cx| this.focus_pick(i, window, cx)),
                        ),
                );
            }
            row
        });
        let saves = caps.saves && f.answer.is_some();
        div()
            .flex()
            .items_center()
            .gap_2()
            .pl_4()
            .pr_2()
            .py_2()
            .border_b_1()
            .border_color(p.border)
            .child(
                div()
                    .text_sm()
                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                    .text_color(p.text)
                    .child(kind.label()),
            )
            .when_some(
                match kind {
                    Kind::Code { lang } if !lang.is_empty() => Some(lang.clone()),
                    _ => None,
                },
                |d, lang| {
                    d.child(
                        div()
                            .text_xs()
                            .text_color(p.faint)
                            .child(SharedString::from(lang)),
                    )
                },
            )
            .when(edited, |d| {
                d.child(div().text_xs().text_color(p.accent).child("Edited"))
            })
            .children(version_chip)
            .child(div().flex_1())
            .when(matches!(kind, Kind::Code { .. }), |d| {
                let wrap = f.model.wrap;
                d.child(
                    btn("focus-wrap", if wrap { "Wrap: on" } else { "Wrap: off" }).on_click(
                        cx.listener(|this, _, window, cx| {
                            let Some(f) = this.focus.as_mut() else {
                                return;
                            };
                            f.model.wrap = !f.model.wrap;
                            let wrap = f.model.wrap;
                            if let Some(e) = f.editor.clone() {
                                e.update(cx, |e, cx| e.set_soft_wrap(wrap, window, cx));
                            }
                            cx.notify();
                        }),
                    ),
                )
                .child(
                    btn("focus-find", "Find").on_click(cx.listener(|this, _, window, cx| {
                        if let Some(e) = this.focus.as_ref().and_then(|f| f.editor.clone()) {
                            let handle = e.read(cx).focus_handle(cx);
                            e.update(cx, |e, cx| e.focus(window, cx));
                            handle.dispatch_action(&Search, window, cx);
                        }
                    })),
                )
            })
            .when(!matches!(kind, Kind::Image { .. }), |d| {
                d.child(
                    btn("focus-ask", "Ask about this")
                        .on_click(cx.listener(|this, _, window, cx| this.focus_ask(window, cx))),
                )
            })
            .child(tools)
            .when(saves, |d| {
                d.child(
                    div()
                        .id("focus-save")
                        .px_2p5()
                        .py(px(4.))
                        .rounded_md()
                        .text_xs()
                        .font_weight(gpui_kit::FontWeight::MEDIUM)
                        .when(edited, |d| {
                            d.cursor_pointer()
                                .bg(p.text)
                                .text_color(p.canvas)
                                .hover(|d| d.opacity(0.9))
                        })
                        .when(!edited, |d| d.bg(p.hover).text_color(p.faint))
                        .child("Save to answer")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.focus_cell_done(cx);
                            this.focus_save(window, cx)
                        })),
                )
            })
            .child(
                div()
                    .id("focus-close")
                    .p_1()
                    .rounded_md()
                    .cursor_pointer()
                    .text_color(p.faint)
                    .hover(move |d| d.bg(p.hover).text_color(p.text))
                    .child(gpui_kit::component::Icon::new(AssetIcon::X).size(px(14.)))
                    .on_click(cx.listener(|this, _, _, cx| this.close_focus(cx))),
            )
            .into_any_element()
    }

    fn focus_code(&self, p: Palette) -> AnyElement {
        let Some(e) = self.focus.as_ref().and_then(|f| f.editor.clone()) else {
            return div().into_any_element();
        };
        div()
            .flex_1()
            .min_h_0()
            .p_2()
            .bg(p.raised)
            .child(
                CodeEditor::new(&e)
                    .bordered(false)
                    .h(gpui_kit::relative(1.)),
            )
            .into_any_element()
    }

    fn focus_table(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let Some(f) = self.focus.as_ref() else {
            return div().into_any_element();
        };
        let Some(t) = f.model.table.as_ref() else {
            return div().into_any_element();
        };
        let cols = t.cols();
        let shown = t.visible(&f.model.filter);
        let editing = f.editing;
        let sort = f.model.sort;
        let small = |id: SharedString, icon: AssetIcon, tip: &'static str| {
            div()
                .id(id)
                .flex_none()
                .p(px(2.))
                .rounded(px(4.))
                .cursor_pointer()
                .text_color(p.faint)
                .hover(move |d| d.bg(p.hover).text_color(p.text))
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(tip).build(window, cx)
                })
                .child(gpui_kit::component::Icon::new(icon).size(px(11.)))
        };
        let cell_box = |id: SharedString| {
            div()
                .id(id)
                .w(px(170.))
                .flex_none()
                .min_h(px(30.))
                .px_2()
                .py_1()
                .border_r_1()
                .border_color(p.border)
                .text_sm()
        };
        let field = f.cell.clone();
        let mut header = div()
            .flex()
            .bg(p.raised)
            .border_b_1()
            .border_color(p.border);
        for c in 0..cols {
            let name = t.headers.get(c).cloned().unwrap_or_default();
            let arrow = match sort {
                Some((sc, true)) if sc == c => " ↑",
                Some((sc, false)) if sc == c => " ↓",
                _ => "",
            };
            let body: AnyElement = if editing == Some((None, c)) {
                Input::new(&field).small().into_any_element()
            } else {
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .id(("focus-sort", c))
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .cursor_pointer()
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .text_color(p.text)
                            .child(SharedString::from(format!("{name}{arrow}")))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.with_focus(cx, |f| f.model.sort_by(c))
                            })),
                    )
                    .child(
                        small(
                            SharedString::from(format!("focus-rename-{c}")),
                            AssetIcon::Pencil,
                            "Rename this column",
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| this.focus_cell_edit(None, c, window, cx),
                        )),
                    )
                    .when(cols > 1, |d| {
                        d.child(
                            small(
                                SharedString::from(format!("focus-dropcol-{c}")),
                                AssetIcon::X,
                                "Remove this column",
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.with_focus(cx, |f| {
                                        if let Some(t) = f.model.table.as_mut() {
                                            t.remove_col(c);
                                        }
                                        f.model.sort = None;
                                    })
                                },
                            )),
                        )
                    })
                    .into_any_element()
            };
            header = header.child(cell_box(SharedString::from(format!("focus-h-{c}"))).child(body));
        }
        header = header.child(div().w(px(28.)).flex_none());
        let mut rows = div().flex().flex_col();
        for &r in &shown {
            let mut line = div()
                .flex()
                .border_b_1()
                .border_color(p.border)
                .hover(move |d| d.bg(p.hover.opacity(0.4)));
            for c in 0..cols {
                let id = SharedString::from(format!("focus-c-{r}-{c}"));
                let body: AnyElement = if editing == Some((Some(r), c)) {
                    Input::new(&field).small().into_any_element()
                } else {
                    div()
                        .text_color(p.text)
                        .child(SharedString::from(t.cell(r, c).to_owned()))
                        .into_any_element()
                };
                line = line.child(cell_box(id).cursor_text().child(body).on_click(cx.listener(
                    move |this, _, window, cx| {
                        if this.focus.as_ref().and_then(|f| f.editing) != Some((Some(r), c)) {
                            this.focus_cell_edit(Some(r), c, window, cx)
                        }
                    },
                )));
            }
            line = line.child(
                div()
                    .w(px(28.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        small(
                            SharedString::from(format!("focus-droprow-{r}")),
                            AssetIcon::Trash,
                            "Remove this row",
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.with_focus(cx, |f| {
                                f.editing = None;
                                if let Some(t) = f.model.table.as_mut() {
                                    t.remove_row(r);
                                }
                            })
                        })),
                    ),
            );
            rows = rows.child(line);
        }
        let hidden = t.rows.len() - shown.len();
        let bar = div()
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(p.border)
            .child(div().w(px(220.)).child(Input::new(&f.filter).small()))
            .child(
                div()
                    .id("focus-addrow")
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .py(px(3.))
                    .rounded_md()
                    .text_xs()
                    .cursor_pointer()
                    .text_color(p.muted)
                    .hover(move |d| d.bg(p.hover).text_color(p.text))
                    .child(gpui_kit::component::Icon::new(AssetIcon::Plus).size(px(12.)))
                    .child("Row")
                    .on_click(cx.listener(|this, _, window, cx| {
                        let Some(r) = this
                            .focus
                            .as_mut()
                            .and_then(|f| f.model.table.as_mut())
                            .map(|t| t.add_row(None))
                        else {
                            return;
                        };
                        if let Some(f) = this.focus.as_mut() {
                            f.model.filter.clear();
                            let field = f.filter.clone();
                            field.update(cx, |i, cx| i.set_value("", window, cx));
                        }
                        this.focus_cell_edit(Some(r), 0, window, cx);
                    })),
            )
            .child(
                div()
                    .id("focus-addcol")
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .py(px(3.))
                    .rounded_md()
                    .text_xs()
                    .cursor_pointer()
                    .text_color(p.muted)
                    .hover(move |d| d.bg(p.hover).text_color(p.text))
                    .child(gpui_kit::component::Icon::new(AssetIcon::Plus).size(px(12.)))
                    .child("Column")
                    .on_click(cx.listener(|this, _, window, cx| {
                        let Some(c) = this
                            .focus
                            .as_mut()
                            .and_then(|f| f.model.table.as_mut())
                            .map(|t| t.add_col("New column"))
                        else {
                            return;
                        };
                        this.focus_cell_edit(None, c, window, cx);
                    })),
            )
            .child(div().flex_1())
            .child(
                div()
                    .text_xs()
                    .text_color(p.faint)
                    .child(SharedString::from(match hidden {
                        0 => format!(
                            "{} rows · click a cell to edit, a heading to sort",
                            t.rows.len()
                        ),
                        n => format!(
                            "{} of {} rows shown ({n} filtered out)",
                            shown.len(),
                            t.rows.len()
                        ),
                    })),
            );
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(bar)
            .child(
                div()
                    .id("focus-grid")
                    .flex_1()
                    .min_h_0()
                    .overflow_scroll()
                    .child(div().flex().flex_col().child(header).child(rows)),
            )
            .into_any_element()
    }

    fn focus_drawing(
        &self,
        kind: &Kind,
        drawing: Option<Drawing>,
        p: Palette,
        me: gpui_kit::EntityId,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(f) = self.focus.as_ref() else {
            return div().into_any_element();
        };
        let view = f.model.view;
        let stage = f.stage.clone();
        let error = match (&drawing, kind) {
            (None, Kind::Math) => Some("RaTeX can't read this formula yet.".to_owned()),
            (None, Kind::Mermaid) => Some(
                crate::richmd::draw_mermaid_or_error(&f.preview)
                    .err()
                    .unwrap_or_else(|| "Mermaid can't draw this yet.".to_owned()),
            ),
            _ => None,
        };
        let pic = drawing.map(|d| {
            img(d.image)
                .absolute()
                .left(px(view.pan.0))
                .top(px(view.pan.1))
                .w(px(d.size.0 * view.zoom))
                .h(px(d.size.1 * view.zoom))
                .object_fit(gpui_kit::ObjectFit::Fill)
        });
        let ctl = |id: &'static str, icon: AssetIcon, tip: &'static str| {
            div()
                .id(id)
                .p_1()
                .rounded_md()
                .cursor_pointer()
                .text_color(p.muted)
                .hover(move |d| d.bg(p.hover).text_color(p.text))
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(tip).build(window, cx)
                })
                .child(gpui_kit::component::Icon::new(icon).size(px(14.)))
        };
        let zoom_by = |k: f32| {
            move |this: &mut Self,
                  _: &gpui_kit::ClickEvent,
                  _: &mut Window,
                  cx: &mut Context<Self>| {
                this.with_focus(cx, |f| {
                    let [_, _, w, h] = f.stage.get();
                    f.model.view.zoom_at(k, (w / 2., h / 2.));
                })
            }
        };
        let direction = match kind {
            Kind::Mermaid => focus::mermaid_direction(&f.model.draft),
            _ => None,
        };
        let controls = div()
            .absolute()
            .bottom(px(12.))
            .right(px(12.))
            .flex()
            .items_center()
            .gap_1()
            .p_1()
            .rounded_lg()
            .border_1()
            .border_color(p.border)
            .bg(p.raised)
            .shadow_md()
            .occlude()
            .children(direction.map(|dir| {
                let seg = |id: &'static str, label: &'static str, d: Dir| {
                    let lit = dir == d;
                    div()
                        .id(id)
                        .px_1p5()
                        .py(px(1.))
                        .rounded(px(4.))
                        .text_xs()
                        .cursor_pointer()
                        .text_color(if lit { p.text } else { p.faint })
                        .when(lit, |x| x.bg(p.hover))
                        .child(label)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.focus_direction(d, window, cx)
                        }))
                };
                div()
                    .flex()
                    .items_center()
                    .gap(px(1.))
                    .mr_1()
                    .child(seg("focus-dir-lr", "Left → right", Dir::LR))
                    .child(seg("focus-dir-tb", "Top ↓ bottom", Dir::TB))
            }))
            .child(
                ctl("focus-zoom-out", AssetIcon::ZoomOut, "Zoom out")
                    .on_click(cx.listener(zoom_by(1. / 1.25))),
            )
            .child(
                div()
                    .w(px(44.))
                    .text_center()
                    .text_xs()
                    .text_color(p.muted)
                    .child(SharedString::from(format!("{}%", view.percent()))),
            )
            .child(
                ctl("focus-zoom-in", AssetIcon::ZoomIn, "Zoom in")
                    .on_click(cx.listener(zoom_by(1.25))),
            )
            .child(
                ctl("focus-fit", AssetIcon::Scan, "Fit to the window").on_click(
                    cx.listener(|this, _, _, cx| this.with_focus(cx, |f| f.model.needs_fit = true)),
                ),
            );
        let stage_el = div()
            .id("focus-stage")
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_hidden()
            .bg(p.raised)
            .cursor_grab()
            .child(
                canvas(
                    |_, _, _: &mut App| (),
                    move |b, _, window, _: &mut App| {
                        let now = [
                            f32::from(b.origin.x),
                            f32::from(b.origin.y),
                            f32::from(b.size.width),
                            f32::from(b.size.height),
                        ];
                        if now != stage.get() {
                            stage.set(now);
                            super::frame::next_frame_for(window, me);
                        }
                    },
                )
                .absolute()
                .inset_0(),
            )
            .children(pic)
            .children(error.map(|e| {
                div()
                    .absolute()
                    .top(px(12.))
                    .left(px(12.))
                    .right(px(12.))
                    .text_xs()
                    .text_color(p.faint)
                    .child(SharedString::from(e))
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, e: &MouseDownEvent, _, cx| {
                    if let Some(f) = this.focus.as_mut() {
                        f.drag = Some((f32::from(e.position.x), f32::from(e.position.y)));
                    }
                    cx.stop_propagation();
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &gpui_kit::MouseUpEvent, _, _| {
                    if let Some(f) = this.focus.as_mut() {
                        f.drag = None;
                    }
                }),
            )
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| {
                let Some(f) = this.focus.as_mut() else {
                    return;
                };
                let Some((x0, y0)) = f.drag else {
                    return;
                };
                if e.pressed_button != Some(MouseButton::Left) {
                    f.drag = None;
                    return;
                }
                let (x, y) = (f32::from(e.position.x), f32::from(e.position.y));
                f.model.view.pan_by(x - x0, y - y0);
                f.drag = Some((x, y));
                cx.notify();
            }))
            .on_scroll_wheel(cx.listener(|this, e: &ScrollWheelEvent, _, cx| {
                cx.stop_propagation();
                let dy = f32::from(e.delta.pixel_delta(px(20.)).y);
                if dy.abs() < f32::EPSILON {
                    return;
                }
                this.with_focus(cx, |f| {
                    let [sx, sy, _, _] = f.stage.get();
                    let at = (f32::from(e.position.x) - sx, f32::from(e.position.y) - sy);
                    // Away from you zooms in.
                    f.model.view.zoom_at((dy * 0.004).exp(), at);
                })
            }))
            .child(controls);
        let editor = f.editor.clone().filter(|_| f.model.caps().edit);
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .children(editor.map(|e| {
                div()
                    .w(px(380.))
                    .flex_none()
                    .h_full()
                    .p_2()
                    .border_r_1()
                    .border_color(p.border)
                    .child(
                        CodeEditor::new(&e)
                            .bordered(false)
                            .h(gpui_kit::relative(1.)),
                    )
            }))
            .child(stage_el)
            .into_any_element()
    }

    /// Turns a flowchart left-to-right or top-to-bottom (in the source).
    fn focus_direction(&mut self, dir: Dir, window: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = self.focus.as_mut() else {
            return;
        };
        let text = focus::set_mermaid_direction(&f.model.draft, dir);
        if text == f.model.draft {
            return;
        }
        f.model.draft = text.clone();
        f.preview = text.clone();
        f.model.needs_fit = true;
        f.note = None;
        if let Some(e) = f.editor.clone() {
            e.update(cx, |e, cx| e.set_value(text, window, cx));
        }
        cx.notify();
    }

    /// The UI tour's hand on the focus view (`tour::Act::Focus`).
    pub(super) fn tour_focus(&mut self, what: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.focus.is_none() {
            super::tour::log(&format!(
                "TOUR error focus {what}: the focus view is not open"
            ));
            return;
        }
        match what {
            "close" => self.close_focus(cx),
            "save" => self.focus_save(window, cx),
            "original" => self.focus_pick(0, window, cx),
            "tb" => self.focus_direction(Dir::TB, window, cx),
            "cell" => {
                self.focus_cell_edit(Some(0), 0, window, cx);
                if let Some(f) = self.focus.as_ref() {
                    let field = f.cell.clone();
                    field.update(cx, |c, cx| {
                        c.set_value("Always retrieve (baseline)", window, cx)
                    });
                }
                self.focus_cell_done(cx);
            }
            "addrow" => {
                self.with_focus(cx, |f| {
                    if let Some(t) = f.model.table.as_mut() {
                        let r = t.add_row(None);
                        t.set(Some(r), 0, "CRAG");
                        t.set(Some(r), 1, "0.80");
                        t.set(Some(r), 2, "700");
                    }
                });
            }
            "zoom" => self.with_focus(cx, |f| {
                let [_, _, w, h] = f.stage.get();
                f.model.view.zoom_at(1.6, (w / 2., h / 2.));
            }),
            "pan" => self.with_focus(cx, |f| f.model.view.pan_by(-120., 60.)),
            "wrap" => {
                if let Some(f) = self.focus.as_mut() {
                    f.model.wrap = !f.model.wrap;
                    let wrap = f.model.wrap;
                    if let Some(e) = f.editor.clone() {
                        e.update(cx, |e, cx| e.set_soft_wrap(wrap, window, cx));
                    }
                    cx.notify();
                }
            }
            w if w.starts_with("sort") => {
                let c = w[4..].parse().unwrap_or(0);
                self.with_focus(cx, |f| f.model.sort_by(c));
            }
            other => super::tour::log(&format!("TOUR error focus: no action {other}")),
        }
        let state = self.focus.as_ref().map(|f| {
            format!(
                "{} edited={} versions={} zoom={}% note={:?}",
                f.model.block.kind.label(),
                f.model.edited(),
                f.model.versions.count(),
                f.model.view.percent(),
                f.note.as_deref().unwrap_or("")
            )
        });
        super::tour::log(&format!(
            "TOUR focus {what}: {}",
            state.unwrap_or_else(|| "closed".to_owned())
        ));
    }

    /// A sent picture's card, clickable to open it large.
    pub(super) fn sent_picture(
        &self,
        c: &tern_agent::Chip,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let image = self.named_preview(&c.name);
        let card = super::sent_card(c, image.clone(), p);
        match image {
            Some(img) => {
                let name = c.name.clone();
                div()
                    .id(SharedString::from(format!("sent-pic-{name}")))
                    .cursor_pointer()
                    .child(card)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_picture(&name, img.clone(), window, cx)
                    }))
                    .into_any_element()
            }
            None => card,
        }
    }
}

/// What a picture saves as: its SVG (and a PNG of it) when it has one.
fn picture_formats(name: &str, svg: Option<&str>) -> (String, Vec<crate::blockexport::Format>) {
    use crate::blockexport::{Data, Format};
    let stem = crate::blockexport::slug(name.rsplit_once('.').map_or(name, |(s, _)| s), "picture");
    let f = svg
        .map(|svg| {
            vec![
                Format {
                    label: "SVG",
                    ext: "svg",
                    data: Data::Text(Arc::from(svg)),
                },
                Format {
                    label: "PNG",
                    ext: "png",
                    data: Data::Png(Arc::from(svg), 2.0),
                },
            ]
        })
        .unwrap_or_default();
    (stem, f)
}

/// The drawing for a block's current source, if it has one.
fn drawing(
    kind: &Kind,
    source: &str,
    picture: Option<Arc<gpui_kit::Image>>,
    dark: bool,
) -> Option<Drawing> {
    let pick = |a: &crate::richmd::Art| {
        if dark {
            a.dark.clone()
        } else {
            a.light.clone()
        }
    };
    match kind {
        Kind::Math => {
            let art = crate::richmd::typeset(source, true)?;
            Some(Drawing {
                image: pick(&art),
                size: (art.width * MATH_SCALE, art.height * MATH_SCALE),
                svg: Some(String::from_utf8_lossy(art.light.bytes()).into_owned()),
            })
        }
        Kind::Mermaid => {
            let art = crate::richmd::draw_mermaid_or_error(source).ok()?;
            Some(Drawing {
                image: pick(&art),
                size: (art.width, art.height),
                svg: Some(String::from_utf8_lossy(art.light.bytes()).into_owned()),
            })
        }
        Kind::Image { svg: true } => {
            let image = Arc::new(gpui_kit::Image::from_bytes(
                gpui_kit::ImageFormat::Svg,
                source.as_bytes().to_vec(),
            ));
            let size = crate::richmd::svg_size(source).unwrap_or((640., 480.));
            Some(Drawing {
                image,
                size,
                svg: Some(source.to_owned()),
            })
        }
        Kind::Image { svg: false } => {
            let image = picture?;
            let size = crate::thumbs::image_size(image.bytes()).unwrap_or((800., 600.));
            Some(Drawing {
                image,
                size,
                svg: None,
            })
        }
        Kind::Code { .. } | Kind::Table => None,
    }
}
