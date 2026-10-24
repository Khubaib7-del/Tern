//! The Git screen: the vault as a repository. Checkpoints (commits),
//! branches, pull and push, a pull request on GitHub, baseline files, and
//! who commits. The title-bar chip shows the branch and pending changes.

use gpui_kit::assets::IconName as AssetIcon;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{Disableable as _, Selectable as _, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, Entity, FontWeight, SharedString, Window, div, px};

use super::Workspace;
use crate::git::GitOp;
use crate::model::Screen;
use crate::theme::Palette;

/// The Git screen's text fields.
pub(super) struct GitInputs {
    pub message: Entity<InputState>,
    pub branch: Entity<InputState>,
    pub remote: Entity<InputState>,
    pub name: Entity<InputState>,
    pub email: Entity<InputState>,
}

impl GitInputs {
    pub fn new(window: &mut Window, cx: &mut Context<Workspace>) -> Self {
        let mut field = |ph: &'static str| cx.new(|cx| InputState::new(window, cx).placeholder(ph));
        Self {
            message: field("What changed, like \"Added chapter 2 sources\""),
            branch: field("Branch name, like chapter-2"),
            remote: field("https://github.com/you/thesis.git"),
            name: field("Your name"),
            email: field("you@example.com"),
        }
    }
}

impl Workspace {
    pub(super) fn git_op(&mut self, op: GitOp, cx: &mut Context<Self>) {
        let Some(dir) = self
            .model
            .editor
            .vault
            .as_ref()
            .map(|v| v.root().to_path_buf())
        else {
            self.model.git.error = Some("The vault is not open.".into());
            cx.notify();
            return;
        };
        if self.model.git.start(&op) {
            self.bridge.git(dir, op);
        }
        cx.notify();
    }

    pub(super) fn open_git(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.save_note(cx);
        self.model.screen = Screen::Git;
        let prefs = tern_agent::Preferences::load();
        self.model.git_credit = prefs.git_credit_tern;
        let (name, email) = (
            prefs.git_name.unwrap_or_default(),
            prefs.git_email.unwrap_or_default(),
        );
        self.git_inputs
            .name
            .update(cx, |i, cx| i.set_value(name, window, cx));
        self.git_inputs
            .email
            .update(cx, |i, cx| i.set_value(email, window, cx));
        let remote = self
            .model
            .git
            .status()
            .and_then(|s| s.remote.clone())
            .unwrap_or_default();
        self.git_inputs
            .remote
            .update(cx, |i, cx| i.set_value(remote, window, cx));
        self.git_op(GitOp::Refresh, cx);
    }

    pub(super) fn close_git(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.model.screen = Screen::Chat;
        self.composer.update(cx, |c, cx| c.focus(window, cx));
        cx.notify();
    }

    fn take_input(
        &self,
        input: &Entity<InputState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> String {
        let v = input.read(cx).value().to_string();
        input.update(cx, |i, cx| i.set_value("", window, cx));
        v
    }

    fn save_identity(&mut self, cx: &mut Context<Self>) {
        let mut prefs = tern_agent::Preferences::load();
        let read = |e: &Entity<InputState>, cx: &mut Context<Self>| {
            Some(e.read(cx).value().trim().to_owned()).filter(|s| !s.is_empty())
        };
        prefs.git_name = read(&self.git_inputs.name, cx);
        prefs.git_email = read(&self.git_inputs.email, cx);
        prefs.git_credit_tern = self.model.git_credit;
        self.bridge.save_prefs(prefs);
        self.model.git.note = Some("Saved who commits checkpoints.".into());
        cx.notify();
    }

    /// The branch and change count in the title bar, when the vault is a
    /// repository.
    pub(super) fn git_chip(&self, p: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let label = self.model.git.chip()?;
        Some(
            div()
                .id("git-chip")
                .occlude()
                .flex()
                .items_center()
                .gap_1()
                .px_2()
                .py(px(2.))
                .rounded_md()
                .text_xs()
                .text_color(p.muted)
                .cursor_pointer()
                .hover(move |d| d.bg(p.hover).text_color(p.text))
                .child(gpui_kit::component::Icon::new(AssetIcon::GitBranch).size_3())
                .child(SharedString::from(label))
                .on_click(cx.listener(|this, _, window, cx| this.open_git(window, cx)))
                .into_any_element(),
        )
    }

    pub(super) fn git_screen(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let g = &self.model.git;
        let busy = g.busy.is_some();
        let section = |title: &'static str| {
            div()
                .pt_5()
                .pb_2()
                .text_xs()
                .text_color(p.faint)
                .child(title)
        };
        let row = || div().flex().items_center().gap_2();
        let vault = self
            .model
            .editor
            .vault
            .as_ref()
            .map(|v| v.root().display().to_string())
            .unwrap_or_default();

        let header = row()
            .justify_between()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Git"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(p.faint)
                            .child(SharedString::from(vault)),
                    ),
            )
            .child(
                Button::new("git-refresh")
                    .icon(AssetIcon::RotateCw)
                    .ghost()
                    .small()
                    .tooltip("Check again")
                    .disabled(busy)
                    .on_click(cx.listener(|this, _, _, cx| this.git_op(GitOp::Refresh, cx))),
            );

        let line = match (g.busy, &g.error, &g.note) {
            (Some(b), _, _) => Some(
                row()
                    .text_sm()
                    .text_color(p.muted)
                    .child(Spinner::new().small())
                    .child(b)
                    .into_any_element(),
            ),
            (None, Some(e), _) => Some(
                div()
                    .text_sm()
                    .text_color(p.text)
                    .child(SharedString::from(e.clone()))
                    .into_any_element(),
            ),
            (None, None, Some(n)) => Some(
                div()
                    .text_sm()
                    .text_color(p.muted)
                    .child(SharedString::from(n.clone()))
                    .into_any_element(),
            ),
            _ => None,
        };

        let mut body = div().flex().flex_col();
        match &g.snapshot {
            None => {}
            Some(s) if !s.available => {
                body = body.child(
                    div()
                        .pt_4()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .text_sm()
                        .child("Git isn't installed on this computer. Tern uses your own git, so pushes use your GitHub sign-in.")
                        .child(
                            Button::new("git-install")
                                .label("Get Git")
                                .small()
                                .on_click(|_, _, cx| cx.open_url("https://git-scm.com/downloads")),
                        ),
                );
            }
            Some(s) if !s.status.is_repo => {
                body = body.child(
                    div()
                        .pt_4()
                        .flex()
                        .flex_col()
                        .items_start()
                        .gap_2()
                        .text_sm()
                        .child("Make the vault a repository to save checkpoints of your notes, work on branches, and push them to GitHub.")
                        .child(
                            Button::new("git-init")
                                .label("Make the vault a repository")
                                .primary()
                                .small()
                                .disabled(busy)
                                .on_click(cx.listener(|this, _, _, cx| this.git_op(GitOp::Init, cx))),
                        ),
                );
            }
            Some(s) => {
                let st = &s.status;
                let mut facts = vec![format!(
                    "On {}",
                    st.branch.clone().unwrap_or_else(|| "no branch".into())
                )];
                if st.ahead > 0 {
                    facts.push(format!("{} to push", st.ahead));
                }
                if st.behind > 0 {
                    facts.push(format!("{} to pull", st.behind));
                }
                facts.push(match &st.remote {
                    Some(r) => r.clone(),
                    None => "no remote yet".into(),
                });
                body = body.child(
                    div()
                        .pt_3()
                        .text_sm()
                        .text_color(p.muted)
                        .child(SharedString::from(facts.join(" · "))),
                );

                // Changes and checkpoint.
                body = body.child(section("Changes"));
                if st.changes.is_empty() {
                    body = body.child(
                        div()
                            .text_sm()
                            .text_color(p.muted)
                            .child("No changes since the last checkpoint."),
                    );
                } else {
                    body = body.children(st.changes.iter().take(60).map(|c| {
                        row()
                            .text_sm()
                            .child(
                                div()
                                    .w(px(72.))
                                    .flex_none()
                                    .text_xs()
                                    .text_color(p.faint)
                                    .child(c.kind()),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .child(SharedString::from(c.path.clone())),
                            )
                    }));
                    if st.changes.len() > 60 {
                        body = body.child(div().text_xs().text_color(p.faint).child(
                            SharedString::from(format!("and {} more", st.changes.len() - 60)),
                        ));
                    }
                }
                body = body.child(
                    row()
                        .pt_2()
                        .child(
                            div()
                                .flex_1()
                                .child(Input::new(&self.git_inputs.message).small()),
                        )
                        .child(
                            Button::new("git-commit")
                                .label("Save checkpoint")
                                .primary()
                                .small()
                                .disabled(busy || st.changes.is_empty())
                                .on_click(cx.listener(|this, _, window, cx| {
                                    let m = this.take_input(
                                        &this.git_inputs.message.clone(),
                                        window,
                                        cx,
                                    );
                                    this.git_op(GitOp::Checkpoint(m), cx);
                                })),
                        ),
                );

                // Branches.
                body = body.child(section("Branches"));
                let current = st.branch.clone();
                body = body.child(div().flex().flex_wrap().gap_1().children(
                    s.branches.iter().enumerate().map(|(ix, b)| {
                        let name = b.clone();
                        Button::new(("git-branch", ix))
                            .label(SharedString::from(b.clone()))
                            .ghost()
                            .xsmall()
                            .selected(current.as_deref() == Some(b.as_str()))
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.git_op(GitOp::Switch(name.clone()), cx)
                            }))
                    }),
                ));
                body = body.child(
                    row()
                        .pt_2()
                        .child(
                            div()
                                .flex_1()
                                .child(Input::new(&self.git_inputs.branch).small()),
                        )
                        .child(
                            Button::new("git-switch")
                                .label("Switch or create")
                                .small()
                                .disabled(busy)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    let b = this.take_input(
                                        &this.git_inputs.branch.clone(),
                                        window,
                                        cx,
                                    );
                                    this.git_op(GitOp::Switch(b), cx);
                                })),
                        ),
                );

                // GitHub.
                body = body.child(section("GitHub"));
                let pr = g.pull_request_url();
                let page = st.github().map(|(o, r)| tern_agent::git::repo_url(&o, &r));
                body = body.child(
                    row()
                        .flex_wrap()
                        .child(
                            Button::new("git-pull")
                                .label("Pull")
                                .icon(AssetIcon::ArrowDown)
                                .small()
                                .disabled(busy || st.remote.is_none())
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.git_op(GitOp::Pull, cx)),
                                ),
                        )
                        .child(
                            Button::new("git-push")
                                .label("Push")
                                .icon(AssetIcon::ArrowUp)
                                .small()
                                .disabled(busy || st.remote.is_none())
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.git_op(GitOp::Push, cx)),
                                ),
                        )
                        .when_some(pr, |d, url| {
                            d.child(
                                Button::new("git-pr")
                                    .label("Open pull request")
                                    .icon(AssetIcon::GitPullRequest)
                                    .small()
                                    .on_click(move |_, _, cx| cx.open_url(&url)),
                            )
                        })
                        .when_some(page, |d, url| {
                            d.child(
                                Button::new("git-page")
                                    .label("Open on GitHub")
                                    .icon(AssetIcon::Github)
                                    .ghost()
                                    .small()
                                    .on_click(move |_, _, cx| cx.open_url(&url)),
                            )
                        }),
                );
                body = body.child(
                    row()
                        .pt_2()
                        .child(
                            div()
                                .flex_1()
                                .child(Input::new(&self.git_inputs.remote).small()),
                        )
                        .child(
                            Button::new("git-remote")
                                .label("Save remote")
                                .small()
                                .disabled(busy)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    let u = this.git_inputs.remote.read(cx).value().to_string();
                                    this.git_op(GitOp::SetRemote(u), cx);
                                })),
                        ),
                );
                body = body.child(
                    div()
                        .pt_1()
                        .text_xs()
                        .text_color(p.faint)
                        .child("Make an empty repository on GitHub, paste its address, then Push. Your own git sign-in is used."),
                );

                // Baselines.
                body = body.child(section("Baseline files")).child(
                    row()
                        .child(
                            Button::new("git-baselines")
                                .label("Add README, LICENSE, SECURITY, CONTRIBUTING")
                                .small()
                                .disabled(busy)
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.git_op(GitOp::Baselines, cx)),
                                ),
                        )
                        .child(div().text_xs().text_color(p.faint).child(
                            "Only missing files are added. LICENSE is all rights reserved.",
                        )),
                );

                // Recent checkpoints.
                if !s.log.is_empty() {
                    body = body.child(section("Recent checkpoints"));
                    body = body.children(s.log.iter().map(|c| {
                        row()
                            .text_sm()
                            .child(
                                div()
                                    .w(px(64.))
                                    .flex_none()
                                    .text_xs()
                                    .text_color(p.faint)
                                    .child(SharedString::from(c.hash.clone())),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .child(SharedString::from(c.subject.clone())),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_xs()
                                    .text_color(p.faint)
                                    .child(SharedString::from(c.when.clone())),
                            )
                    }));
                }
            }
        }

        // Who commits: shown whenever git is there.
        let identity = g.snapshot.as_ref().filter(|s| s.available).map(|_| {
            let credit = self.model.git_credit;
            div()
                .flex()
                .flex_col()
                .child(section("Commit as"))
                .child(
                    row()
                        .child(
                            div()
                                .flex_1()
                                .child(Input::new(&self.git_inputs.name).small()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .child(Input::new(&self.git_inputs.email).small()),
                        ),
                )
                .child(
                    row()
                        .pt_2()
                        .child(
                            Button::new("git-credit")
                                .label("Credit Tern as co-author")
                                .icon(if credit {
                                    AssetIcon::SquareCheck
                                } else {
                                    AssetIcon::Square
                                })
                                .ghost()
                                .small()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.model.git_credit = !this.model.git_credit;
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("git-identity")
                                .label("Save")
                                .small()
                                .on_click(cx.listener(|this, _, _, cx| this.save_identity(cx))),
                        ),
                )
                .child(
                    div()
                        .pt_1()
                        .text_xs()
                        .text_color(p.faint)
                        .child("Leave these empty to use your git settings."),
                )
        });

        div()
            .id("git-screen")
            .flex_1()
            .min_h_0()
            .overflow_y_scrollbar()
            .child(
                div()
                    .mx_auto()
                    .w_full()
                    .max_w(px(720.))
                    .px_6()
                    .py_6()
                    .flex()
                    .flex_col()
                    .child(header)
                    .children(line.map(|l| div().pt_3().child(l)))
                    .child(body)
                    .children(identity),
            )
            .into_any_element()
    }
}
