//! The Evaluation page: saved runs as a table (each row with its subject's
//! logo, or Tern's mark offline), the selected run's suites with every
//! metric, its 95% interval, and how it moved since the previous run of
//! the same rung, and each suite's failures, published in full.

use gpui_kit::assets::IconName as AssetIcon;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{Disableable as _, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, FontWeight, SharedString, Window, div, px, svg};
use tern_agent::eval::{Change, EvalRun, Metric, RunSummary, SuiteKind, SuiteResult};

use super::Workspace;
use crate::assets;
use crate::evaluation::short_time;
use crate::theme::Palette;

const PAGE_MAX_W: f32 = 880.;
const TRIALS: usize = 3;

fn tile(p: Palette, inner: AnyElement) -> AnyElement {
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
        .child(inner)
        .into_any_element()
}

/// Who answered: the provider's logo when Tern has one, else Tern's mark.
fn subject_logo(provider: Option<&str>, p: Palette) -> AnyElement {
    let path = match provider {
        Some(id) if assets::service_names().any(|n| n == id) => assets::service_path(id),
        _ => assets::MARK.into(),
    };
    tile(
        p,
        svg()
            .path(path)
            .size(px(15.))
            .text_color(p.text)
            .into_any_element(),
    )
}

fn kind_icon(kind: SuiteKind) -> AssetIcon {
    match kind {
        SuiteKind::Research => AssetIcon::Search,
        SuiteKind::Claims => AssetIcon::BadgeCheck,
        SuiteKind::Quiz => AssetIcon::GraduationCap,
        SuiteKind::Decision => AssetIcon::Signpost,
        SuiteKind::Safety => AssetIcon::ShieldCheck,
        SuiteKind::Tests => AssetIcon::ListChecks,
        SuiteKind::Usability => AssetIcon::Users,
    }
}

fn kind_tile(kind: SuiteKind, p: Palette) -> AnyElement {
    tile(
        p,
        svg()
            .path(kind_icon(kind).path())
            .size(px(15.))
            .text_color(p.text)
            .into_any_element(),
    )
}

impl Workspace {
    pub(super) fn open_eval(&mut self, cx: &mut Context<Self>) {
        self.show_settings(crate::settings::Section::Evaluation);
        if !self.model.eval.loaded {
            self.bridge.eval_load(None);
        }
        cx.notify();
    }

    fn eval_start(&mut self, cx: &mut Context<Self>) {
        if self.model.eval.running {
            return;
        }
        self.model.eval.running = true;
        self.model.eval.note = None;
        self.bridge.eval_run(TRIALS);
        cx.notify();
    }

    pub(super) fn eval_screen(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let st = &self.model.eval;
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
                    .child(div().text_xl().font_weight(FontWeight::MEDIUM).child("Evaluation"))
                    .child(div().text_sm().text_color(p.muted).child(
                        "Tern's own test ladder: claim verdicts, research answers, quiz grading, decision routing on a held-out split, and injection safety. The offline suites run here in a few seconds; live, pinned, public, and usability runs start from the terminal (tern eval run --live, --level pinned, --level public, tern eval usability) and show up here.",
                    )),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Button::new("eval-run")
                            .icon(AssetIcon::Play)
                            .label("Run offline suites")
                            .small()
                            .disabled(st.running)
                            .on_click(cx.listener(|this, _, _, cx| this.eval_start(cx))),
                    ),
            );
        let status: AnyElement = if st.running {
            div()
                .flex()
                .items_center()
                .gap_2()
                .text_xs()
                .text_color(p.muted)
                .child(Spinner::new().small().color(p.muted))
                .child(SharedString::from(format!(
                    "Running every offline suite, {TRIALS} trials…"
                )))
                .into_any_element()
        } else {
            match &st.note {
                Some(Ok(t)) => div()
                    .text_xs()
                    .text_color(p.muted)
                    .child(SharedString::from(t.clone()))
                    .into_any_element(),
                Some(Err(e)) => div()
                    .text_xs()
                    .text_color(p.text)
                    .child(SharedString::from(format!("The run failed: {e}")))
                    .into_any_element(),
                None => div().into_any_element(),
            }
        };
        let mut column = div()
            .w_full()
            .max_w(px(PAGE_MAX_W))
            .flex()
            .flex_col()
            .gap_5()
            .py_6()
            .px_4()
            .child(header)
            .child(status);
        if !st.loaded {
            column = column.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_sm()
                    .text_color(p.muted)
                    .child(Spinner::new().small().color(p.muted))
                    .child("Reading saved runs…"),
            );
        } else if st.runs.is_empty() {
            column = column.child(
                div()
                    .text_sm()
                    .text_color(p.muted)
                    .child("No runs yet. Run the offline suites to get Tern's first scores."),
            );
        } else {
            column = column.child(self.eval_runs_table(p, cx));
            if let Some(run) = st.selected.clone() {
                column = column.child(self.eval_run_detail(&run, p, cx));
            }
        }
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .items_center()
            .child(column)
            .overflow_y_scrollbar()
            .id("evaluation")
            .into_any_element()
    }

    fn eval_runs_table(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let st = &self.model.eval;
        let rows: Vec<AnyElement> = st
            .runs
            .iter()
            .take(12)
            .map(|r| self.eval_run_row(r, st.selected_id() == Some(r.id.as_str()), p, cx))
            .collect();
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(div().text_xs().text_color(p.faint).child("Runs"))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .rounded_xl()
                    .border_1()
                    .border_color(p.border)
                    .children(rows),
            )
            .into_any_element()
    }

    fn eval_run_row(
        &self,
        r: &RunSummary,
        selected: bool,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = r.id.clone();
        let heads: Vec<String> = r
            .headlines
            .iter()
            .take(3)
            .map(|(s, _, v)| format!("{s} {v}"))
            .collect();
        div()
            .id(SharedString::from(format!("eval-row-{}", r.id)))
            .flex()
            .items_center()
            .gap_3()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(p.border)
            .when(selected, |d| d.bg(p.hover))
            .hover(|d| d.bg(p.hover))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                this.bridge.eval_load(Some(id.clone()));
                cx.notify();
            }))
            .child(subject_logo(r.provider.as_deref(), p))
            .child(
                div()
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
                                    .child(SharedString::from(r.id.clone())),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(p.faint)
                                    .child(SharedString::from(format!(
                                        "{} · {} · {} · {} trial{}",
                                        r.level.label(),
                                        r.mode,
                                        r.subject,
                                        r.trials,
                                        if r.trials == 1 { "" } else { "s" }
                                    ))),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(p.muted)
                            .truncate()
                            .child(SharedString::from(heads.join(" · "))),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_end()
                    .child(
                        div()
                            .text_sm()
                            .child(SharedString::from(format!("{}/{}", r.passed, r.attempted))),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(p.faint)
                            .child(SharedString::from(short_time(&r.started))),
                    ),
            )
            .into_any_element()
    }

    fn eval_run_detail(&self, run: &EvalRun, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let st = &self.model.eval;
        let (passed, attempted) = run.passed();
        let against = match &st.previous {
            Some(prev) => format!(
                "Against {}: {} regression{}",
                prev.id,
                st.regressions(),
                if st.regressions() == 1 { "" } else { "s" }
            ),
            None => "No earlier run of this rung to compare with.".into(),
        };
        let mut d = div().flex().flex_col().gap_3().child(
            div()
                .flex()
                .items_center()
                .gap_3()
                .child(subject_logo(run.provider.as_deref(), p))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .child(div().text_base().font_weight(FontWeight::MEDIUM).child(
                            SharedString::from(format!("{} · {passed}/{attempted} passed", run.id)),
                        ))
                        .child(
                            div()
                                .text_xs()
                                .text_color(p.muted)
                                .child(SharedString::from(against)),
                        ),
                ),
        );
        for n in &run.notes {
            d = d.child(
                div()
                    .text_xs()
                    .text_color(p.faint)
                    .child(SharedString::from(n.clone())),
            );
        }
        for s in &run.suites {
            d = d.child(self.eval_suite_card(s, p, cx));
        }
        d.into_any_element()
    }

    fn eval_suite_card(&self, s: &SuiteResult, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let st = &self.model.eval;
        let open = st.is_open(&s.id);
        let sid = s.id.clone();
        let head = s
            .headline()
            .map(|m| m.show(m.mean))
            .unwrap_or_else(|| "–".into());
        let mut meta = format!(
            "{} · {} cases × {} trials · {}/{} passed",
            s.tag(),
            s.cases,
            s.trials,
            s.passed,
            s.attempted
        );
        if s.skipped > 0 {
            meta.push_str(&format!(" · {} skipped ({})", s.skipped, s.skip_reason));
        }
        let metrics: Vec<AnyElement> = s
            .metrics
            .iter()
            .map(|m| self.eval_metric_row(&s.id, m, p))
            .collect();
        let mut card = div()
            .flex()
            .flex_col()
            .rounded_xl()
            .border_1()
            .border_color(p.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_3()
                    .py_2p5()
                    .border_b_1()
                    .border_color(p.border)
                    .child(kind_tile(s.kind, p))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(SharedString::from(s.title.clone())),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(p.faint)
                                    .child(SharedString::from(meta)),
                            ),
                    )
                    .child(
                        div()
                            .text_base()
                            .font_weight(FontWeight::MEDIUM)
                            .child(SharedString::from(head)),
                    ),
            )
            .child(
                div()
                    .px_3()
                    .pt_1()
                    .text_xs()
                    .text_color(p.faint)
                    .child(SharedString::from(format!("Origin: {}", s.origin))),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .px_3()
                    .py_2()
                    .gap_1()
                    .children(metrics),
            );
        if !s.failures.is_empty() {
            card = card.child(
                div().px_3().pb_2().child(
                    Button::new(SharedString::from(format!("eval-fail-{}", s.id)))
                        .icon(AssetIcon::TriangleAlert)
                        .label(SharedString::from(format!(
                            "{} {} failure{}",
                            if open { "Hide" } else { "Show" },
                            s.failures.len(),
                            if s.failures.len() == 1 { "" } else { "s" }
                        )))
                        .ghost()
                        .small()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.model.eval.toggle(&sid);
                            cx.notify();
                        })),
                ),
            );
            if open {
                let rows: Vec<AnyElement> = s
                    .failures
                    .iter()
                    .take(60)
                    .map(|f| {
                        div()
                            .flex()
                            .flex_col()
                            .px_3()
                            .py_1p5()
                            .border_t_1()
                            .border_color(p.border)
                            .child(div().text_xs().font_weight(FontWeight::MEDIUM).child(
                                SharedString::from(format!(
                                    "{} · trial {} · expected {} · got {}",
                                    f.case, f.trial, f.expected, f.got
                                )),
                            ))
                            .when(!f.why.is_empty(), |d| {
                                d.child(
                                    div()
                                        .text_xs()
                                        .text_color(p.muted)
                                        .child(SharedString::from(f.why.clone())),
                                )
                            })
                            .into_any_element()
                    })
                    .collect();
                card = card.children(rows);
                if s.failures.len() > 60 {
                    card = card.child(div().px_3().py_1p5().text_xs().text_color(p.faint).child(
                        SharedString::from(format!(
                            "{} more in the saved report (tern eval report {}).",
                            s.failures.len() - 60,
                            st.selected_id().unwrap_or("last")
                        )),
                    ));
                }
            }
        }
        card.into_any_element()
    }

    fn eval_metric_row(&self, suite: &str, m: &Metric, p: Palette) -> AnyElement {
        let change = self.model.eval.change(suite, &m.key);
        let delta: AnyElement = match change {
            Some((c @ (Change::Improved | Change::Regressed), old)) => div()
                .text_xs()
                .text_color(if c == Change::Regressed {
                    p.text
                } else {
                    p.muted
                })
                .child(SharedString::from(format!("{} was {old}", c.glyph())))
                .into_any_element(),
            Some((Change::Same, _)) => div()
                .text_xs()
                .text_color(p.faint)
                .child("= same")
                .into_any_element(),
            Some((Change::New, _)) => div()
                .text_xs()
                .text_color(p.faint)
                .child("+ new")
                .into_any_element(),
            _ => div().into_any_element(),
        };
        div()
            .flex()
            .items_center()
            .gap_3()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_xs()
                    .text_color(p.muted)
                    .truncate()
                    .child(SharedString::from(if m.note.is_empty() {
                        m.label.clone()
                    } else {
                        format!("{} ({})", m.label, m.note)
                    })),
            )
            .child(div().text_xs().child(SharedString::from(m.line())))
            .child(div().w(px(110.)).flex().justify_end().child(delta))
            .into_any_element()
    }
}
