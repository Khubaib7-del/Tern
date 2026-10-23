//! Deep research in the window: the composer's mode toggle, the plan card
//! (Approve, Edit, Skip plan, Cancel; in edit mode a field per
//! sub-question and Run), and the run panel's Workers view (one row per
//! worker with its status, pages read with their sites' logos, tokens,
//! and a cancel button).

use gpui_kit::assets::IconName as AssetIcon;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, FontWeight, SharedString, Window, div, px};
use tern_agent::deep::{PlanAnswer, PlanSource};
use tern_agent::run::compact;
use tern_agent::{ResearchPlan, Run, WorkerRow, WorkerStatus};

use super::Workspace;
use crate::theme::Palette;

impl Workspace {
    /// Hands the plan to the window: one text field per sub-question for
    /// editing, then the card.
    pub(super) fn show_plan(
        &mut self,
        plan: ResearchPlan,
        reply: tokio::sync::oneshot::Sender<PlanAnswer>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.cancel.is_cancelled() {
            let _ = reply.send(PlanAnswer::Cancel);
            return;
        }
        self.plan_inputs = plan
            .sub_questions
            .iter()
            .map(|s| {
                let text = s.question.clone();
                cx.new(|cx| {
                    let mut i = InputState::new(window, cx).placeholder("Sub-question");
                    i.set_value(text, window, cx);
                    i
                })
            })
            .collect();
        self.plan_dropped.clear();
        self.plan_reply = Some(reply);
        self.model.ask_plan(plan);
    }

    /// Sends the user's answer to the waiting run.
    pub(super) fn answer_plan(&mut self, answer: PlanAnswer, cx: &mut Context<Self>) {
        if let Some(r) = self.plan_reply.take() {
            let _ = r.send(answer);
        }
        self.plan_inputs.clear();
        self.plan_dropped.clear();
        self.model.plan_answered();
        cx.notify();
    }

    /// The plan with the edit fields' text; dropped rows left out. A
    /// reworded sub-question searches for its new wording.
    fn edited_plan(&self, cx: &Context<Self>) -> Option<ResearchPlan> {
        let (plan, _) = self.model.pending_plan()?;
        let mut out = plan.clone();
        out.sub_questions.clear();
        for (i, s) in plan.sub_questions.iter().enumerate() {
            if self.plan_dropped.contains(&i) {
                continue;
            }
            let text = self
                .plan_inputs
                .get(i)
                .map(|e| e.read(cx).value().to_string())
                .unwrap_or_else(|| s.question.clone());
            let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
            if text.is_empty() {
                continue;
            }
            let mut s = s.clone();
            if text != s.question {
                s.queries = vec![text.clone()];
                s.intent.clear();
                s.question = text;
            }
            out.sub_questions.push(s);
        }
        Some(out)
    }

    /// The plan card in the transcript while a plan waits.
    pub(super) fn plan_card(&self, p: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (plan, editing) = self.model.pending_plan()?;
        let (low, high) = plan.estimate();
        let workers = self
            .model
            .panel_run()
            .deep
            .as_ref()
            .map_or(3, |d| d.concurrency.max(1));
        let by = match plan.source {
            PlanSource::Model => "Drafted by the model",
            PlanSource::Fallback => "Tern's own split (the model's plan could not be read)",
            PlanSource::Skipped => "One worker",
        };
        let minutes = plan.budget.seconds.div_ceil(60);
        let meta = format!(
            "{by} · about {}–{} tokens · up to {} pages · {minutes} min · {workers} at once",
            compact(low),
            compact(high),
            plan.budget.pages,
        );
        let rows = plan.sub_questions.iter().enumerate().map(|(i, s)| {
            let dropped = self.plan_dropped.contains(&i);
            let kinds: Vec<&str> = s.sources.iter().map(|k| k.label()).collect();
            let detail = format!(
                "{} · {} · {}%",
                s.queries.join("; "),
                kinds.join(", "),
                s.share
            );
            let question: AnyElement = match (editing, self.plan_inputs.get(i)) {
                (true, Some(input)) if !dropped => Input::new(input).small().into_any_element(),
                _ => div()
                    .text_sm()
                    .text_color(if dropped { p.faint } else { p.text })
                    .child(SharedString::from(if dropped {
                        format!("{} (dropped)", s.question)
                    } else {
                        s.question.clone()
                    }))
                    .into_any_element(),
            };
            div()
                .flex()
                .items_start()
                .gap_2()
                .py_1()
                .child(
                    div()
                        .w(px(18.))
                        .flex_none()
                        .pt(px(2.))
                        .text_xs()
                        .text_color(p.faint)
                        .child(format!("{}.", s.n)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(question)
                        .when(!s.intent.is_empty() && !editing, |d| {
                            d.child(
                                div()
                                    .text_xs()
                                    .text_color(p.muted)
                                    .child(SharedString::from(s.intent.clone())),
                            )
                        })
                        .child(
                            div()
                                .text_xs()
                                .text_color(p.faint)
                                .truncate()
                                .child(SharedString::from(detail)),
                        ),
                )
                .when(editing, |d| {
                    d.child(
                        Button::new(SharedString::from(format!("plan-drop-{i}")))
                            .icon(if dropped {
                                AssetIcon::RotateCw
                            } else {
                                AssetIcon::X
                            })
                            .ghost()
                            .xsmall()
                            .tooltip(if dropped { "Keep" } else { "Drop" })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !this.plan_dropped.remove(&i) {
                                    this.plan_dropped.insert(i);
                                }
                                cx.notify();
                            })),
                    )
                })
        });
        let buttons = if editing {
            div()
                .flex()
                .gap_2()
                .child(
                    Button::new("plan-run")
                        .label("Run")
                        .small()
                        .on_click(cx.listener(|this, _, _, cx| {
                            match this.edited_plan(cx) {
                                Some(p) if !p.sub_questions.is_empty() => {
                                    this.answer_plan(PlanAnswer::Run(p), cx)
                                }
                                // Everything dropped: one worker on the question.
                                _ => this.answer_plan(PlanAnswer::Skip, cx),
                            }
                        })),
                )
                .child(
                    Button::new("plan-discard")
                        .label("Discard edits")
                        .ghost()
                        .small()
                        .on_click(cx.listener(|this, _, window, cx| {
                            if let Some((plan, _)) = this.model.pending_plan() {
                                let texts: Vec<String> = plan
                                    .sub_questions
                                    .iter()
                                    .map(|s| s.question.clone())
                                    .collect();
                                for (input, text) in this.plan_inputs.iter().zip(texts) {
                                    input.update(cx, |i, cx| i.set_value(text, window, cx));
                                }
                            }
                            this.plan_dropped.clear();
                            this.model.set_plan_editing(false);
                            cx.notify();
                        })),
                )
        } else {
            div()
                .flex()
                .gap_2()
                .child(
                    Button::new("plan-approve")
                        .label("Approve")
                        .small()
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some((plan, _)) = this.model.pending_plan() {
                                let plan = plan.clone();
                                this.answer_plan(PlanAnswer::Run(plan), cx);
                            }
                        })),
                )
                .child(
                    Button::new("plan-edit")
                        .label("Edit")
                        .small()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.model.set_plan_editing(true);
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("plan-skip")
                        .label("Skip plan")
                        .ghost()
                        .small()
                        .tooltip("One worker on the whole question")
                        .on_click(
                            cx.listener(|this, _, _, cx| this.answer_plan(PlanAnswer::Skip, cx)),
                        ),
                )
                .child(
                    Button::new("plan-cancel")
                        .label("Cancel")
                        .ghost()
                        .small()
                        .on_click(
                            cx.listener(|this, _, _, cx| this.answer_plan(PlanAnswer::Cancel, cx)),
                        ),
                )
        };
        Some(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .p_3()
                .rounded_xl()
                .border_1()
                .border_color(p.border)
                .bg(p.raised)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div().text_color(p.muted).child(
                                gpui_kit::component::Icon::new(AssetIcon::ListChecks).small(),
                            ),
                        )
                        .child(div().font_weight(FontWeight::MEDIUM).child("Research plan")),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(p.faint)
                        .child(SharedString::from(meta)),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(p.muted)
                        .child(SharedString::from(plan.question.clone())),
                )
                .child(div().flex().flex_col().children(rows))
                .child(
                    div()
                        .text_xs()
                        .text_color(p.faint)
                        .child("Nothing is searched or read until you run the plan."),
                )
                .child(buttons)
                .into_any_element(),
        )
    }

    /// The run panel's Workers view.
    pub(super) fn run_workers(&self, run: &Run, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let Some(deep) = &run.deep else {
            return self.run_summary(run, p, cx);
        };
        if deep.workers.is_empty() {
            let text = match &deep.plan {
                Some(_) => "Workers start when you run the plan.",
                None => "Drafting the plan.",
            };
            return div()
                .text_sm()
                .text_color(p.faint)
                .child(text)
                .into_any_element();
        }
        let live = run.end.is_none();
        let head = format!(
            "{} of {} done · {} at once",
            deep.finished(),
            deep.workers.len(),
            deep.concurrency.max(1)
        );
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_xs()
                    .text_color(p.faint)
                    .child(SharedString::from(head)),
            )
            .children(deep.workers.iter().map(|w| self.worker_row(w, live, p, cx)))
            .into_any_element()
    }

    fn worker_row(
        &self,
        w: &WorkerRow,
        live: bool,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let n = w.n;
        let status: AnyElement = match w.status {
            WorkerStatus::Running if live => {
                Spinner::new().xsmall().color(p.muted).into_any_element()
            }
            s => div()
                .text_color(if s.is_ok() { p.faint } else { p.text })
                .child(
                    gpui_kit::component::Icon::new(match s {
                        WorkerStatus::Done => AssetIcon::CircleCheck,
                        WorkerStatus::Queued => AssetIcon::CircleDashed,
                        WorkerStatus::Running => AssetIcon::CircleDot,
                        WorkerStatus::Cancelled => AssetIcon::CircleStop,
                        WorkerStatus::TimedOut => AssetIcon::Clock,
                        WorkerStatus::OverBudget | WorkerStatus::Failed => AssetIcon::TriangleAlert,
                    })
                    .xsmall(),
                )
                .into_any_element(),
        };
        let tokens = w.tokens_in + w.tokens_out;
        // Pages read, then the worker's page budget: never "0 pages of 7",
        // which reads as seven results found.
        let mut detail = format!(
            "{} · {} page{} read",
            w.status.label(),
            w.pages.len(),
            if w.pages.len() == 1 { "" } else { "s" }
        );
        if let Some(b) = w.budget {
            detail.push_str(&format!(" (budget {})", b.pages));
        }
        if tokens > 0 {
            detail.push_str(&format!(" · {} tokens", compact(tokens)));
        }
        let stack = div()
            .flex()
            .items_center()
            .children(w.pages.iter().take(6).enumerate().map(|(k, pg)| {
                div()
                    .when(k > 0, |d| d.ml(px(-3.)))
                    .rounded_sm()
                    .border_1()
                    .border_color(p.sidebar)
                    .child(self.site_icon(pg, 14., p))
            }));
        let can_cancel = live && !w.status.is_finished();
        div()
            .id(SharedString::from(format!("worker-{n}")))
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
                    .items_start()
                    .gap_2()
                    .child(div().pt(px(2.)).flex_none().child(status))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .text_color(p.text)
                            .child(SharedString::from(format!("{n}. {}", w.question))),
                    )
                    .when(can_cancel, |d| {
                        d.child(
                            Button::new(SharedString::from(format!("worker-cancel-{n}")))
                                .icon(AssetIcon::X)
                                .ghost()
                                .xsmall()
                                .tooltip("Stop this worker")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.deep_control.cancel_worker(n);
                                    cx.notify();
                                })),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .pl(px(20.))
                    .when(!w.pages.is_empty(), |d| d.child(stack))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .text_color(p.faint)
                            .child(SharedString::from(detail)),
                    ),
            )
            .when(!w.note.is_empty() && !w.status.is_ok(), |d| {
                d.child(
                    div()
                        .pl(px(20.))
                        .text_xs()
                        .text_color(p.muted)
                        .child(SharedString::from(w.note.clone())),
                )
            })
            .when(!w.pages.is_empty(), |d| {
                d.children(w.pages.iter().enumerate().map(|(k, pg)| {
                    let url = pg.url.clone();
                    div()
                        .id(SharedString::from(format!("worker-{n}-page-{k}")))
                        .flex()
                        .items_center()
                        .gap_2()
                        .ml(px(20.))
                        .px_1()
                        .py(px(3.))
                        .rounded_md()
                        .cursor_pointer()
                        .hover(|d| d.bg(p.hover))
                        .child(self.site_icon(pg, 14., p))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(p.muted)
                                .child(SharedString::from(pg.label().to_owned())),
                        )
                        .on_click(
                            cx.listener(move |this, _, window, cx| this.browse(&url, window, cx)),
                        )
                }))
            })
            .into_any_element()
    }
}
