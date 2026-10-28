//! The plan card (#77): the model's plan for a chat (`plan`) and a task
//! graph it ran (`run_tasks`), above the tool cards of the answer. Both
//! update live from the run's `plan.updated` events: a step's mark moves
//! from pending to in progress to done; a graph's tasks also say which
//! tool they call and what they wait for. Restrained: grey, the done
//! green and the failed red the tool cards use, no fills.

use gpui_kit::assets::IconName as AssetIcon;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, FontWeight, SharedString, div, px};
use tern_agent::Run;
use tern_agent::planner::{PlanStep, PlanView, StepStatus};

use super::Workspace;
use super::work_view::{done_green, fail_red};
use crate::theme::Palette;

/// Steps a folded card still shows: the one in progress and the next.
const FOLDED_STEPS: usize = 3;

fn status_icon(s: StepStatus, p: Palette) -> AnyElement {
    let icon = |i: AssetIcon, ink: gpui_kit::Hsla| {
        div()
            .flex_none()
            .text_color(ink)
            .child(gpui_kit::component::Icon::new(i).xsmall())
            .into_any_element()
    };
    match s {
        StepStatus::Pending => icon(AssetIcon::CircleDashed, p.faint),
        StepStatus::InProgress => div()
            .flex_none()
            .child(Spinner::new().xsmall().color(p.text))
            .into_any_element(),
        StepStatus::Done => icon(AssetIcon::CircleCheck, done_green(p)),
        StepStatus::Failed => icon(AssetIcon::CircleX, fail_red(p)),
        StepStatus::Skipped => icon(AssetIcon::CircleStop, p.faint),
    }
}

impl Workspace {
    /// The plan and task graph cards of an answer (`entry`) or the run in
    /// progress (`None`).
    pub(super) fn plan_cards(
        &self,
        entry: Option<usize>,
        run: &Run,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (plan, graph) = tern_agent::planner::of_run(run);
        if plan.is_none() && graph.is_none() {
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
                .children(plan.map(|v| self.plan_block(format!("{base}-plan"), &v, p, cx)))
                .children(graph.map(|v| self.plan_block(format!("{base}-graph"), &v, p, cx)))
                .into_any_element(),
        )
    }

    fn plan_block(
        &self,
        key: String,
        v: &PlanView,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let folded = self.work.open.get(&key).copied() == Some(false);
        let n = v.steps.len();
        let done = v.done();
        let k2 = key.clone();
        let (icon, label) = if v.is_graph() {
            (AssetIcon::Workflow, "Task graph")
        } else {
            (AssetIcon::ListChecks, "Plan")
        };
        let mut meta = v.progress();
        if v.is_graph() {
            let running = v.count(StepStatus::InProgress);
            if running > 0 {
                meta.push_str(&format!(" · {running} running"));
            }
            meta.push_str(&format!(
                " · {} {}",
                v.waves,
                if v.waves == 1 { "wave" } else { "waves" }
            ));
        }
        let head = div()
            .id(SharedString::from(format!("plan-{key}")))
            .flex()
            .items_center()
            .gap_2()
            .min_w_0()
            .cursor_pointer()
            .child(
                div()
                    .flex_none()
                    .text_color(p.muted)
                    .child(gpui_kit::component::Icon::new(icon).xsmall()),
            )
            .child(
                div()
                    .flex_none()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(p.text)
                    .child(label),
            )
            .children(v.title.as_ref().map(|t| {
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .text_color(p.muted)
                    .child(SharedString::from(t.clone()))
            }))
            .when(v.title.is_none(), |d| d.child(div().flex_1()))
            .child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(p.faint)
                    .child(SharedString::from(meta)),
            )
            .child(
                div().flex_none().text_color(p.faint).child(
                    gpui_kit::component::Icon::new(if folded {
                        AssetIcon::ChevronRight
                    } else {
                        AssetIcon::ChevronDown
                    })
                    .xsmall(),
                ),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                let now = this.work.open.get(&k2).copied() == Some(false);
                this.work.open.insert(k2.clone(), now);
                cx.notify();
            }));
        // A thin bar: the share of steps done.
        let share = if n == 0 { 0. } else { done as f32 / n as f32 };
        let bar = div().h(px(2.)).w_full().rounded_sm().bg(p.border).child(
            div()
                .h_full()
                .rounded_sm()
                .bg(done_green(p))
                .w(gpui_kit::relative(share)),
        );
        let shown: Vec<&PlanStep> = if folded {
            // Folded: the step in progress and the next few.
            let from = v
                .steps
                .iter()
                .position(|s| !matches!(s.status, StepStatus::Done))
                .unwrap_or(0);
            v.steps.iter().skip(from).take(FOLDED_STEPS).collect()
        } else {
            v.steps.iter().collect()
        };
        let hidden = n.saturating_sub(shown.len());
        let rows = shown.into_iter().map(|s| self.plan_row(v.is_graph(), s, p));
        div()
            .id(SharedString::from(format!("plancard-{key}")))
            .flex()
            .flex_col()
            .gap_1p5()
            .px_2()
            .py_1p5()
            .rounded_md()
            .border_1()
            .border_color(p.border)
            .bg(p.raised)
            .child(head)
            .child(bar)
            .child(div().flex().flex_col().gap_1().children(rows))
            .when(folded && hidden > 0, |d| {
                d.child(
                    div()
                        .text_xs()
                        .text_color(p.faint)
                        .child(SharedString::from(format!("{hidden} more steps"))),
                )
            })
            .when(!v.notes.is_empty() && !folded, |d| {
                d.child(
                    div()
                        .text_xs()
                        .text_color(p.faint)
                        .child(SharedString::from(format!(
                            "Tern fixed: {}",
                            v.notes.join("; ")
                        ))),
                )
            })
            .into_any_element()
    }

    fn plan_row(&self, graph: bool, s: &PlanStep, p: Palette) -> AnyElement {
        let ink = if s.status == StepStatus::InProgress {
            p.text
        } else {
            p.muted
        };
        div()
            .flex()
            .items_center()
            .gap_2()
            .min_w_0()
            .child(status_icon(s.status, p))
            .when(graph, |d| {
                d.child(
                    div()
                        .flex_none()
                        .w(px(22.))
                        .font_family(crate::assets::MONO_FONT)
                        .text_xs()
                        .text_color(p.faint)
                        .child(SharedString::from(s.id.clone())),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .text_color(ink)
                    .when(s.status == StepStatus::InProgress, |d| {
                        d.font_weight(FontWeight::MEDIUM)
                    })
                    .child(SharedString::from(s.title.clone())),
            )
            .when(graph && !s.after.is_empty(), |d| {
                d.child(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(p.faint)
                        .child(SharedString::from(format!("after {}", s.after.join(", ")))),
                )
            })
            .when(graph, |d| {
                d.child(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(p.faint)
                        .child(s.status.label()),
                )
            })
            .into_any_element()
    }
}
