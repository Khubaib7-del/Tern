//! Settings › Local models: a section per task, a row per model with its
//! logo, name, size, licence and runtime, Download (a progress bar and
//! Cancel while it runs), an Enable switch, a Default radio per task, and
//! Delete. Rows are culled off screen like Providers (`crate::cull`) and
//! the buttons are the light `pill` capsules. The state is
//! `crate::localmodels`; the downloads run on the bridge.

use gpui_kit::component::Sizable as _;
use gpui_kit::component::switch::Switch;
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, FontWeight, SharedString, div, px};
use tern_agent::local_models::{self, ModelStatus, State, Task};

use super::{PillLook, Workspace, brand_logo, pill};
use crate::localmodels::{LocalModelsPage, logo_key};
use crate::settings::Section;
use crate::theme::Palette;

impl Workspace {
    /// Opens the page and reads the models folder.
    pub(super) fn open_local_models(&mut self) {
        self.show_settings(Section::LocalModels);
        self.reload_local_models();
    }

    /// Reads the models' state again (after a download or delete).
    pub(super) fn reload_local_models(&self) {
        self.bridge
            .local_models_load(self.model.providers.prefs.local_models.clone());
    }

    pub(super) fn local_models_page(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let page = &self.model.settings.local;
        let mut body: Vec<AnyElement> = Vec::new();
        if let Some(w) = &page.warning {
            body.push(
                div()
                    .text_sm()
                    .text_color(p.warm)
                    .child(SharedString::from(w.clone()))
                    .into_any_element(),
            );
        }
        match &page.statuses {
            None => body.push(
                div()
                    .text_sm()
                    .text_color(p.muted)
                    .child("Reading the models folder…")
                    .into_any_element(),
            ),
            Some(_) => {
                let handle = self.settings_page_scroll();
                for task in Task::ALL {
                    let models: Vec<ModelStatus> =
                        page.for_task(task).into_iter().cloned().collect();
                    let rows: Vec<AnyElement> = if models.is_empty() {
                        vec![
                            div()
                                .px_3()
                                .py_2p5()
                                .text_sm()
                                .text_color(p.faint)
                                .child("No models listed for this yet.")
                                .into_any_element(),
                        ]
                    } else {
                        models
                            .into_iter()
                            .map(|s| {
                                let key = format!("local-model-{}", s.entry.id);
                                self.culled(key, &handle, || self.local_model_row(s, p, cx))
                            })
                            .collect()
                    };
                    body.push(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(Self::settings_card(task.label(), p, rows))
                            .child(div().text_xs().text_color(p.faint).child(task.blurb()))
                            .into_any_element(),
                    );
                }
            }
        }
        body.push(
            div()
                .text_xs()
                .text_color(p.faint)
                .child(SharedString::from(format!(
                    "Downloaded from Hugging Face only when you ask, checked against each file's SHA-256, and kept in {}. Entries marked placeholder are stand-ins until Tern's model picks are final; add or replace entries with {} in the settings folder.",
                    page.root.as_deref().unwrap_or("Tern's data folder"),
                    local_models::OVERRIDES
                )))
                .into_any_element(),
        );
        self.page(Section::LocalModels, p, None, body)
    }

    fn local_model_row(&self, s: ModelStatus, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let page = &self.model.settings.local;
        let id = s.entry.id.clone();
        let bid = |what: &str| SharedString::from(format!("local-{what}-{id}"));
        let dl = page.downloads.get(&id).cloned();
        let error = page.errors.get(&id).cloned();
        let runnable = local_models::runtime_available(s.entry.runtime);

        let logo = div()
            .size(px(28.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded_lg()
            .border_1()
            .border_color(p.border)
            .bg(p.raised)
            .child(brand_logo(&logo_key(&s), 16., p));

        let mut info = div()
            .flex()
            .flex_col()
            .min_w_0()
            .flex_1()
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::MEDIUM)
                    .child(SharedString::from(s.entry.name.clone())),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(p.muted)
                    .child(SharedString::from(LocalModelsPage::detail(&s))),
            );
        if !s.entry.note.is_empty() {
            info = info.child(
                div()
                    .text_xs()
                    .text_color(p.faint)
                    .child(SharedString::from(s.entry.note.clone())),
            );
        }
        if s.installed() && !runnable {
            info = info.child(
                div()
                    .text_xs()
                    .text_color(p.faint)
                    .child(SharedString::from(format!(
                        "Downloaded. This build cannot run {} models yet.",
                        s.entry.runtime.label()
                    ))),
            );
        }
        if let Some(e) = error {
            info = info.child(
                div()
                    .text_xs()
                    .text_color(p.warm)
                    .child(SharedString::from(e)),
            );
        }

        let mut actions = div().flex().items_center().gap_2().flex_shrink_0();
        if let Some(d) = &dl {
            let cid = id.clone();
            actions = actions.child(pill(
                bid("cancel"),
                "Cancel",
                PillLook::Ghost,
                d.cancel.is_cancelled(),
                None,
                p,
                cx.listener(move |this, _, _, cx| {
                    this.model.settings.local.cancel(&cid);
                    cx.notify();
                }),
            ));
        } else if s.installed() {
            let sid = id.clone();
            actions = actions
                .child(
                    Switch::new(bid("on"))
                        .checked(s.enabled)
                        .small()
                        .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                            let on = *checked;
                            let id = sid.clone();
                            this.save_ui_prefs(move |p| p.local_models.set_enabled(&id, on));
                            this.reload_local_models();
                            cx.notify();
                        })),
                )
                .child(self.default_radio(&s, p, cx));
        } else {
            let did = id.clone();
            actions = actions.child(pill(
                bid("download"),
                LocalModelsPage::download_label(&s),
                PillLook::Solid,
                false,
                Some(SharedString::from(format!(
                    "Download {} from huggingface.co/{}",
                    local_models::human_size(s.entry.size()),
                    s.entry.hf_repo
                ))),
                p,
                cx.listener(move |this, _, _, cx| {
                    this.start_local_download(&did);
                    cx.notify();
                }),
            ));
        }
        if dl.is_none() && s.state != State::NotInstalled {
            let armed = page.confirm_delete.as_deref() == Some(id.as_str());
            let rid = id.clone();
            actions = actions.child(pill(
                bid("delete"),
                if armed { "Delete?" } else { "Delete" },
                PillLook::Ghost,
                false,
                Some(SharedString::from(if armed {
                    "Click again to delete the files"
                } else {
                    "Delete the downloaded files"
                })),
                p,
                cx.listener(move |this, _, _, cx| {
                    if this.model.settings.local.arm_delete(&rid) {
                        this.bridge.local_model_remove(rid.clone());
                    }
                    cx.notify();
                }),
            ));
        }

        let mut row = div()
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
                    .items_center()
                    .gap_3()
                    .child(logo)
                    .child(info)
                    .child(actions),
            );
        if let Some(d) = dl {
            let frac = d.fraction();
            row = row.child(
                div()
                    .pl(px(40.))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div().h(px(4.)).w_full().rounded_full().bg(p.border).child(
                            div()
                                .h_full()
                                .w(gpui_kit::relative(frac))
                                .rounded_full()
                                .bg(p.accent),
                        ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(p.muted)
                            .child(SharedString::from(d.line())),
                    ),
            );
        }
        row.into_any_element()
    }

    /// The task's default as a radio: one model per task, and a click on
    /// the chosen one clears it.
    fn default_radio(&self, s: &ModelStatus, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let id = s.entry.id.clone();
        let task = s.entry.task;
        let on = s.is_default;
        div()
            .id(SharedString::from(format!("local-default-{id}")))
            .flex()
            .items_center()
            .gap_1()
            .px_1()
            .rounded_md()
            .hover(|st| st.bg(p.hover))
            .child(
                div()
                    .size(px(14.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .border_1()
                    .border_color(if on { p.accent } else { p.muted })
                    .when(on, |d| {
                        d.child(div().size(px(6.)).rounded_full().bg(p.accent))
                    }),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(if on { p.text } else { p.muted })
                    .child("Default"),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                let id = id.clone();
                this.save_ui_prefs(move |p| {
                    let pick = (p.local_models.default_for(task) != Some(id.as_str()))
                        .then_some(id.as_str());
                    p.local_models.set_default(task, pick);
                });
                this.reload_local_models();
                cx.notify();
            }))
            .into_any_element()
    }

    /// Starts downloading model `id` (one download per model).
    pub(super) fn start_local_download(&mut self, id: &str) {
        if let Some(cancel) = self.model.settings.local.start(id) {
            self.bridge.local_model_download(id.to_owned(), cancel);
        }
    }
}
