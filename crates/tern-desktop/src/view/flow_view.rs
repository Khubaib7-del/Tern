//! Research flow v2 in the window (s06–s11, s01, s15): queued steer rows
//! over the composer, the "Tern needs you" card with numbered answers,
//! the checkpoint strip on the transcript's edge, the budget card, the
//! citation hover cards under an answer, claim review and report export
//! in the Report tab, the launch splash, and the title bar's crumb and
//! search button. The GPUI-free parts are in `crate::flow`.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::hover_card::HoverCard;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{Disableable as _, IconName};
use gpui_kit::prelude::*;
use gpui_kit::{
<<<<<<< Updated upstream
    AnimationExt as _, AnyElement, Context, Entity, FontWeight, KeyDownEvent, SharedString, Window,
    div, px, svg,
=======
    AnimationExt as _, AnyElement, Context, Entity, FontWeight, KeyDownEvent, SharedString, Window, div, px, svg,
>>>>>>> Stashed changes
};
use tern_agent::run::compact;
use tern_agent::{ApprovalAnswer, PageRef, Run};

use super::Workspace;
use super::press::Capsule as _;
use crate::flow::{ClaimReview, clock, safe_points};
use crate::model::{Phase, Screen};
use crate::theme::Palette;

/// Ticks the checkpoint strip shows at most (the latest ones).
const MAX_TICKS: usize = 14;
/// The launch splash: the mark settles, holds, then flies to the title bar.
const SPLASH_IN: f32 = 0.55;
const SPLASH_HOLD: f32 = 0.2;
const SPLASH_FLY: f32 = 0.6;
const SPLASH_MARK: f32 = 96.;

/// The window's research-flow state.
pub(super) struct FlowUi {
    /// When the window opened, for the splash.
    launched: Instant,
    /// Each run's claim review, by run key (its report's path, else its
    /// start time); loaded from beside the report the first time it shows.
    reviews: RefCell<HashMap<String, ClaimReview>>,
    /// The note being written on a claim: run key, claim, field.
    note: Option<(String, usize, Entity<InputState>)>,
    /// Which quote each citation's hover card shows, by (entry, source).
    cite_at: Rc<RefCell<HashMap<(usize, usize), usize>>>,
}

impl Default for FlowUi {
    fn default() -> Self {
        Self {
            launched: Instant::now(),
            reviews: RefCell::default(),
            note: None,
            cite_at: Rc::default(),
        }
    }
}

/// The key a run's review is kept under.
fn run_key(run: &Run) -> String {
    match &run.report {
        Some(f) => f.markdown.to_string_lossy().to_string(),
        None => run.started_rfc3339().unwrap_or_default(),
    }
}

/// A key cap: "1", "Tab".
fn kbd(key: &str, p: Palette) -> AnyElement {
    div()
        .flex_none()
        .min_w(px(18.))
        .h(px(18.))
        .px_1()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.))
        .border_1()
        .border_color(p.border)
        .text_xs()
        .text_color(p.muted)
        .child(SharedString::from(key.to_owned()))
        .into_any_element()
}

/// One quote a citation's hover card can show.
#[derive(Clone)]
struct Quote {
    claim: usize,
    verdict: &'static str,
    quote: String,
    page: Option<u32>,
    url: String,
}

impl Workspace {
    // ----- Steer (v2 s06) -----

    /// Steering notes over the composer while the run goes: each one
    /// queued for the next safe point, then applied (or not).
    pub(super) fn steer_rows(&self, p: Palette) -> Option<AnyElement> {
        if !self.model.is_busy() {
            return None;
        }
        let run = self.model.panel_run();
        if run.steers.is_empty() {
            return None;
        }
        let run_at = run.started_rfc3339().unwrap_or_default();
        let skip = run.steers.len().saturating_sub(3);
        let rows: Vec<AnyElement> = run
            .steers
            .iter()
            .enumerate()
            .skip(skip)
            .map(|(k, s)| {
                let (state, done) = match s.applied {
                    None => ("queued · next safe point".to_owned(), false),
                    Some(true) => ("applied".to_owned(), true),
                    Some(false) => ("not applied".to_owned(), true),
                };
                let row = div()
                    .id(SharedString::from(format!("steer-row-{k}")))
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(px(26.))
                    .px_1p5()
                    .text_sm()
                    .text_color(if done { p.faint } else { p.text })
                    .child(div().flex_none().text_color(p.faint).child("↳"))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(SharedString::from(s.text.clone())),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_xs()
                            .text_color(p.faint)
                            .child(SharedString::from(state)),
                    )
                    .into_any_element();
                self.grow_in("steer-rows", format!("{run_at}/{k}"), row)
            })
            .collect();
        Some(div().flex().flex_col().children(rows).into_any_element())
    }

    // ----- Needs you (v2 s07) -----

    /// "◆ Tern needs you": what it wants, then numbered answers, in a
    /// yellow outline that pulses (#75). A file write shows its diff; a
    /// command shows the sandbox it runs in. 1 to 4 answer from the
    /// keyboard while the composer is empty.
    pub(super) fn needs_you_card(&self, p: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let Phase::Approval { request, .. } = &self.model.phase else {
            return None;
        };
        let target = request.target.clone().unwrap_or_default();
        let write = request.tool == "file_write";
        let rule = tern_agent::PermissionRule::suggest(
            &request.tool,
            request.effect,
            request.target.as_deref(),
            &request.args,
        );
        let options: [String; 4] = if write {
            [
                "Approve".into(),
                "Approve for this chat".into(),
                format!("Always allow writes in {}", rule.pattern),
                "Reject, and tell Tern what to do".into(),
            ]
        } else {
            [
                "Allow once".into(),
                "Allow for this chat".into(),
                format!("Always allow this ({})", rule.line()),
                "No, and tell Tern what to do".into(),
            ]
        };
        let rows = options.into_iter().enumerate().map(|(k, label)| {
            div()
                .id(SharedString::from(format!("needs-you-{}", k + 1)))
                .flex()
                .items_center()
                .gap_2p5()
                .h(px(30.))
                .px_2()
                .ml(px(10.))
                .rounded_md()
                .cursor_pointer()
                .text_sm()
                .hover(|d| d.bg(p.hover))
                .child(kbd(&(k + 1).to_string(), p))
                .child(div().min_w_0().truncate().child(SharedString::from(label)))
                .on_click(
                    cx.listener(move |this, _, window, cx| this.pick_needs_you(k, window, cx)),
                )
        });
        let yellow = super::work_view::approval_yellow(p);
        let detail: Option<AnyElement> = if write {
            let path = request.args["path"].as_str().unwrap_or_default();
            let text = request.args["text"].as_str().unwrap_or_default();
            Some(
                div()
                    .id("needs-you-diff")
                    .ml(px(18.))
                    .max_h(px(320.))
                    .overflow_y_scroll()
                    .p_2()
                    .rounded_md()
                    .border_1()
                    .border_color(p.border)
                    .bg(p.canvas)
                    .child(self.diff_view(path, text, p))
                    .into_any_element(),
            )
        } else if request.tool == "run_command" {
            let folder = self
                .model
                .files
                .root
                .clone()
                .map(|r| tern_agent::workbench::policy_for(&r).summary())
                .unwrap_or_default();
            Some(
                div()
                    .ml(px(18.))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .p_2()
                            .rounded_md()
                            .bg(p.canvas)
                            .border_1()
                            .border_color(p.border)
                            .font_family(crate::assets::MONO_FONT)
                            .text_xs()
                            .child(SharedString::from(format!("$ {target}"))),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(p.faint)
                            .child(SharedString::from(format!(
                                "Sandbox: {folder} · runs in the chat's folder"
                            ))),
                    )
                    .into_any_element(),
            )
        } else {
            None
        };
        let card = div()
            .id("needs-you")
            .flex()
            .flex_col()
            .gap_1()
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(yellow)
            .bg(p.raised)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_sm()
                    .child(div().text_color(yellow).child("◆"))
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .child("Tern needs you"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(p.faint)
                            .child(SharedString::from(format!(
                                "wants to use {} · approval mode {}",
                                request.tool,
                                self.model.providers.prefs.approval().label()
                            ))),
                    ),
            )
            .child(
                div()
                    .ml(px(18.))
                    .text_sm()
                    .text_color(p.muted)
                    .child(SharedString::from(request.summary.clone())),
            )
            .when(
                !target.is_empty() && !write && request.tool != "run_command",
                |d| {
                    d.child(
                        div()
                            .ml(px(18.))
                            .text_xs()
                            .text_color(p.faint)
                            .truncate()
                            .child(SharedString::from(target)),
                    )
                },
            )
            .children(detail)
            .child(div().h(px(4.)))
            .children(rows);
        Some(if self.reduced_motion {
            card.into_any_element()
        } else {
            card.with_animation(
                "needs-you-pulse",
                gpui_kit::Animation::new(Duration::from_millis(1400)).repeat(),
                move |d, t| {
                    let a = 0.45 + 0.55 * (0.5 + 0.5 * (t * std::f32::consts::TAU).cos());
                    d.border_color(yellow.opacity(a))
                },
            )
            .into_any_element()
        })
    }

    fn pick_needs_you(&mut self, k: usize, window: &mut Window, cx: &mut Context<Self>) {
        let answer = match k {
            0 => ApprovalAnswer::AllowOnce,
            1 => ApprovalAnswer::AllowForSession,
            2 => ApprovalAnswer::AllowAlways,
            _ => ApprovalAnswer::Deny,
        };
        if k == 2
            && let Phase::Approval { request, .. } = &self.model.phase
        {
            // "Always allow this": the rule is kept in Settings ›
            // Permissions, where it can be removed.
            let rule = tern_agent::PermissionRule::suggest(
                &request.tool,
                request.effect,
                request.target.as_deref(),
                &request.args,
            );
            tern_agent::diag::note(&format!("approval: always allow -> rule {}", rule.line()));
            let line = rule.line();
            self.save_ui_prefs(move |p| {
                if !p.permission_rules.contains(&rule) {
                    p.permission_rules.push(rule.clone());
                }
            });
            self.model.toast_info(format!(
                "Added the rule {line}. Remove it in Settings › Permissions."
            ));
        }
        self.answer(answer, cx);
        if k >= 3 {
            self.composer.update(cx, |c, cx| c.focus(window, cx));
            self.model.toast_info(
                "Not allowed. Tell Tern what to do instead: the composer steers the run.",
            );
        }
        cx.notify();
    }

    /// 1 to 4 answer the needs-you card while the composer is empty.
    pub(super) fn needs_you_keys(
        &mut self,
        e: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !matches!(self.model.phase, Phase::Approval { .. }) {
            return false;
        }
        let m = &e.keystroke.modifiers;
        if m.control || m.platform || m.alt || m.shift {
            return false;
        }
        if !self.composer.read(cx).value().trim().is_empty() {
            return false;
        }
        let k = match e.keystroke.key.as_str() {
            "1" => 0,
            "2" => 1,
            "3" => 2,
            "4" => 3,
            _ => return false,
        };
        self.pick_needs_you(k, window, cx);
        cx.stop_propagation();
        true
    }

    // ----- Checkpoints (v2 s11) -----

    /// Ticks on the transcript's left edge, one per safe point the run
    /// passed. Hover one for where it was; the latest one restores (resumes
    /// from its checkpoint) when the run stopped short.
    pub(super) fn checkpoint_strip(
        &self,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.model.entries.is_empty() {
            return None;
        }
        let points = safe_points(self.model.panel_run());
        if points.is_empty() {
            return None;
        }
        let n = points.len();
        let resumable = !self.model.is_busy() && self.model.resume_offer.is_some();
        let ws = cx.entity().downgrade();
        let ticks: Vec<AnyElement> = points
            .into_iter()
            .enumerate()
            .skip(n.saturating_sub(MAX_TICKS))
            .map(|(k, pt)| {
                let latest = k + 1 == n;
                let trigger = div()
                    .id(SharedString::from(format!("ckpt-{k}")))
                    .relative()
                    .w(px(16.))
                    .h(px(9.))
                    .cursor_pointer()
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .top(px(4.))
                            .h(px(1.5))
                            .w(px(if latest { 12. } else { 8. }))
                            .rounded(px(1.))
                            .bg(if latest { p.accent } else { p.faint }),
                    );
                let title = format!("Checkpoint {} · {}", k + 1, pt.label);
                let detail = format!(
                    "{} pages read · {} tokens · at {}",
                    pt.pages,
                    compact(pt.tokens),
                    clock(pt.at)
                );
                let ws = ws.clone();
                HoverCard::new(SharedString::from(format!("ckpt-card-{k}")))
                    .open_delay(Duration::from_millis(120))
                    .trigger(trigger)
                    .content(move |_, _, _| {
                        let ws = ws.clone();
                        let foot: AnyElement = if latest && resumable {
                            div()
                                .id("ckpt-restore")
                                .text_xs()
                                .text_color(p.text)
                                .cursor_pointer()
                                .hover(|d| d.underline())
                                .child("↺ Restore from here")
                                .on_click(move |_, _, cx| {
                                    let _ = ws.update(cx, |this, cx| this.resume_run(cx));
                                })
                                .into_any_element()
                        } else if latest {
                            div()
                                .text_xs()
                                .text_color(p.muted)
                                .child("The run's latest safe point")
                                .into_any_element()
                        } else {
                            div()
                                .text_xs()
                                .text_color(p.muted)
                                .child("A resume picks up from the latest checkpoint")
                                .into_any_element()
                        };
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .max_w(px(280.))
                            .text_sm()
                            .child(
                                div()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(SharedString::from(title.clone())),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(p.faint)
                                    .child(SharedString::from(detail.clone())),
                            )
                            .child(foot)
                    })
                    .into_any_element()
            })
            .collect();
        Some(
            div()
                .id("checkpoint-strip")
                .absolute()
                .left(px(6.))
                .top(px(24.))
                .flex()
                .flex_col()
                .gap(px(10.))
                .children(ticks)
                .into_any_element(),
        )
    }

    // ----- Budget (v2 s11) -----

    /// "Paused at a safe point": the run reached its token limit. Raise the
    /// limit and it resumes from its checkpoint, or stop there.
    pub(super) fn budget_card(&self, p: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let offer = self.model.resume_offer.as_ref()?;
        let stop = offer.budget?;
        let limit = stop
            .limit
            .or_else(|| self.session.as_ref().map(|s| s.budget().max_tokens))
            .unwrap_or(0);
        let line = if limit > 0 {
            format!(
                "This run has used {} of {} tokens.",
                compact(stop.used),
                compact(limit)
            )
        } else {
            format!("This run has used {} tokens.", compact(stop.used))
        };
        let can = self.model.can_send() && self.session.is_some();
        let raise = |id: &'static str, f: f64, primary: bool| {
            let label = if limit > 0 {
                format!(
                    "Raise to {} and resume",
                    compact((limit as f64 * f).round() as u64)
                )
            } else {
                format!("Raise ×{f} and resume")
            };
            let b = Button::new(id)
                .capsule()
                .label(SharedString::from(label))
                .small()
                .disabled(!can)
                .on_click(cx.listener(move |this, _, _, cx| this.raise_and_resume(f, cx)));
            if primary { b.primary() } else { b.ghost() }
        };
        let pause = div()
            .flex_none()
            .flex()
            .gap(px(3.))
            .child(div().w(px(3.)).h(px(11.)).rounded(px(1.)).bg(p.text))
            .child(div().w(px(3.)).h(px(11.)).rounded(px(1.)).bg(p.text));
        Some(
            div()
                .id("budget-card")
                .flex()
                .flex_col()
                .gap_2()
                .p_3()
                .rounded_lg()
                .border_1()
                .border_color(p.border)
                .bg(p.raised)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_sm()
                        .child(pause)
                        .child(
                            div()
                                .font_weight(FontWeight::MEDIUM)
                                .child("Paused at a safe point"),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(p.faint)
                                .child(SharedString::from(offer.progress.clone())),
                        ),
                )
                .child(
                    div()
                        .ml(px(17.))
                        .text_sm()
                        .text_color(p.muted)
                        .child(SharedString::from(line)),
                )
                .child(
                    div()
                        .ml(px(17.))
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .child(raise("budget-raise-1", 1.5, true))
                        .child(raise("budget-raise-2", 2.0, false))
                        .child(
                            Button::new("budget-stop")
                                .capsule()
                                .label("Stop")
                                .ghost()
                                .small()
                                .tooltip("Forget the paused run")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(s) = &this.session {
                                        let _ = s.discard_checkpoint();
                                    }
                                    this.model.resume_offer = None;
                                    cx.notify();
                                })),
                        ),
                )
                .into_any_element(),
        )
    }

    fn raise_and_resume(&mut self, factor: f64, cx: &mut Context<Self>) {
        let Some(s) = self.session.as_mut() else {
            return;
        };
        match s.raise_budget(factor) {
            Ok(limit) => {
                self.model.toast_info(format!(
                    "Limit raised to {} tokens · resumed from the checkpoint",
                    compact(limit)
                ));
                self.resume_run(cx);
            }
            Err(e) => self
                .model
                .toast_error(format!("Could not raise the limit: {e}"), None),
        }
        cx.notify();
    }

    // ----- Citations (v2 s08) -----

    /// The sources an answer cites, as chips under it. Hover one for the
    /// quoted passage, its page and claim, with ‹ › through every passage
    /// quoted from that source, and Open to read it in Tern's browser.
    pub(super) fn citations(
        &self,
        i: usize,
        run: &Run,
        pages: &[PageRef],
        p: Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let mut by: BTreeMap<usize, (String, String, Vec<Quote>)> = BTreeMap::new();
        for c in &run.checks {
            let Some(ev) = &c.evidence else {
                continue;
            };
            let e = by
                .entry(ev.source)
                .or_insert_with(|| (ev.title.clone(), ev.site.clone(), Vec::new()));
            e.2.push(Quote {
                claim: c.claim,
                verdict: c.verdict.label(),
                quote: ev.quote.clone(),
                page: ev.page,
                url: if ev.anchor.is_empty() {
                    ev.url.clone()
                } else {
                    ev.anchor.clone()
                },
            });
        }
        if by.is_empty() {
            return None;
        }
        let ws = cx.entity().downgrade();
        let chips: Vec<AnyElement> = by
            .into_iter()
            .map(|(n, (title, site, quotes))| {
                let icon = quotes
                    .first()
                    .and_then(|q| pages.iter().find(|pg| q.url.starts_with(&pg.url)))
                    .map(|pg| self.site_icon(pg, 12., p));
                let trigger = div()
                    .id(SharedString::from(format!("cite-{i}-{n}")))
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_1p5()
                    .py(px(1.))
                    .rounded_md()
                    .border_1()
                    .border_color(p.border)
                    .text_xs()
                    .text_color(p.muted)
                    .cursor_pointer()
                    .hover(|d| d.bg(p.hover).text_color(p.text))
                    .child(SharedString::from(format!("[{n}]")))
                    .children(icon)
                    .child(
                        div()
                            .max_w(px(140.))
                            .truncate()
                            .child(SharedString::from(site.clone())),
                    );
                let at = self.flow.cite_at.clone();
                let ws = ws.clone();
                let key = (i, n);
                HoverCard::new(SharedString::from(format!("cite-card-{i}-{n}")))
                    .open_delay(Duration::from_millis(150))
                    .trigger(trigger)
                    .content(move |_, _, _| {
                        let total = quotes.len().max(1);
                        let k = at.borrow().get(&key).copied().unwrap_or(0).min(total - 1);
                        let q = quotes[k].clone();
                        let meta = match q.page {
                            Some(pg) => format!("Claim {} · {} · page {pg}", q.claim, q.verdict),
                            None => format!("Claim {} · {}", q.claim, q.verdict),
                        };
                        let step = |id: &'static str, glyph: &'static str, delta: usize| {
                            let at = at.clone();
                            div()
                                .id(id)
                                .px_1()
                                .rounded_sm()
                                .cursor_pointer()
                                .text_color(p.muted)
                                .hover(|d| d.bg(p.hover).text_color(p.text))
                                .child(glyph)
                                .on_click(move |_, window, _| {
                                    {
                                        let mut m = at.borrow_mut();
                                        let v = m.entry(key).or_insert(0);
                                        *v = (*v + delta) % total;
                                    }
                                    window.refresh();
                                })
                        };
                        let ws = ws.clone();
                        let url = q.url.clone();
                        div()
                            .flex()
                            .flex_col()
                            .gap_1p5()
                            .w(px(300.))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(p.faint)
                                    .child(SharedString::from(format!("[{n}] {site}"))),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::MEDIUM)
                                    .line_clamp(2)
                                    .child(SharedString::from(title.clone())),
                            )
                            .child(
                                div()
                                    .pl_2()
                                    .border_l_2()
                                    .border_color(p.border)
                                    .text_sm()
                                    .text_color(p.muted)
                                    .child(SharedString::from(format!("“{}”", q.quote))),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(p.faint)
                                    .child(SharedString::from(meta)),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .text_xs()
                                    .when(total > 1, |d| {
                                        d.child(step("cite-prev", "‹", total - 1))
                                            .child(div().text_color(p.faint).child(
                                                SharedString::from(format!("{} of {total}", k + 1)),
                                            ))
                                            .child(step("cite-next", "›", 1))
                                    })
                                    .child(div().flex_1())
                                    .child(
                                        div()
                                            .id("cite-open")
                                            .text_color(p.text)
                                            .cursor_pointer()
                                            .hover(|d| d.underline())
                                            .child("Open")
                                            .on_click(move |_, window, cx| {
                                                let _ = ws.update(cx, |this, cx| {
                                                    this.browse(&url, window, cx)
                                                });
                                            }),
                                    ),
                            )
                    })
                    .into_any_element()
            })
            .collect();
        Some(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_1()
                .child(div().text_xs().text_color(p.faint).child("Cited"))
                .children(chips)
                .into_any_element(),
        )
    }

    // ----- Claim review (v2 s09) -----

    /// The run's review, loaded from beside its report the first time.
    fn review_of(&self, key: &str) -> ClaimReview {
        if let Some(r) = self.flow.reviews.borrow().get(key) {
            return r.clone();
        }
        let r = std::fs::read_to_string(crate::flow::review_path(std::path::Path::new(key)))
            .ok()
            .filter(|_| key.ends_with(".md"))
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .map(|v| ClaimReview::from_json(&v))
            .unwrap_or_default();
        self.flow
            .reviews
            .borrow_mut()
            .insert(key.to_owned(), r.clone());
        r
    }

    fn change_review(&self, key: &str, f: impl FnOnce(&mut ClaimReview)) {
        let mut r = self.review_of(key);
        f(&mut r);
        self.flow.reviews.borrow_mut().insert(key.to_owned(), r);
    }

    /// Whether a claim of this run was rejected in review.
    pub(super) fn claim_rejected(&self, run: &Run, claim: usize) -> bool {
        self.review_of(&run_key(run)).rejected.contains(&claim)
    }

    /// "Review claims" over the Report tab's claims: Keep all or Review
    /// each, then Done reviewing.
    pub(super) fn review_bar(
        &self,
        run: &Run,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if run.checks.is_empty() || run.end.is_none() {
            return None;
        }
        let key = run_key(run);
        let r = self.review_of(&key);
        let buttons = div().flex().items_center().gap_1();
        let buttons = if r.reviewing {
            let k = key.clone();
            buttons.child(
                Button::new("review-done")
                    .capsule()
                    .label("Done reviewing")
                    .primary()
                    .xsmall()
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.finish_review(&k, window, cx)),
                    ),
            )
        } else if r.done {
            let k = key.clone();
            buttons
                .child(div().text_xs().text_color(p.muted).child("✓ Reviewed"))
                .child(
                    Button::new("review-again")
                        .capsule()
                        .label("Review again")
                        .ghost()
                        .xsmall()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.change_review(&k, |r| {
                                r.reviewing = true;
                                r.done = false;
                            });
                            cx.notify();
                        })),
                )
        } else {
            let k1 = key.clone();
            let k2 = key.clone();
            buttons
                .child(
                    Button::new("review-keep-all")
                        .capsule()
                        .label("Keep all")
                        .ghost()
                        .xsmall()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.change_review(&k1, |r| {
                                r.rejected.clear();
                                r.reviewing = true;
                            });
                            this.finish_review(&k1, window, cx);
                        })),
                )
                .child(
                    Button::new("review-each")
                        .capsule()
                        .label("Review each")
                        .xsmall()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.change_review(&k2, |r| r.reviewing = true);
                            cx.notify();
                        })),
                )
        };
        Some(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .child("Review claims"),
                )
                .child(buttons)
                .into_any_element(),
        )
    }

    /// Keep, Reject and Comment under a claim while reviewing; its note
    /// once written.
    pub(super) fn claim_actions(
        &self,
        run: &Run,
        claim: usize,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let key = run_key(run);
        let r = self.review_of(&key);
        let note = r.notes.get(&claim).cloned();
        let editing = self
            .flow
            .note
            .as_ref()
            .filter(|(k, c, _)| *k == key && *c == claim)
            .map(|(_, _, input)| input.clone());
        if !r.reviewing {
            return note.map(|n| {
                div()
                    .text_xs()
                    .text_color(p.faint)
                    .child(SharedString::from(format!("Your note: {n}")))
                    .into_any_element()
            });
        }
        let rejected = r.rejected.contains(&claim);
        let k_reject = key.clone();
        let k_note = key.clone();
        let row = div()
            .flex()
            .items_center()
            .gap_1()
            .child(
                Button::new(SharedString::from(format!("claim-reject-{claim}")))
                    .capsule()
                    .label(if rejected { "Keep" } else { "Reject" })
                    .ghost()
                    .xsmall()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.change_review(&k_reject, |r| r.toggle_reject(claim));
                        cx.notify();
                    })),
            )
            .child(
                Button::new(SharedString::from(format!("claim-note-{claim}")))
                    .capsule()
                    .label(if note.is_some() {
                        "Edit comment"
                    } else {
                        "Add comment"
                    })
                    .ghost()
                    .xsmall()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_claim_note(&k_note, claim, window, cx)
                    })),
            )
            .when(rejected, |d| {
                d.child(div().text_xs().text_color(p.faint).child("rejected"))
            });
        let field = editing.map(|input| {
            let k = key.clone();
            div()
                .flex()
                .items_center()
                .gap_1()
                .child(div().flex_1().min_w_0().child(Input::new(&input).small()))
                .child(
                    Button::new(SharedString::from(format!("claim-note-save-{claim}")))
                        .capsule()
                        .label("Save")
                        .xsmall()
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.save_claim_note(&k, claim, cx)),
                        ),
                )
        });
        Some(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(row)
                .children(field)
                .when_some(
                    note.filter(|_| editing_is_none(&self.flow.note, &key, claim)),
                    |d, n| {
                        d.child(
                            div()
                                .text_xs()
                                .text_color(p.faint)
                                .child(SharedString::from(format!("Your note: {n}"))),
                        )
                    },
                )
                .into_any_element(),
        )
    }

    fn open_claim_note(
        &mut self,
        key: &str,
        claim: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let note = self
            .review_of(key)
            .notes
            .get(&claim)
            .cloned()
            .unwrap_or_default();
        let input = cx.new(|cx| {
            let mut i = InputState::new(window, cx).placeholder("A note for Tern's next pass");
            i.set_value(note, window, cx);
            i
        });
        input.update(cx, |i, cx| i.focus(window, cx));
        self.flow.note = Some((key.to_owned(), claim, input));
        cx.notify();
    }

    fn save_claim_note(&mut self, key: &str, claim: usize, cx: &mut Context<Self>) {
        let Some((_, _, input)) = self.flow.note.take() else {
            return;
        };
        let text = input.read(cx).value().to_string();
        self.change_review(key, |r| r.set_note(claim, &text));
        cx.notify();
    }

    /// Ends the review: saves it beside the report, strikes rejected
    /// claims, and puts the notes in the composer as the next message.
    fn finish_review(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((k, claim, input)) = self.flow.note.take() {
            let text = input.read(cx).value().to_string();
            self.change_review(&k, |r| r.set_note(claim, &text));
        }
        self.change_review(key, |r| {
            r.reviewing = false;
            r.done = true;
        });
        let r = self.review_of(key);
        if key.ends_with(".md") {
            let path = crate::flow::review_path(std::path::Path::new(key));
            if let Err(e) = std::fs::write(&path, r.to_json().to_string()) {
                self.model
                    .toast_error(format!("Could not save the review: {e}"), None);
            }
        }
        if let Some(text) = r.follow_up() {
            self.composer.update(cx, |c, cx| {
                crate::view::set_end(c, text, window, cx);
                c.focus(window, cx);
            });
        }
        self.model.toast_info(r.summary());
        cx.notify();
    }

    /// The UI tour's claim review: start, reject claim 3, note on claim 4,
    /// then finish.
    pub(super) fn tour_review(&mut self, step: u8, window: &mut Window, cx: &mut Context<Self>) {
        let key = run_key(self.model.panel_run());
        match step {
            0 => {
                self.model.run_panel_open = true;
                self.model.set_run_view(crate::model::RunView::Report);
                self.change_review(&key, |r| r.reviewing = true);
            }
            1 => self.change_review(&key, |r| r.toggle_reject(3)),
            2 => self.change_review(&key, |r| {
                r.set_note(4, "Check against the Adaptive-RAG appendix")
            }),
            _ => self.finish_review(&key, window, cx),
        }
        cx.notify();
    }

    // ----- Export (v2 s15) -----

    /// Export buttons for the saved report: Markdown, and a printable HTML
    /// page (print it to PDF from the browser).
    pub(super) fn export_buttons(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .items_center()
            .gap_1()
            .child(
                Button::new("report-export-md")
                    .capsule()
                    .label("Export Markdown")
                    .ghost()
                    .xsmall()
                    .tooltip("The report with your review, as Markdown beside it")
                    .on_click(cx.listener(|this, _, _, cx| this.export_report(false, cx))),
            )
            .child(
                Button::new("report-export-html")
                    .capsule()
                    .label("Export HTML / PDF")
                    .ghost()
                    .xsmall()
                    .tooltip("A printable page; print it to PDF from your browser")
                    .on_click(cx.listener(|this, _, _, cx| this.export_report(true, cx))),
            )
            .into_any_element()
    }

    fn export_report(&mut self, html: bool, cx: &mut Context<Self>) {
        let made = {
            let run = self.model.panel_run();
            let Some(f) = run.report.clone() else {
                return;
            };
            let key = f.markdown.to_string_lossy().to_string();
            let review = self.review_of(&key);
            std::fs::read_to_string(&f.markdown)
                .map_err(|e| e.to_string())
                .and_then(|text| {
                    let md = crate::flow::export_markdown(&text, run, Some(&review));
                    let out = crate::flow::export_path(&f.markdown, html);
                    let body = if html {
                        let title = md
                            .lines()
                            .find_map(|l| l.strip_prefix("# "))
                            .unwrap_or("Tern report")
                            .to_owned();
                        crate::flow::to_html(&title, &md)
                    } else {
                        md
                    };
                    std::fs::write(&out, body)
                        .map(|_| out)
                        .map_err(|e| e.to_string())
                })
        };
        match made {
            Ok(out) => {
                let name = out
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                self.model.toast_info(format!("Exported {name}"));
                cx.open_url(&tern_agent::platform::file_url(&out.to_string_lossy()));
            }
            Err(e) => self
                .model
                .toast_error(format!("Could not export the report: {e}"), None),
        }
        cx.notify();
    }

    // ----- Splash and title bar (v2 s01, s15) -----

    /// Seconds since the window opened, while the splash still shows.
    fn splash_t(&self) -> Option<f32> {
        if self.reduced_motion {
            return None;
        }
        let t = self.flow.launched.elapsed().as_secs_f32();
        (t < SPLASH_IN + SPLASH_HOLD + SPLASH_FLY).then_some(t)
    }

    /// Whether the title bar's mark waits for the splash to land in it.
    pub(super) fn mark_in_flight(&self) -> bool {
        self.splash_t().is_some()
    }

    /// On launch: the mark settles in the middle of the window, then flies
    /// to its place in the title bar while the cover fades.
    pub(super) fn splash(&self, p: Palette, window: &Window) -> Option<AnyElement> {
        let t = self.splash_t()?;
        self.anim_pending.set(true);
        let view = window.viewport_size();
        let (cx0, cy0) = (f32::from(view.width) / 2., f32::from(view.height) / 2.);
        // The title bar's mark: after the traffic lights on macOS.
        let slot_x = if cfg!(target_os = "macos") { 80. } else { 12. } + 7.5;
        let slot_y = 17.;
        let ease = crate::motion::ease;
        let (x, y, size, mark_a, cover_a) = if t < SPLASH_IN {
            let k = ease(t / SPLASH_IN);
            (cx0, cy0, SPLASH_MARK * (0.6 + 0.4 * k), k, 1.)
        } else if t < SPLASH_IN + SPLASH_HOLD {
            (cx0, cy0, SPLASH_MARK, 1., 1.)
        } else {
            let k = ease(((t - SPLASH_IN - SPLASH_HOLD) / SPLASH_FLY).clamp(0., 1.));
            (
                cx0 + (slot_x - cx0) * k,
                cy0 + (slot_y - cy0) * k,
                SPLASH_MARK + (15. - SPLASH_MARK) * k,
                1.,
                1. - k,
            )
        };
        Some(
            div()
                .absolute()
                .inset_0()
                .child(div().absolute().inset_0().bg(p.canvas).opacity(cover_a))
                .child(
                    svg()
                        .path(crate::assets::MARK)
                        .absolute()
                        .left(px(x - size / 2.))
                        .top(px(y - size / 2.))
                        .size(px(size))
                        .text_color(p.text)
                        .opacity(mark_a),
                )
                .into_any_element(),
        )
    }

    /// Where the chat sits, in the title bar: its project, then its title.
    pub(super) fn title_crumb(&self, p: Palette) -> Option<AnyElement> {
        if !matches!(self.model.screen, Screen::Chat) {
            return None;
        }
        let tab = self.tabs.active_tab();
        let chat = if tab.title.trim().is_empty() {
            "New chat".to_owned()
        } else {
            tab.title.clone()
        };
        // A draft is not in the list yet: it goes where the tray put it
        // (#74).
        let project = match tab
            .id
            .as_ref()
            .and_then(|id| self.model.sessions.iter().find(|s| &s.id == id))
        {
            Some(s) => s
                .project
                .and_then(|pid| self.model.projects.find(pid))
                .map(|pr| pr.row.name.clone()),
            None => self.model.projects.new_in.clone(),
        }
        .unwrap_or_else(|| "Inbox".to_owned());
        Some(
            div()
                .id("title-crumb")
                .flex()
                .items_center()
                .gap_1()
                .min_w_0()
                .max_w(px(360.))
                .text_xs()
                .text_color(p.faint)
                .child(div().flex_none().child(SharedString::from(project)))
                .child(div().flex_none().child("›"))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(p.muted)
                        .child(SharedString::from(chat)),
                )
                .into_any_element(),
        )
    }

    /// The title bar's search button: opens the command palette.
    pub(super) fn search_button(&self, cx: &mut Context<Self>) -> AnyElement {
        Button::new("title-search")
            .capsule()
            .icon(IconName::Search)
            .ghost()
            .small()
            .tooltip(if cfg!(target_os = "macos") {
                "Search commands (Cmd+K)"
            } else {
                "Search commands (Ctrl+K)"
            })
            .on_click(cx.listener(|this, _, _, cx| {
                this.cmdk = Some(Default::default());
                cx.notify();
            }))
            .into_any_element()
    }
}

fn editing_is_none(
    note: &Option<(String, usize, Entity<InputState>)>,
    key: &str,
    claim: usize,
) -> bool {
    !note
        .as_ref()
        .is_some_and(|(k, c, _)| k == key && *c == claim)
}
