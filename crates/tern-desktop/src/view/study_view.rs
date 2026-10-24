//! The Study harness in the window: the Study home (materials, topics with
//! mastery rings, and the Tutor, Quiz, and Viva actions), the tutor chat
//! with its mode chips, the quiz card (one question at a time with instant
//! feedback and a score summary), and the viva screen with its timer and
//! rubric report. Flashcards (review and deck) are in `cards_view`; the
//! Explain in 3D action on a topic opens an explainer tab
//! (`explain_view`).
//!
//! Study is told apart from Research by tone and shape, not hue: a warmer
//! surface, round markers, pill radius, and italic serif labels.

use std::time::{Duration, Instant};

use gpui_kit::assets::IconName as AssetIcon;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::progress::ProgressCircle;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::text::TextView;
use gpui_kit::component::{Disableable as _, Selectable as _, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, Entity, SharedString, Subscription, Window, div, px, svg};
use tern_agent::Harness;
use tern_agent::study::{
    Difficulty, Mark, Material, MaterialKind, QuestionKind, QuizSpec, Topic, TutorMode,
};

use super::Workspace;
use crate::assets;
use crate::bridge::StudyMsg;
use crate::model::Screen;
use crate::study::{StudyTab, StudyView, fmt_score};
use crate::theme::Palette;

/// Study's pill radius.
pub(super) const PILL: f32 = 16.;

/// The Study screen's text fields.
pub(super) struct StudyInputs {
    /// The tutor's message box and a quiz's typed answers.
    pub answer: Entity<InputState>,
    /// A viva answer: a paragraph.
    pub viva: Entity<TextareaState>,
    /// What the viva is on.
    pub title: Entity<InputState>,
    /// A card's two sides on the deck screen.
    pub card_front: Entity<InputState>,
    pub card_back: Entity<InputState>,
    /// A page or online PDF to add as a material.
    pub link: Entity<InputState>,
}

impl StudyInputs {
    pub fn new(window: &mut Window, cx: &mut Context<Workspace>) -> (Self, Vec<Subscription>) {
        let answer = cx.new(|cx| InputState::new(window, cx).placeholder("Type your answer"));
        let viva = cx.new(|cx| {
            TextareaState::new(window, cx).auto_grow(3, 10).placeholder(
                "Answer as you would in the room. Enter sends, Shift+Enter for a new line.",
            )
        });
        let title = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("What is being examined (defaults to your report)")
        });
        let card_front = cx.new(|cx| InputState::new(window, cx).placeholder("Front"));
        let card_back = cx.new(|cx| InputState::new(window, cx).placeholder("Back"));
        let link = cx
            .new(|cx| InputState::new(window, cx).placeholder("https://… a page or an online PDF"));
        let subs = vec![
            cx.subscribe_in(
                &answer,
                window,
                |this: &mut Workspace, _, event: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { .. } = event {
                        this.study_enter(window, cx);
                    }
                },
            ),
            cx.subscribe_in(
                &viva,
                window,
                |this: &mut Workspace, _, event: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { shift: false, .. } = event {
                        this.viva_send(false, window, cx);
                    }
                },
            ),
        ];
        (
            Self {
                answer,
                viva,
                title,
                card_front,
                card_back,
                link,
            },
            subs,
        )
    }
}

fn glyph(name: &str, size: f32, color: gpui_kit::Hsla) -> AnyElement {
    match assets::glyph(name) {
        Some(path) => svg()
            .path(path)
            .size(px(size))
            .flex_none()
            .text_color(color)
            .into_any_element(),
        None => div().size(px(size)).flex_none().into_any_element(),
    }
}

fn icon(name: AssetIcon, color: gpui_kit::Hsla) -> AnyElement {
    gpui_kit::component::Icon::new(name)
        .small()
        .text_color(color)
        .into_any_element()
}

/// An italic serif label, Study's voice.
pub(super) fn serif(text: impl Into<SharedString>, p: Palette) -> gpui_kit::Div {
    div()
        .font_family(assets::SERIF_FONT)
        .italic()
        .text_color(p.text)
        .child(text.into())
}

fn material_icon(m: &Material, p: Palette) -> AnyElement {
    if m.kind == MaterialKind::Link
        && let Some(service) = url_site(&m.origin).and_then(|s| assets::service_for(&s))
    {
        return super::service_logo(service, 16., p, p.muted);
    }
    let name = match m.kind {
        MaterialKind::Pdf => AssetIcon::FileText,
        MaterialKind::Note => AssetIcon::NotebookPen,
        MaterialKind::Vault => AssetIcon::BookMarked,
        MaterialKind::Link => AssetIcon::Globe,
        MaterialKind::Report => AssetIcon::ScrollText,
        MaterialKind::Document => AssetIcon::Presentation,
    };
    icon(name, p.muted)
}

fn url_site(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    Some(
        rest.split('/')
            .next()?
            .trim_start_matches("www.")
            .to_owned(),
    )
}

fn mark_glyph(mark: Mark) -> &'static str {
    match mark {
        Mark::Correct => "verdict-supported",
        Mark::Partly => "verdict-partial",
        Mark::Incorrect => "verdict-unsupported",
        Mark::Unanswered => "verdict-unverified",
    }
}

impl Workspace {
    /// The harness switcher: Study opens its screen and loads the library.
    /// Each harness comes back to the chat it was left on.
    pub(super) fn choose_harness(&mut self, h: Harness, cx: &mut Context<Self>) {
        self.switch_harness(h, None, cx);
    }

    /// Where the open harness is now: its chat, the unsent draft, the
    /// transcript's scroll and the panels.
    pub(super) fn current_place(&self, cx: &mut Context<Self>) -> tern_agent::prefs::LastChat {
        tern_agent::prefs::LastChat {
            chat: self
                .session
                .as_ref()
                .filter(|s| !s.is_draft())
                .map(|s| s.session_id())
                .or(self.current)
                .map(|id| id.to_string()),
            draft: self.composer.read(cx).value().to_string(),
            scroll: -(f32::from(self.scroll.offset().y)).round() as i64,
            side_open: self.model.cont.side.open,
            browser_open: self.model.browser.visible,
        }
    }

    /// Puts back the draft, scroll and panels of the harness just opened,
    /// once its chat is on screen.
    pub(super) fn apply_restore(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((h, last)) = self.pending_restore.take() else {
            return;
        };
        if h != self.model.harness {
            return;
        }
        let here = self.current.map(|id| id.to_string());
        if last.chat.is_some() && last.chat != here {
            return;
        }
        let draft = last.draft.clone();
        self.composer
            .update(cx, |c, cx| c.set_value(draft, window, cx));
        if last.scroll > 0 {
            self.scroll
                .set_offset(gpui_kit::point(px(0.), px(-(last.scroll as f32))));
        }
        self.model.cont.side.open = last.side_open;
        self.model.browser.visible = last.browser_open && self.model.browser.page.is_some();
        cx.notify();
    }

    /// Switches to harness `h`, opening `open` there (a chat of that
    /// harness picked from somewhere else) or the chat it was left on.
    pub(super) fn switch_harness(
        &mut self,
        h: Harness,
        open: Option<tern_core::SessionId>,
        cx: &mut Context<Self>,
    ) {
        self.switcher = None;
        let from = self.model.harness;
        if h != from {
            if self.model.is_busy() {
                self.model.notice_line(
                    "Stop or finish the running answer before switching between Research and Study."
                        .into(),
                );
                cx.notify();
                return;
            }
            // Each harness has its own chat: Study questions never land in
            // the Research chat, and switching back reopens where you were.
            if let Some(s) = &self.session
                && !s.is_draft()
            {
                self.parked.insert(from, s.session_id());
            }
            let place = self.current_place(cx);
            self.harness_memory.record(from, place);
            let memory = self.harness_memory.clone();
            self.save_ui_prefs(|p| memory.save_into(p));
            let target = open
                .or_else(|| self.harness_memory.chat(h))
                .or_else(|| self.parked.get(&h).copied());
            // A chat picked from elsewhere opens clean; otherwise the
            // harness's own place comes back. Either way the old harness's
            // draft never follows into this one.
            self.pending_restore = Some(match open {
                Some(id) => (
                    h,
                    tern_agent::prefs::LastChat {
                        chat: Some(id.to_string()),
                        ..Default::default()
                    },
                ),
                None => (h, self.harness_memory.get(h).clone()),
            });
            // The right pane belongs to the chat it was opened from.
            self.model.cont.side.open = false;
            self.model.browser.visible = false;
            self.session = None;
            self.current = None;
            self.model.new_chat();
            self.model.phase = crate::model::Phase::Connecting;
            // Each harness keeps its own chat tabs.
            self.tabs_restored = true;
            self.stash_tab_place(cx);
            let current = std::mem::take(&mut self.tabs);
            let known = crate::harness::only(&self.model.all_sessions, h);
            self.tabs = self.parked_tabs.swap(from, h, current).unwrap_or_else(|| {
                tern_agent::Preferences::load()
                    .open_tabs
                    .get(crate::harness::key(h))
                    .map(|t| crate::layout::Tabs::from_saved(t, &known))
                    .unwrap_or_default()
            });
            match open.or(target) {
                Some(id) => {
                    let title = known
                        .iter()
                        .find(|s| s.id == id)
                        .map(|s| s.title.clone())
                        .unwrap_or_else(|| "Chat".into());
                    self.tabs.open(id, &title);
                }
                None => {
                    if self.tabs.active_tab().id.is_some() {
                        self.tabs.add();
                    }
                }
            }
            self.bridge.launch(h, target);
        }
        self.model.choose_harness(h);
        // The composer keeps its mode only if it belongs to this harness.
        self.model.composer.harness_changed(h);
        if h == Harness::Study {
            self.model.study.busy = Some("Opening your study library…".into());
            self.bridge.study_load();
            self.bridge.cards_load();
        }
        cx.notify();
    }

    /// The Study harness's own chat: its transcript and composer. It is
    /// a separate chat from Research, filed under Study.
    pub(super) fn open_study_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.model.harness != Harness::Study {
            self.choose_harness(Harness::Study, cx);
        }
        self.model.study.set_tab(StudyTab::Chat);
        self.model.study.materials_open = false;
        self.model.screen = crate::model::Screen::Chat;
        self.composer.update(cx, |c, cx| c.focus(window, cx));
        cx.notify();
    }

    pub(super) fn on_study(&mut self, m: StudyMsg, _window: &mut Window, cx: &mut Context<Self>) {
        let s = &mut self.model.study;
        match m {
            StudyMsg::Library(Ok(lib), note, who) => {
                s.loaded(lib);
                if note.is_some() {
                    s.notice = note;
                }
                if who.is_some() || s.engine.is_none() {
                    s.engine = who;
                }
            }
            StudyMsg::Library(Err(e), ..) => s.fail(e),
            StudyMsg::TutorText(t) => s.tutor_text(&t),
            StudyMsg::Tutor(r, lib) => {
                s.tutor_done(r);
                if let Some(l) = lib {
                    s.library = l;
                }
                self.scroll.scroll_to_bottom();
            }
            StudyMsg::Quiz(Ok((quiz, note))) => s.quiz_ready(quiz, note, tern_agent::study::now()),
            StudyMsg::Quiz(Err(e)) => s.fail(e),
            StudyMsg::Graded(n, given, grade) => {
                if let Some(run) = &mut s.quiz {
                    run.record(n, given, grade);
                }
            }
            StudyMsg::QuizDone(Ok(summary), lib) => {
                if let Some(l) = lib {
                    s.library = l;
                }
                if let Some(run) = &mut s.quiz {
                    run.done = Some(summary);
                }
                s.busy = None;
            }
            StudyMsg::QuizDone(Err(e), _) => s.fail(e),
            StudyMsg::Viva(Ok(v), note) => {
                s.viva_next(v, Instant::now());
                s.notice = note;
            }
            StudyMsg::Viva(Err(e), _) => {
                s.fail(e);
                if let Some(run) = &mut s.viva {
                    run.thinking = false;
                }
            }
            StudyMsg::VivaReport(Ok((v, report, path)), note, lib) => {
                s.viva_report(v, report, path, Instant::now());
                s.notice = note;
                if let Some(l) = lib {
                    s.library = l;
                }
            }
            StudyMsg::VivaReport(Err(e), ..) => {
                s.fail(e);
                if let Some(run) = &mut s.viva {
                    run.thinking = false;
                }
            }
            StudyMsg::Deck(Ok(deck), note) => {
                s.busy = None;
                // A review in progress keeps its own copy of the deck; the
                // saved one is the same plus nothing new.
                if s.view != StudyView::Cards {
                    s.deck_ready(deck);
                } else {
                    s.deck_loaded = true;
                }
                if note.is_some() {
                    s.notice = note;
                }
            }
            StudyMsg::Deck(Err(e), _) => s.fail(e),
            StudyMsg::Reviewed(Ok(lib)) => s.library = lib,
            StudyMsg::Reviewed(Err(e)) => s.notice = Some(e),
            StudyMsg::Explain(key, r) => self.on_explain(key, r),
            StudyMsg::ExplainSaved(key, r) => {
                if let Some(x) = self.explains.get_mut(&key) {
                    x.saved = Some(match r {
                        Ok(line) => line,
                        Err(e) => e,
                    });
                }
            }
        }
        self.start_study_ticker(cx);
        cx.notify();
    }

    /// Redraws the viva clock once a second while a viva runs.
    fn start_study_ticker(&mut self, cx: &mut Context<Self>) {
        if !self.model.study.ticking() || self.study_ticker.is_some() {
            return;
        }
        self.study_ticker = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let go = this.update(cx, |this, cx| {
                    cx.notify();
                    let t = this.model.study.ticking() && this.model.screen == Screen::Study;
                    if !t {
                        this.study_ticker = None;
                    }
                    t
                });
                if !matches!(go, Ok(true)) {
                    break;
                }
            }
        }));
    }

    fn pick_study_files(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: true,
            directories: true,
            multiple: true,
            prompt: Some("Add to Study".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = picked.await
                && !paths.is_empty()
            {
                let _ = this.update(cx, |this, cx| {
                    let n = paths.len();
                    this.model.study.busy = Some(format!(
                        "Reading {n} item{}…",
                        if n == 1 { "" } else { "s" }
                    ));
                    this.bridge.study_add(paths);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Enter in the answer box: a tutor message, or a typed quiz answer.
    fn study_enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.model.study.view {
            StudyView::Tutor => self.tutor_send(window, cx),
            StudyView::Quiz => self.quiz_submit(window, cx),
            _ => {}
        }
    }

    fn open_tutor(&mut self, topic: Topic, cx: &mut Context<Self>) {
        self.model.study.open_tutor(topic);
        if self.model.study.tutor.is_empty()
            && let Some(h) = self.model.study.tutor_start()
        {
            self.send_tutor(h);
        }
        cx.notify();
    }

    fn send_tutor(&mut self, history: Vec<tern_agent::study::TutorTurn>) {
        let s = &self.model.study;
        if let Some(t) = &s.tutor_topic {
            self.bridge.study_tutor(t.id.clone(), s.tutor_mode, history);
            self.scroll.scroll_to_bottom();
        }
    }

    fn tutor_send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.study_inputs.answer.read(cx).value().to_string();
        if let Some(h) = self.model.study.tutor_say(&text) {
            self.study_inputs
                .answer
                .update(cx, |i, cx| i.set_value("", window, cx));
            self.send_tutor(h);
        }
        cx.notify();
    }

    pub(super) fn start_quiz(&mut self, cx: &mut Context<Self>) {
        let s = &mut self.model.study;
        let spec = QuizSpec {
            topic: s.quiz_topic.as_ref().map(|t| t.id.clone()),
            count: s.quiz_count,
            difficulty: s.quiz_difficulty,
            kinds: Vec::new(),
        };
        s.quiz = None;
        s.busy = Some("Writing your quiz…".into());
        s.notice = None;
        self.bridge.study_quiz(spec);
        cx.notify();
    }

    /// Submits the answer on screen: the picked choice, or the typed text.
    pub(super) fn quiz_submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let typed = self.study_inputs.answer.read(cx).value().to_string();
        let Some(run) = &mut self.model.study.quiz else {
            return;
        };
        if run.grading || run.graded().is_some() {
            return;
        }
        let Some(q) = run.question().cloned() else {
            return;
        };
        let given = match q.kind {
            QuestionKind::MultipleChoice | QuestionKind::TrueFalse => {
                run.picked.clone().unwrap_or_default()
            }
            _ => typed,
        };
        if given.trim().is_empty() {
            return;
        }
        run.grading = true;
        self.study_inputs
            .answer
            .update(cx, |i, cx| i.set_value("", window, cx));
        self.bridge.study_grade(q, given);
        cx.notify();
    }

    pub(super) fn quiz_next(&mut self, cx: &mut Context<Self>) {
        let Some(run) = &mut self.model.study.quiz else {
            return;
        };
        if run.next() {
            let (quiz, attempt) = (run.quiz.clone(), run.attempt.clone());
            self.model.study.busy = Some("Saving your score…".into());
            self.bridge.study_finish_quiz(quiz, attempt);
        }
        cx.notify();
    }

    fn viva_start(&mut self, cx: &mut Context<Self>) {
        let title = self.study_inputs.title.read(cx).value().trim().to_owned();
        let s = &mut self.model.study;
        s.viva = None;
        s.notice = None;
        s.busy = Some("The examiner is reading your materials…".into());
        self.bridge
            .study_viva_start(Some(title).filter(|t| !t.is_empty()), s.viva_questions);
        cx.notify();
    }

    fn viva_send(&mut self, end: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.model.study.view != StudyView::Viva {
            return;
        }
        if !self.model.study.viva_waiting() {
            return;
        }
        let text = self.study_inputs.viva.read(cx).value().to_string();
        let now = Instant::now();
        let Some(run) = &mut self.model.study.viva else {
            return;
        };
        if run.thinking || run.report.is_some() || (text.trim().is_empty() && !end) {
            return;
        }
        let secs = run.answer_secs(now);
        run.thinking = true;
        let viva = run.viva.clone();
        self.study_inputs
            .viva
            .update(cx, |i, cx| i.set_value("", window, cx));
        self.model.study.busy = Some(if end {
            "Scoring your viva…".into()
        } else {
            "The examiner is thinking…".into()
        });
        self.bridge.study_viva_answer(viva, text, secs, end);
        cx.notify();
    }

    // ---- Rendering ----

    /// Switches the one Study window's mode.
    pub(super) fn study_tab(&mut self, tab: StudyTab, cx: &mut Context<Self>) {
        self.model.study.set_tab(tab);
        if tab != StudyTab::Chat {
            self.model.study.materials_open = false;
        }
        self.model.screen = Screen::Study;
        cx.notify();
    }

    /// Builds a 3D explainer inside the Study window's Explain tab.
    pub(super) fn study_explain(
        &mut self,
        ask: tern_agent::explain::Ask,
        label: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // One explainer at a time in the Study window: the last one stops.
        if let Some(old) = self.model.study.explain.take() {
            self.drop_explain(old);
        }
        self.model.study.explain_hint = None;
        self.next_explain += 1;
        let key = self.next_explain;
        self.explains
            .insert(key, crate::explain::ExplainState::new(label));
        self.explain_asks.insert(key, ask.clone());
        self.model.study.explain = Some(key);
        self.model.study.set_tab(StudyTab::Explain);
        self.model.screen = Screen::Study;
        self.bridge.explain_build(key, ask);
        self.start_explain_loader(cx);
        self.keys.focus(window, cx);
        cx.notify();
    }

    /// A Study message that names a mode ("quiz me on paging") switches
    /// to it and starts it instead of going to the chat. `true` when it
    /// was taken.
    pub(super) fn study_intent(
        &mut self,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let text = text.trim();
        if text.is_empty() || text.len() > 140 {
            return false;
        }
        let tab_now = self.model.study.tab;
        let (tab, subject) = match crate::study::intent(text) {
            Some(i) => i,
            // Typed in the Explain tab: the words are the concept.
            None if tab_now == StudyTab::Explain => (StudyTab::Explain, text.to_owned()),
            None => return false,
        };
        let low = subject.to_lowercase();
        let topic = (!low.is_empty())
            .then(|| {
                self.model
                    .study
                    .library
                    .topics
                    .iter()
                    .find(|t| {
                        let tl = t.title.to_lowercase();
                        tl.contains(&low) || low.contains(&tl)
                    })
                    .cloned()
            })
            .flatten();
        match tab {
            StudyTab::Quiz => {
                self.study_tab(StudyTab::Quiz, cx);
                if !self.model.study.library.topics.is_empty() {
                    self.model.study.open_quiz(topic);
                    self.start_quiz(cx);
                }
            }
            StudyTab::Cards => {
                self.study_tab(StudyTab::Cards, cx);
                let now = tern_agent::study::now();
                if self.model.study.due(None, now) > 0 {
                    self.start_review(topic.map(|t| t.id), window, cx);
                } else if !self.model.study.library.materials.is_empty() {
                    self.gen_cards(topic.map(|t| t.id), cx);
                }
            }
            StudyTab::Viva => {
                self.study_tab(StudyTab::Viva, cx);
                if !subject.is_empty() {
                    let s = subject.clone();
                    self.study_inputs
                        .title
                        .update(cx, |i, cx| i.set_value(s, window, cx));
                }
            }
            StudyTab::Explain => {
                let label = if subject.is_empty() {
                    "Explain"
                } else {
                    subject.as_str()
                };
                let ask = tern_agent::explain::Ask {
                    concept: subject.clone(),
                    topic: topic.map(|t| t.id),
                    context: None,
                };
                self.study_explain(ask, label, window, cx);
            }
            StudyTab::Chat => return false,
        }
        true
    }

    /// The one Study window (v2 s14): the mode tabs centred at the top,
    /// the mode in the middle, and the Study composer at the bottom. The
    /// chat and every tool live here; nothing opens a separate page.
    pub(super) fn study_window(
        &self,
        p: Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let s = &self.model.study;
        let tab = s.tab;
        let segs: Vec<super::segmented::Segment> = StudyTab::ALL
            .iter()
            .map(|t| super::segmented::Segment::text(t.label()))
            .collect();
        let tabs = self.segmented(
            "study-tabs",
            &segs,
            tab.index(),
            super::segmented::Look {
                framed: true,
                fill: false,
                small: false,
            },
            p,
            std::rc::Rc::new(|this: &mut Self, i, _, cx| {
                this.study_tab(StudyTab::ALL[i], cx);
            }),
            cx,
        );
        let now = tern_agent::study::now();
        let header = div()
            .w_full()
            .flex()
            .flex_wrap()
            .items_center()
            .justify_center()
            .gap_2()
            .px_4()
            .pt(px(14.))
            .pb_2()
            .child(div().occlude().child(tabs))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(p.faint)
                            .child(SharedString::from(s.status_line(now))),
                    )
                    .when(tab == StudyTab::Chat, |d| {
                        d.child(
                            Button::new("study-materials")
                                .icon(AssetIcon::BookOpen)
                                .label("Materials")
                                .ghost()
                                .small()
                                .selected(s.materials_open)
                                .tooltip("Your materials, topics and progress")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    let s = &mut this.model.study;
                                    s.materials_open = !s.materials_open;
                                    if s.view != StudyView::Tutor {
                                        s.view = StudyView::Home;
                                    }
                                    cx.notify();
                                })),
                        )
                    }),
            );
        let empty_lib = s.library.materials.is_empty();
        let body: AnyElement = match tab {
            StudyTab::Chat => {
                if s.view == StudyView::Tutor || s.materials_open || self.model.entries.is_empty() {
                    self.study_screen(p, window, cx)
                } else {
                    div()
                        .flex_1()
                        .min_h_0()
                        .flex()
                        .flex_col()
                        .child(self.transcript(p, cx))
                        .children(self.range_bar(p, cx).map(|b| {
                            div()
                                .w_full()
                                .max_w(px(super::COLUMN_MAX_W))
                                .mx_auto()
                                .px_4()
                                .pb_1()
                                .child(b)
                        }))
                        .into_any_element()
                }
            }
            StudyTab::Cards if empty_lib && s.deck.cards.is_empty() => {
                self.material_empty(tab, p, cx)
            }
            t if t.needs_materials() && empty_lib && s.quiz.is_none() && s.viva.is_none() => {
                self.material_empty(tab, p, cx)
            }
            StudyTab::Explain => match s.explain.filter(|k| self.explains.contains_key(k)) {
                Some(key) => self.explain_view(key, p, cx),
                None => self.explain_empty(p, cx),
            },
            _ => self.study_screen(p, window, cx),
        };
        div()
            .id("study-window")
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(p.warm)
            .child(header)
            .child(div().flex_1().min_h_0().flex().flex_col().child(body))
            .child(self.composer(p, cx))
            .into_any_element()
    }

    /// A mode that works from materials, with none yet: what it does and
    /// the three ways to add one (a file, a link, the last report).
    fn material_empty(&self, tab: StudyTab, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let busy = self.model.study.busy.is_some();
        let (title, line) = match tab {
            StudyTab::Quiz => (
                "Quizzes from your own sources",
                "Add something to study and Tern writes questions from it, grades each answer and explains every miss with the page it came from.",
            ),
            StudyTab::Viva => (
                "Viva practice",
                "An examiner asks, you defend, Tern scores it. Add your report or notes so the questions come from your own work.",
            ),
            StudyTab::Cards => (
                "Flashcards",
                "Tern writes cards from your materials and schedules them like Anki. Add something to start a deck.",
            ),
            _ => ("Start here", "Add something to study."),
        };
        div()
            .id("study-empty")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .items_center()
            .px_4()
            .py_6()
            .child(
                self.card(p)
                    .w_full()
                    .max_w(px(520.))
                    .gap_3()
                    .p_5()
                    .child(serif(title, p).text_2xl())
                    .child(div().text_sm().text_color(p.muted).child(line))
                    .children(self.study_status(p, cx))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .child(
                                Button::new("study-empty-files")
                                    .icon(AssetIcon::Plus)
                                    .label("Add material")
                                    .primary()
                                    .small()
                                    .disabled(busy)
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.pick_study_files(cx)),
                                    ),
                            )
                            .child(
                                Button::new("study-empty-pdf")
                                    .icon(AssetIcon::FileText)
                                    .label("Upload PDF")
                                    .small()
                                    .disabled(busy)
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.pick_study_files(cx)),
                                    ),
                            )
                            .child(
                                Button::new("study-empty-report")
                                    .icon(AssetIcon::ScrollText)
                                    .label("Use the last report")
                                    .ghost()
                                    .small()
                                    .disabled(busy)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.model.study.busy = Some("Adding your report…".into());
                                        this.bridge.study_add_report("last".into());
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(Input::new(&self.study_inputs.link).small()),
                            )
                            .child(
                                Button::new("study-empty-link")
                                    .icon(AssetIcon::Link)
                                    .label("Paste link")
                                    .small()
                                    .disabled(busy)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.study_add_link(window, cx)
                                    })),
                            ),
                    )
                    .child(div().text_xs().text_color(p.faint).child(
                        "Or ask Tern anything in the box below; it answers without materials.",
                    )),
            )
            .into_any_element()
    }

    /// Adds the page or online PDF in the link field as a material.
    pub(super) fn study_add_link(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let url = self.study_inputs.link.read(cx).value().trim().to_owned();
        if url.is_empty() {
            self.model.study.notice = Some("Paste a link to a page or an online PDF first.".into());
            cx.notify();
            return;
        }
        self.model.study.busy = Some(format!("Reading {url}…"));
        self.bridge.study_add_link(url);
        self.study_inputs
            .link
            .update(cx, |i, cx| i.set_value("", window, cx));
        cx.notify();
    }

    /// The Explain tab before a scene: what it does and the topics to
    /// start from (or type a concept in the box below).
    fn explain_empty(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let topics: Vec<AnyElement> = self
            .model
            .study
            .library
            .topics
            .iter()
            .take(10)
            .enumerate()
            .map(|(i, t)| {
                let topic = t.clone();
                Button::new(("explain-topic", i))
                    .label(SharedString::from(t.title.clone()))
                    .small()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let ask = tern_agent::explain::Ask {
                            concept: topic.title.clone(),
                            topic: Some(topic.id.clone()),
                            context: None,
                        };
                        this.study_explain(ask, &topic.title.clone(), window, cx);
                    }))
                    .into_any_element()
            })
            .collect();
        // The last answer that explains something; a quiz question is
        // skipped (it asks, it does not explain).
        let last_answer = self
            .model
            .entries
            .len()
            .checked_sub(1)
            .and_then(|i| self.model.explanatory_answer(i))
            .and_then(|i| self.model.entry_text(i).map(str::to_owned));
        let hint = self.model.study.explain_hint.clone();
        div()
            .id("explain-empty")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .items_center()
            .px_4()
            .py_6()
            .child(
                self.card(p)
                    .w_full()
                    .max_w(px(560.))
                    .gap_3()
                    .p_5()
                    .child(serif("Explain in 3D", p).text_2xl())
                    .child(div().text_sm().text_color(p.muted).child(
                        "Type a concept below and Tern builds a 3D scene you can turn, step through and save, with every step cited.",
                    ))
                    .when_some(hint, |d, h| {
                        d.child(
                            div()
                                .px_3()
                                .py_1p5()
                                .rounded(px(16.))
                                .border_1()
                                .border_color(p.border)
                                .text_xs()
                                .text_color(p.muted)
                                .child(h),
                        )
                    })
                    .when(!topics.is_empty(), |d| {
                        d.child(div().text_xs().text_color(p.faint).child("From your topics"))
                            .child(div().flex().flex_wrap().gap_2().children(topics))
                    })
                    .when_some(last_answer, |d, text| {
                        d.child(
                            Button::new("explain-last-answer")
                                .icon(AssetIcon::Box)
                                .label("Explain the last answer in 3D")
                                .ghost()
                                .small()
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    let ask = tern_agent::explain::Ask {
                                        concept: String::new(),
                                        topic: None,
                                        context: Some(text.clone()),
                                    };
                                    this.study_explain(ask, "This chat", window, cx);
                                })),
                        )
                    }),
            )
            .into_any_element()
    }

    pub(super) fn study_screen(
        &self,
        p: Palette,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let s = &self.model.study;
        let body = match s.view {
            StudyView::Home => self.study_home(p, cx),
            StudyView::Tutor => self.tutor_view(p, cx),
            StudyView::Quiz => self.quiz_view(p, cx),
            StudyView::Viva => self.viva_view(p, cx),
            StudyView::Cards => self.review_view(p, cx),
            StudyView::Deck => self.deck_view(p, cx),
        };
        div()
            .id("study-screen")
            .flex_1()
            .min_h_0()
            .bg(p.warm)
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .flex()
            .flex_col()
            .items_center()
            .child(
                div()
                    .w_full()
                    .max_w(px(super::COLUMN_MAX_W))
                    .px_4()
                    .py_6()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .pb(px(48.))
                    .children(self.study_status(p, cx))
                    .child(body),
            )
            .into_any_element()
    }

    /// The busy line and the last notice.
    fn study_status(&self, p: Palette, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let s = &self.model.study;
        let mut out = Vec::new();
        if let Some(b) = &s.busy {
            out.push(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_sm()
                    .text_color(p.muted)
                    .child(Spinner::new().small())
                    .child(b.clone())
                    .into_any_element(),
            );
        }
        if let Some(n) = &s.notice {
            out.push(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .rounded(px(PILL))
                    .border_1()
                    .border_color(p.border)
                    .text_xs()
                    .text_color(p.muted)
                    .child(div().flex_1().min_w_0().child(n.clone()))
                    .child(
                        Button::new("study-notice-x")
                            .icon(AssetIcon::X)
                            .ghost()
                            .xsmall()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.model.study.notice = None;
                                cx.notify();
                            })),
                    )
                    .into_any_element(),
            );
        }
        out
    }

    /// A mode's heading in the one Study window. The tab bar moves
    /// between modes, so there is no Back button.
    pub(super) fn back_row(
        &self,
        title: String,
        p: Palette,
        _cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(serif(title, p).text_xl())
            .into_any_element()
    }

    pub(super) fn card(&self, p: Palette) -> gpui_kit::Div {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .p_4()
            .rounded(px(PILL))
            .border_1()
            .border_color(p.border)
            .bg(p.canvas)
    }

    fn section_label(&self, text: &'static str, p: Palette) -> AnyElement {
        serif(text, p)
            .text_lg()
            .text_color(p.muted)
            .into_any_element()
    }

    fn study_home(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let s = &self.model.study;
        let lib = &s.library;
        let now = tern_agent::study::now();
        let busy = s.busy.is_some();
        let header = div()
            .flex()
            .items_start()
            .justify_between()
            .gap_3()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(glyph("study", 18., p.accent))
                            .child(serif("Study", p).text_2xl()),
                    )
                    .child(div().text_xs().text_color(p.faint).child(SharedString::from(
                        if lib.materials.is_empty() {
                            "Grounded in your own materials: add PDFs, notes, or a research report to begin.".to_owned()
                        } else {
                            format!(
                                "{} material{} · {} topic{} · overall {:.0}%{}",
                                lib.materials.len(),
                                if lib.materials.len() == 1 { "" } else { "s" },
                                lib.topics.len(),
                                if lib.topics.len() == 1 { "" } else { "s" },
                                lib.overall() * 100.,
                                match &s.engine {
                                    Some(w) => format!(" · passages go to {w}"),
                                    None => " · offline (no model)".into(),
                                }
                            )
                        },
                    ))),
            )
            .child(
                div()
                    .flex()
                    .gap_1()
                    .child(
                        Button::new("study-add")
                            .icon(AssetIcon::Plus)
                            .label("Add files")
                            .small()
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| this.pick_study_files(cx))),
                    )
                    .child(
                        Button::new("study-add-report")
                            .icon(AssetIcon::ScrollText)
                            .label("Add last report")
                            .ghost()
                            .small()
                            .disabled(busy)
                            .tooltip("Add the latest research report as a material, for viva practice on it")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.model.study.busy = Some("Adding your report…".into());
                                this.bridge.study_add_report("last".into());
                                cx.notify();
                            })),
                    ),
            );

        // Actions.
        let weakest = lib.weakest(now).cloned();
        let has = !lib.topics.is_empty();
        let has_materials = !lib.materials.is_empty();
        // Why a tile cannot open yet, said on the tile itself.
        let blocked = if !has_materials {
            Some("Needs materials: add a PDF, notes, or your last report")
        } else if !has {
            Some("Needs topics: use Rebuild topics under Topics")
        } else {
            None
        };
        let action = |id: &'static str,
                      icon_name: AssetIcon,
                      label: &'static str,
                      hint: String,
                      ready: bool| {
            let line = match (ready, blocked) {
                (false, Some(why)) => why.to_owned(),
                _ => hint,
            };
            div()
                .id(id)
                .flex_1()
                .flex_basis(px(170.))
                .min_w(px(150.))
                .flex()
                .flex_col()
                .gap_1()
                .p_3()
                .rounded(px(PILL))
                .border_1()
                .border_color(p.border)
                .bg(p.canvas)
                // Always clickable: a tile that cannot run yet opens its
                // mode with the add-material empty state.
                .cursor_pointer()
                .hover(|d| d.bg(p.hover))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .when(!ready, |d| d.opacity(0.6))
                        .child(icon(icon_name, p.text))
                        .child(serif(label, p).text_lg()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(if ready { p.muted } else { p.text })
                        .child(line),
                )
        };
        let tutor_hint = weakest
            .as_ref()
            .map(|t| format!("Start with {}", t.title))
            .unwrap_or_else(|| "A Socratic tutor on one topic".into());
        let actions = div()
            .flex()
            .flex_wrap()
            .gap_2()
            .child(
                action(
                    "study-act-tutor",
                    AssetIcon::GraduationCap,
                    "Tutor",
                    tutor_hint,
                    has,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(t) = this
                        .model
                        .study
                        .library
                        .weakest(tern_agent::study::now())
                        .cloned()
                    {
                        this.open_tutor(t, cx);
                    } else {
                        this.study_tab(StudyTab::Chat, cx);
                    }
                })),
            )
            .child(
                action(
                    "study-act-quiz",
                    AssetIcon::ListChecks,
                    "Quiz",
                    "On all your materials".into(),
                    has,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.study_tab(StudyTab::Quiz, cx);
                    if !this.model.study.library.topics.is_empty() {
                        this.model.study.open_quiz(None);
                        cx.notify();
                    }
                })),
            )
            .child({
                let due = s.due(None, now);
                let total = s.deck.cards.len();
                action(
                    "study-act-cards",
                    AssetIcon::WalletCards,
                    "Flashcards",
                    if total == 0 {
                        "Write cards from your materials".into()
                    } else if due == 0 {
                        format!("Nothing due · {total} cards")
                    } else {
                        format!("{due} due today · {total} cards")
                    },
                    total > 0 || has_materials,
                )
                .when(due > 0, |d| {
                    d.child(
                        div()
                            .absolute()
                            .top(px(10.))
                            .right(px(12.))
                            .size(px(7.))
                            .rounded_full()
                            .bg(p.accent),
                    )
                })
                .relative()
                .on_click(cx.listener(|this, _, window, cx| {
                    this.study_tab(StudyTab::Cards, cx);
                    let now = tern_agent::study::now();
                    let s = &this.model.study;
                    if s.deck.cards.is_empty() {
                        if !s.library.materials.is_empty() {
                            this.model.study.view = StudyView::Deck;
                            this.gen_cards(None, cx);
                        }
                    } else if s.due(None, now) > 0 {
                        this.start_review(None, window, cx);
                    } else {
                        this.model.study.view = StudyView::Deck;
                        cx.notify();
                    }
                }))
            })
            .child(
                action(
                    "study-act-viva",
                    AssetIcon::Mic,
                    "Viva",
                    "Oral exam practice with a rubric".into(),
                    has,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    if !this.model.study.library.topics.is_empty() {
                        this.model.study.open_viva();
                        cx.notify();
                    }
                })),
            );

        // With nothing to study from yet: the one-click ways in.
        let start = (!has_materials).then(|| {
            div()
                .flex()
                .flex_col()
                .gap_2()
                .p_3()
                .rounded(px(PILL))
                .border_1()
                .border_color(p.border)
                .bg(p.canvas)
                .child(serif("Start here", p).text_lg())
                .child(div().text_sm().text_color(p.muted).child(
                    "Tutor, Quiz, Flashcards, and Viva work from your own materials. Add one, or just ask Tern below.",
                ))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .child(
                            Button::new("study-start-add")
                                .icon(AssetIcon::Plus)
                                .label("Add a PDF or notes")
                                .primary()
                                .small()
                                .disabled(busy)
                                .on_click(cx.listener(|this, _, _, cx| this.pick_study_files(cx))),
                        )
                        .child(
                            Button::new("study-start-report")
                                .icon(AssetIcon::ScrollText)
                                .label("Use the last report")
                                .small()
                                .disabled(busy)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.model.study.busy = Some("Adding your report…".into());
                                    this.bridge.study_add_report("last".into());
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("study-start-ask")
                                .label("Ask without materials")
                                .ghost()
                                .small()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_study_chat(window, cx)
                                })),
                        ),
                )
        });

        // Materials.
        let mut materials = self.card(p).child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(self.section_label("Materials", p))
                .child(
                    div()
                        .text_xs()
                        .text_color(p.faint)
                        .child("PDFs keep their page numbers"),
                ),
        );
        if lib.materials.is_empty() {
            materials = materials.child(
                div()
                    .text_sm()
                    .text_color(p.faint)
                    .child("Nothing yet. Add lecture PDFs, your notes, or your FYP report."),
            );
        }
        for m in &lib.materials {
            let id = m.id.clone();
            let mut meta = m.kind.label().to_owned();
            if m.pages > 0 {
                meta.push_str(&format!(" · {} pages", m.pages));
            }
            meta.push_str(&format!(" · {} chunks", m.chunks));
            if m.cut {
                meta.push_str(" · cut to fit");
            }
            materials = materials.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .py_1()
                    .child(material_icon(m, p))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().text_sm().truncate().child(m.name.clone()))
                            .child(div().text_xs().text_color(p.faint).child(meta)),
                    )
                    .child(
                        Button::new(SharedString::from(format!("study-rm-{}", m.id)))
                            .icon(AssetIcon::X)
                            .ghost()
                            .xsmall()
                            .tooltip("Remove from Study")
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.model.study.busy = Some("Removing…".into());
                                this.bridge.study_remove(id.clone());
                                cx.notify();
                            })),
                    ),
            );
        }

        // Topics.
        let mut topics = self.card(p).child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(self.section_label("Topics", p))
                .child(
                    Button::new("study-rebuild")
                        .icon(AssetIcon::RefreshCw)
                        .label("Rebuild topics")
                        .ghost()
                        .xsmall()
                        .disabled(busy || lib.materials.is_empty())
                        .tooltip(
                            "Ask the model to regroup your materials into topics and objectives",
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.model.study.busy =
                                Some("Grouping your materials into topics…".into());
                            this.bridge.study_rebuild_topics();
                            cx.notify();
                        })),
                ),
        );
        if lib.topics.is_empty() && !lib.materials.is_empty() {
            topics = topics.child(div().text_sm().text_color(p.faint).child("No topics yet."));
        }
        for (i, t) in lib.topics.iter().enumerate() {
            let m = lib.mastery_of(&t.id);
            let tutor_t = t.clone();
            let quiz_t = t.clone();
            let due = m.is_due(now);
            topics = topics.child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .py_1p5()
                    .child(
                        ProgressCircle::new(SharedString::from(format!("ring-{i}")))
                            .value(m.rating * 100.)
                            .color(p.accent)
                            .accessibility_label(SharedString::from(format!(
                                "{}: {}",
                                t.title,
                                m.level().label()
                            )))
                            .size(px(22.)),
                    )
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
                                    .gap_1p5()
                                    .child(div().text_sm().truncate().child(t.title.clone()))
                                    .when(due, |d| {
                                        d.child(
                                            div()
                                                .px_1p5()
                                                .rounded_full()
                                                .border_1()
                                                .border_color(p.accent)
                                                .text_xs()
                                                .text_color(p.accent)
                                                .child("● due"),
                                        )
                                    }),
                            )
                            .child(div().text_xs().text_color(p.faint).truncate().child(
                                SharedString::from(format!(
                                    "{} · {}",
                                    m.explain(now),
                                    t.where_label()
                                )),
                            ))
                            .children(t.objectives.first().map(|o| {
                                div()
                                    .text_xs()
                                    .text_color(p.muted)
                                    .truncate()
                                    .child(o.clone())
                            })),
                    )
                    .child(
                        Button::new(SharedString::from(format!("topic-tutor-{i}")))
                            .label("Tutor")
                            .ghost()
                            .xsmall()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.open_tutor(tutor_t.clone(), cx);
                            })),
                    )
                    .child(
                        Button::new(SharedString::from(format!("topic-quiz-{i}")))
                            .label("Quiz")
                            .ghost()
                            .xsmall()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.model.study.open_quiz(Some(quiz_t.clone()));
                                cx.notify();
                            })),
                    )
                    .child({
                        let cards_n = s.deck.for_topic(&t.id).count();
                        let due_t = s.due(Some(&t.id), now);
                        let tid = t.id.clone();
                        Button::new(SharedString::from(format!("topic-cards-{i}")))
                            .label(SharedString::from(if due_t > 0 {
                                format!("Cards · {due_t}")
                            } else {
                                "Cards".to_owned()
                            }))
                            .ghost()
                            .xsmall()
                            .disabled(busy && cards_n == 0)
                            .tooltip(if cards_n == 0 {
                                "Write flashcards on this topic"
                            } else {
                                "Review this topic's cards (or write more from the deck)"
                            })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                let now = tern_agent::study::now();
                                if this.model.study.deck.for_topic(&tid).count() == 0 {
                                    this.gen_cards(Some(tid.clone()), cx);
                                } else if this.model.study.due(Some(&tid), now) > 0 {
                                    this.start_review(Some(tid.clone()), window, cx);
                                } else {
                                    this.model.study.deck_topic = Some(tid.clone());
                                    this.model.study.view = StudyView::Deck;
                                    cx.notify();
                                }
                            }))
                    })
                    .child({
                        let x_t = t.clone();
                        Button::new(SharedString::from(format!("topic-3d-{i}")))
                            .icon(AssetIcon::Cuboid)
                            .ghost()
                            .xsmall()
                            .tooltip(
                                "Explain in 3D: this topic as an isometric machine, step by step",
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                let ask = tern_agent::explain::Ask {
                                    concept: String::new(),
                                    topic: Some(x_t.id.clone()),
                                    context: None,
                                };
                                this.open_explain(ask, &x_t.title.clone(), window, cx);
                            }))
                    }),
            );
        }

        // History.
        let mut history = self.card(p).child(self.section_label("History", p));
        let mut rows: Vec<(i64, String)> = lib
            .quizzes
            .iter()
            .map(|q| {
                (
                    q.taken_at,
                    format!(
                        "Quiz · {} · {} / {} · {}",
                        q.title,
                        fmt_score(q.score),
                        q.total,
                        tern_agent::study::ago(now - q.taken_at)
                    ),
                )
            })
            .chain(lib.vivas.iter().map(|v| {
                (
                    v.taken_at,
                    format!(
                        "Viva · {} · {} / 100 · {}",
                        v.title,
                        v.overall,
                        tern_agent::study::ago(now - v.taken_at)
                    ),
                )
            }))
            .collect();
        rows.sort_by_key(|r| std::cmp::Reverse(r.0));
        if rows.is_empty() {
            history = history.child(
                div()
                    .text_xs()
                    .text_color(p.faint)
                    .child("Quizzes and vivas you take show here."),
            );
        }
        for (_, r) in rows.into_iter().take(6) {
            history = history.child(div().text_xs().text_color(p.muted).child(r));
        }

        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(header)
            .children(start)
            .child(actions)
            .child(materials)
            .child(topics)
            .child(history)
            .child(self.study_settings(p, cx))
            .into_any_element()
    }

    /// The Study group of settings: questions per quiz, difficulty, viva
    /// length, and whether to use the model.
    pub(super) fn study_settings(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let s = &self.model.study;
        let row = |label: &'static str| {
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .child(div().text_xs().text_color(p.muted).child(label))
        };
        let counts = [5usize, 10, 15].map(|n| {
            Button::new(SharedString::from(format!("set-count-{n}")))
                .label(SharedString::from(n.to_string()))
                .ghost()
                .xsmall()
                .selected(s.quiz_count == n)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.model.study.quiz_count = n;
                    this.save_study_prefs();
                    cx.notify();
                }))
        });
        let diffs = [Difficulty::Easy, Difficulty::Medium, Difficulty::Hard].map(|d| {
            Button::new(SharedString::from(format!("set-diff-{}", d.label())))
                .label(d.label())
                .ghost()
                .xsmall()
                .selected(s.quiz_difficulty == d)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.model.study.quiz_difficulty = d;
                    this.save_study_prefs();
                    cx.notify();
                }))
        });
        let vivas = [4usize, 6, 8].map(|n| {
            Button::new(SharedString::from(format!("set-viva-{n}")))
                .label(SharedString::from(n.to_string()))
                .ghost()
                .xsmall()
                .selected(s.viva_questions == n)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.model.study.viva_questions = n;
                    this.save_study_prefs();
                    cx.notify();
                }))
        });
        let offline = s.offline;
        self.card(p)
            .child(self.section_label("Study settings", p))
            .child(row("Questions per quiz").child(div().flex().gap_1().children(counts)))
            .child(row("Quiz difficulty").child(div().flex().gap_1().children(diffs)))
            .child(row("Viva questions").child(div().flex().gap_1().children(vivas)))
            .child(
                row("Use the model (off: offline study from your materials only)").child(
                    Button::new("set-offline")
                        .label(if offline { "Off" } else { "On" })
                        .ghost()
                        .xsmall()
                        .selected(!offline)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.model.study.offline = !this.model.study.offline;
                            this.save_study_prefs();
                            this.bridge.study_load();
                            cx.notify();
                        })),
                ),
            )
            .into_any_element()
    }

    fn save_study_prefs(&self) {
        let s = &self.model.study;
        let mut prefs = tern_agent::Preferences::load();
        prefs.study_quiz_count = Some(s.quiz_count);
        prefs.study_difficulty = Some(s.quiz_difficulty.label().to_lowercase());
        prefs.study_viva_questions = Some(s.viva_questions);
        prefs.study_offline = s.offline;
        self.bridge.save_prefs(prefs);
    }

    fn tutor_view(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let s = &self.model.study;
        let Some(topic) = &s.tutor_topic else {
            return div().into_any_element();
        };
        let chips = TutorMode::ALL.map(|m| {
            Button::new(SharedString::from(format!("tutor-mode-{}", m.label())))
                .label(m.label())
                .ghost()
                .small()
                .selected(s.tutor_mode == m)
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(h) = this.model.study.set_tutor_mode(m) {
                        this.send_tutor(h);
                    }
                    cx.notify();
                }))
        });
        let mastery = s.library.mastery_of(&topic.id);
        let mut col = div().flex().flex_col().gap_3().child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .child(self.back_row(topic.title.clone(), p, cx))
                .child(
                    ProgressCircle::new("tutor-ring")
                        .value(mastery.rating * 100.)
                        .color(p.accent)
                        .size(px(20.)),
                ),
        );
        col = col.child(div().flex().gap_1().children(chips)).child(
            div()
                .text_xs()
                .text_color(p.faint)
                .child(SharedString::from(format!(
                    "{} · grounded in {} · anything beyond your materials is flagged",
                    mastery.explain(tern_agent::study::now()),
                    topic.where_label()
                ))),
        );
        // Hide the opening line; it is the tutor's cue, not the user's words.
        for (i, line) in s.tutor.iter().enumerate().skip(1) {
            col = col.child(self.tutor_line(i, line, p));
        }
        if s.tutor_waiting {
            if s.tutor_stream.is_empty() {
                col = col.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_sm()
                        .text_color(p.muted)
                        .child(Spinner::new().small())
                        .child("Reading your materials…"),
                );
            } else {
                col = col.child(
                    self.card(p)
                        .child(TextView::markdown("tutor-stream", s.tutor_stream.clone())),
                );
            }
        }
        col = col.child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .px_2()
                .py_1()
                .rounded(px(PILL))
                .border_1()
                .border_color(p.border)
                .bg(p.canvas)
                .child(
                    div()
                        .flex_1()
                        .child(Input::new(&self.study_inputs.answer).appearance(false)),
                )
                .child(
                    Button::new("tutor-send")
                        .icon(AssetIcon::ArrowUp)
                        .small()
                        .disabled(s.tutor_waiting)
                        .on_click(cx.listener(|this, _, window, cx| this.tutor_send(window, cx))),
                ),
        );
        col.into_any_element()
    }

    fn tutor_line(&self, i: usize, line: &crate::study::TutorLine, p: Palette) -> AnyElement {
        if line.turn.is_user() {
            return div()
                .flex()
                .justify_end()
                .child(
                    div()
                        .max_w(px(520.))
                        .px_3()
                        .py_2()
                        .rounded(px(PILL))
                        .bg(p.hover)
                        .text_sm()
                        .child(line.turn.text.clone()),
                )
                .into_any_element();
        }
        let mut card = self
            .card(p)
            .child(TextView::markdown(("tutor", i), line.turn.text.clone()));
        let mut foot: Vec<String> = Vec::new();
        if !line.citations.is_empty() {
            foot.push(format!("● {}", line.citations.join("  ● ")));
        }
        if line.beyond {
            foot.push("Includes something beyond your materials (marked)".into());
        }
        if line.offline {
            foot.push("Offline: quoted from your materials".into());
        }
        for f in foot {
            card = card.child(div().text_xs().text_color(p.faint).child(f));
        }
        card.into_any_element()
    }

    fn quiz_view(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let s = &self.model.study;
        let title = s
            .quiz_topic
            .as_ref()
            .map(|t| format!("Quiz · {}", t.title))
            .unwrap_or_else(|| "Quiz · All materials".into());
        let mut col = div()
            .flex()
            .flex_col()
            .gap_3()
            .child(self.back_row(title, p, cx));
        let Some(run) = &s.quiz else {
            // Setup.
            let counts = [5usize, 10, 15].map(|n| {
                Button::new(SharedString::from(format!("quiz-count-{n}")))
                    .label(SharedString::from(format!("{n} questions")))
                    .ghost()
                    .small()
                    .selected(s.quiz_count == n)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.model.study.quiz_count = n;
                        cx.notify();
                    }))
            });
            let diffs = [Difficulty::Easy, Difficulty::Medium, Difficulty::Hard].map(|d| {
                Button::new(SharedString::from(format!("quiz-diff-{}", d.label())))
                    .label(d.label())
                    .ghost()
                    .small()
                    .selected(s.quiz_difficulty == d)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.model.study.quiz_difficulty = d;
                        cx.notify();
                    }))
            });
            return col
                .child(
                    self.card(p)
                        .child(div().text_sm().text_color(p.muted).child(
                            "Multiple choice, true or false, fill in the blank, and short answers, each with the page it comes from.",
                        ))
                        .child(div().flex().gap_1().children(counts))
                        .child(div().flex().gap_1().children(diffs))
                        .child(
                            div().pt_2().child(
                                Button::new("quiz-start")
                                    .icon(AssetIcon::Play)
                                    .label("Start quiz")
                                    .small()
                                    .disabled(s.busy.is_some())
                                    .on_click(cx.listener(|this, _, _, cx| this.start_quiz(cx))),
                            ),
                        ),
                )
                .into_any_element();
        };
        if let Some(done) = &run.done {
            return col
                .child(self.quiz_summary(run, done, p, cx))
                .into_any_element();
        }
        let Some(q) = run.question() else {
            return col.into_any_element();
        };
        let graded = run.graded();
        let mut card = self
            .card(p)
            .child(
                div()
                    .flex()
                    .justify_between()
                    .text_xs()
                    .text_color(p.faint)
                    .child(SharedString::from(format!(
                        "{} of {} · {}",
                        run.index + 1,
                        run.total(),
                        q.kind.label()
                    )))
                    .child(SharedString::from(run.running_score())),
            )
            .child(div().text_base().child(q.prompt.clone()));
        match q.kind {
            QuestionKind::MultipleChoice | QuestionKind::TrueFalse => {
                let choices: Vec<String> = if q.kind == QuestionKind::TrueFalse {
                    vec!["True".into(), "False".into()]
                } else {
                    q.choices.clone()
                };
                for (i, c) in choices.into_iter().enumerate() {
                    let picked = run.picked.as_deref() == Some(c.as_str());
                    let is_answer = tern_agent::study::grade_exact(q, &c)
                        .is_some_and(|g| g.mark == Mark::Correct);
                    let reveal = graded.is_some();
                    let value = c.clone();
                    card = card.child(
                        div()
                            .id(SharedString::from(format!("choice-{}-{i}", q.n)))
                            .flex()
                            .items_center()
                            .gap_2()
                            .px_3()
                            .py_2()
                            .rounded(px(PILL))
                            .border_1()
                            .border_color(if picked || (reveal && is_answer) {
                                p.accent
                            } else {
                                p.border
                            })
                            .when(!reveal, |d| d.cursor_pointer().hover(|d| d.bg(p.hover)))
                            .child(div().text_xs().text_color(p.faint).child(
                                if q.kind == QuestionKind::TrueFalse {
                                    "●".to_owned()
                                } else {
                                    tern_agent::study::letter(i).to_string()
                                },
                            ))
                            .child(div().flex_1().text_sm().child(c))
                            .when(reveal && is_answer, |d| {
                                d.child(glyph("verdict-supported", 14., p.accent))
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(run) = &mut this.model.study.quiz
                                    && run.graded().is_none()
                                    && !run.grading
                                {
                                    run.picked = Some(value.clone());
                                }
                                cx.notify();
                            })),
                    );
                }
            }
            _ => {
                if graded.is_none() {
                    card = card.child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded(px(PILL))
                            .border_1()
                            .border_color(p.border)
                            .child(Input::new(&self.study_inputs.answer).appearance(false)),
                    );
                }
            }
        }
        match graded {
            None => {
                card = card.child(
                    div().pt_1().flex().items_center().gap_2().child(
                        Button::new("quiz-submit")
                            .label(if run.grading {
                                "Checking…"
                            } else {
                                "Check answer"
                            })
                            .small()
                            .disabled(run.grading)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.quiz_submit(window, cx)),
                            ),
                    ),
                );
            }
            Some(a) => {
                card = card
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .gap_2()
                            .p_3()
                            .rounded(px(PILL))
                            .bg(p.hover)
                            .child(glyph(mark_glyph(a.grade.mark), 16., p.text))
                            .child(
                                div()
                                    .flex_1()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(serif(a.grade.mark.label(), p).text_base())
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(p.muted)
                                            .child(a.grade.feedback.clone()),
                                    )
                                    .child(div().text_xs().text_color(p.faint).child(
                                        SharedString::from(format!(
                                            "Graded by {}",
                                            a.grade.method.label()
                                        )),
                                    )),
                            ),
                    )
                    .child(
                        div().pt_1().child(
                            Button::new("quiz-next")
                                .label(if run.is_last() {
                                    "See score"
                                } else {
                                    "Next question"
                                })
                                .small()
                                .on_click(cx.listener(|this, _, _, cx| this.quiz_next(cx))),
                        ),
                    );
            }
        }
        col = col.child(card);
        col.into_any_element()
    }

    fn quiz_summary(
        &self,
        run: &crate::study::QuizRun,
        done: &tern_agent::study::QuizSummary,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let pct = if done.total == 0 {
            0.
        } else {
            done.score / done.total as f32 * 100.
        };
        let mut card = self.card(p).child(
            div()
                .flex()
                .items_center()
                .gap_3()
                .child(
                    ProgressCircle::new("quiz-score-ring")
                        .value(pct)
                        .color(p.accent)
                        .size(px(40.)),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .child(
                            serif(format!("{} of {}", fmt_score(done.score), done.total), p)
                                .text_2xl(),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(p.faint)
                                .child(SharedString::from(format!(
                                    "{pct:.0}% · {} · saved to your history; mastery updated",
                                    run.quiz.difficulty.label()
                                ))),
                        ),
                ),
        );
        for q in &run.quiz.questions {
            let a = run.attempt.answers.iter().find(|a| a.n == q.n);
            let mark = a.map(|a| a.grade.mark).unwrap_or(Mark::Unanswered);
            card =
                card.child(
                    div()
                        .flex()
                        .items_start()
                        .gap_2()
                        .child(
                            div()
                                .pt(px(2.))
                                .child(glyph(mark_glyph(mark), 14., p.muted)),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .child(div().text_sm().child(q.prompt.clone()))
                                .child(div().text_xs().text_color(p.faint).child(
                                    SharedString::from(format!(
                                "Answer: {}{}",
                                q.answer,
                                q.anchor
                                    .as_ref()
                                    .map(|a| format!(" · {}", a.label()))
                                    .unwrap_or_default()
                            )),
                                )),
                        ),
                );
        }
        card.child(
            div()
                .pt_2()
                .flex()
                .gap_2()
                .child(
                    Button::new("quiz-again")
                        .icon(AssetIcon::RefreshCw)
                        .label("New quiz")
                        .small()
                        .on_click(cx.listener(|this, _, _, cx| this.start_quiz(cx))),
                )
                .child(
                    Button::new("quiz-home")
                        .label("Back to Study")
                        .ghost()
                        .small()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.model.study.go_home();
                            cx.notify();
                        })),
                ),
        )
        .into_any_element()
    }

    fn viva_view(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let s = &self.model.study;
        let now = Instant::now();
        let mut col = div().flex().flex_col().gap_3();
        let Some(run) = &s.viva else {
            col = col.child(self.back_row("Viva practice".into(), p, cx));
            return col
                .child(
                    self.card(p)
                        .child(div().text_sm().text_color(p.muted).child(SharedString::from(format!(
                            "An examiner asks {} questions, from recall up to defending your choices, following up when an answer is thin. Then you get a scored rubric: clarity, correctness, depth, and use of sources.",
                            s.viva_questions
                        ))))
                        .child(Input::new(&self.study_inputs.title).small())
                        .child(
                            div().pt_2().child(
                                Button::new("viva-start")
                                    .icon(AssetIcon::Mic)
                                    .label("Start viva")
                                    .small()
                                    .disabled(s.busy.is_some())
                                    .on_click(cx.listener(|this, _, _, cx| this.viva_start(cx))),
                            ),
                        ),
                )
                .into_any_element();
        };
        col = col.child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(self.back_row(format!("Viva · {}", run.viva.title), p, cx))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .px_3()
                        .py_1()
                        .rounded_full()
                        .border_1()
                        .border_color(if run.report.is_none() {
                            p.accent
                        } else {
                            p.border
                        })
                        .font_family(assets::MONO_FONT)
                        .text_sm()
                        .child(icon(AssetIcon::Timer, p.muted))
                        .child(run.clock(now)),
                ),
        );
        if let Some((report, path)) = &run.report {
            return col
                .child(self.viva_report(run, report, path.as_deref(), p, cx))
                .into_any_element();
        }
        // Past questions, folded small.
        for (i, t) in run.viva.turns.iter().enumerate() {
            if t.answer.is_empty() {
                continue;
            }
            col = col.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .text_xs()
                    .text_color(p.faint)
                    .child(SharedString::from(format!(
                        "Q{} · level {}{} · {}s",
                        i + 1,
                        t.level,
                        if t.follow_up { " · follow-up" } else { "" },
                        t.seconds
                    )))
                    .child(div().text_color(p.muted).child(t.question.clone()))
                    .child(div().child(tern_study_clip(&t.answer, 220))),
            );
        }
        if let Some(t) = run.viva.waiting() {
            let level = t.level;
            col = col.child(
                self.card(p)
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .text_xs()
                            .text_color(p.faint)
                            .child(SharedString::from(format!(
                                "{}{}",
                                run.progress(),
                                if t.follow_up { " · follow-up" } else { "" }
                            )))
                            .child(SharedString::from(format!(
                                "Level {level} of 5 · {}",
                                crate::study::clock(now.saturating_duration_since(run.asked))
                            ))),
                    )
                    .child(serif(t.question.clone(), p).text_xl())
                    .child(
                        div()
                            .p_2()
                            .rounded(px(PILL))
                            .border_1()
                            .border_color(p.border)
                            .child(Textarea::new(&self.study_inputs.viva).appearance(false)),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                Button::new("viva-send")
                                    .label("Answer")
                                    .small()
                                    .disabled(run.thinking)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.viva_send(false, window, cx)
                                    })),
                            )
                            .child(
                                Button::new("viva-end")
                                    .label("End and score")
                                    .ghost()
                                    .small()
                                    .disabled(run.thinking)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.viva_send(true, window, cx)
                                    })),
                            ),
                    ),
            );
        }
        col.into_any_element()
    }

    fn viva_report(
        &self,
        run: &crate::study::VivaRun,
        report: &tern_agent::study::VivaReport,
        path: Option<&std::path::Path>,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut card = self.card(p).child(
            div()
                .flex()
                .items_center()
                .gap_3()
                .child(
                    ProgressCircle::new("viva-score-ring")
                        .value(report.overall() as f32)
                        .color(p.accent)
                        .size(px(44.)),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .child(serif(format!("{} / 100", report.overall()), p).text_2xl())
                        .child(
                            div()
                                .text_xs()
                                .text_color(p.faint)
                                .child(SharedString::from(format!(
                                    "{} questions · {} · scored by {}",
                                    run.viva.turns.len(),
                                    run.clock(Instant::now()),
                                    report.method
                                ))),
                        ),
                ),
        );
        for (name, v) in report.rubric.rows() {
            card = card.child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(div().w(px(120.)).text_sm().text_color(p.muted).child(name))
                    .child(div().flex().gap_1().children((1..=5u8).map(|i| {
                        div()
                            .size(px(10.))
                            .rounded_full()
                            .border_1()
                            .border_color(p.muted)
                            .when(i <= v, |d| d.bg(p.text))
                    })))
                    .child(
                        div()
                            .text_xs()
                            .text_color(p.faint)
                            .child(SharedString::from(format!("{v} / 5"))),
                    ),
            );
        }
        if !report.summary.is_empty() {
            card = card.child(div().pt_1().text_sm().child(report.summary.clone()));
        }
        for (head, items) in [
            ("Strengths", &report.strengths),
            ("Weaknesses", &report.weaknesses),
            ("Revise next", &report.suggested_topics),
        ] {
            if items.is_empty() {
                continue;
            }
            card = card.child(serif(head, p).pt_1().text_base().text_color(p.muted));
            for i in items {
                card = card.child(div().text_sm().child(SharedString::from(format!("● {i}"))));
            }
        }
        if let Some(path) = path {
            card = card.child(div().pt_1().text_xs().text_color(p.faint).child(
                SharedString::from(format!("Report saved: {}", path.display())),
            ));
        }
        card.child(
            div()
                .pt_2()
                .flex()
                .gap_2()
                .child(
                    Button::new("viva-again")
                        .icon(AssetIcon::RefreshCw)
                        .label("Another viva")
                        .small()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.model.study.viva = None;
                            this.viva_start(cx);
                        })),
                )
                .child(
                    Button::new("viva-home")
                        .label("Back to Study")
                        .ghost()
                        .small()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.model.study.go_home();
                            cx.notify();
                        })),
                ),
        )
        .into_any_element()
    }
}

fn tern_study_clip(s: &str, n: usize) -> String {
    tern_agent::study::clip(s, n)
}
