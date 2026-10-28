//! Settings › Permissions and the composer's approval-mode chip (#75).
//!
//! The approval mode (Read-only, Ask, Workspace write, Full access), the
//! permission rules (allow, ask or deny by web domain, path or tool, with
//! add and remove), the workbench's programs (the built-in list and the
//! user's own) and its network switch. The rules themselves are
//! `tern_agent::permissions`; the sandbox is `tern_agent::sandbox`.

use std::rc::Rc;

use gpui_kit::assets::IconName as AssetIcon;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::switch::Switch;
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, FontWeight, SharedString, Window, div, px};
use tern_agent::permissions::{ApprovalMode, PermissionRule, RuleKind};

use super::Workspace;
use super::press::Capsule as _;
use super::segmented::{Look, OnPick, Segment};
use crate::composer::Menu;
use crate::settings::Section;
use crate::theme::Palette;

/// The actions a new rule can take, in the picker's order.
const ACTIONS: [tern_agent::Mode; 3] = [
    tern_agent::Mode::Allow,
    tern_agent::Mode::Ask,
    tern_agent::Mode::Deny,
];

/// A mode's icon on the chip and in the menu.
pub(super) fn mode_icon(m: ApprovalMode) -> AssetIcon {
    match m {
        ApprovalMode::ReadOnly => AssetIcon::Eye,
        ApprovalMode::Ask => AssetIcon::ShieldQuestionMark,
        ApprovalMode::WorkspaceWrite => AssetIcon::FolderPen,
        ApprovalMode::Full => AssetIcon::ShieldOff,
    }
}

fn action_label(m: tern_agent::Mode) -> &'static str {
    match m {
        tern_agent::Mode::Allow => "Allow",
        tern_agent::Mode::Ask => "Ask",
        tern_agent::Mode::Deny => "Deny",
    }
}

/// The placeholder for a new rule's pattern, by kind.
pub(super) fn pattern_hint(k: RuleKind) -> &'static str {
    match k {
        RuleKind::Url => "arxiv.org or *.edu",
        RuleKind::Path => "./data/** or ~/Documents/**",
        RuleKind::Tool => "web_search, mcp__github__* or run_command pandoc *",
    }
}

impl Workspace {
    /// Sets the approval mode for the next turn and keeps it.
    pub(super) fn set_approval_mode(&mut self, m: ApprovalMode) {
        let id = m.id().to_owned();
        self.save_ui_prefs(move |p| p.approval_mode = Some(id.clone()));
        tern_agent::diag::note(&format!("permissions: approval mode {}", m.id()));
    }

    /// Adds the rule typed in Settings › Permissions.
    pub(super) fn add_permission_rule(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let pattern = self
            .settings_inputs
            .rule_pattern
            .read(cx)
            .value()
            .trim()
            .to_string();
        if pattern.is_empty() {
            return;
        }
        let kind = RuleKind::ALL[self.work.rule_kind.min(RuleKind::ALL.len() - 1)];
        let action = ACTIONS[self.work.rule_action.min(ACTIONS.len() - 1)];
        let rule = PermissionRule::new(kind, pattern, action);
        let line = rule.line();
        self.save_ui_prefs(move |p| {
            if !p.permission_rules.contains(&rule) {
                p.permission_rules.push(rule.clone());
            }
        });
        tern_agent::diag::note(&format!("permissions: added rule {line}"));
        self.settings_inputs
            .rule_pattern
            .update(cx, |i, cx| i.set_value("", window, cx));
        cx.notify();
    }

    /// Adds the program typed under Workbench programs.
    pub(super) fn add_workbench_program(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self
            .settings_inputs
            .program
            .read(cx)
            .value()
            .trim()
            .to_string();
        if name.is_empty() || name.contains(char::is_whitespace) {
            return;
        }
        self.save_ui_prefs(move |p| {
            if !p
                .workbench_programs
                .iter()
                .any(|x| x.eq_ignore_ascii_case(&name))
            {
                p.workbench_programs.push(name.clone());
            }
        });
        tern_agent::workbench::set_settings(self.model.providers.prefs.workbench());
        self.settings_inputs
            .program
            .update(cx, |i, cx| i.set_value("", window, cx));
        cx.notify();
    }

    /// A rule's logo: the site's icon for a domain, a folder for a path,
    /// the program's logo for a command, the server's mark for MCP.
    fn rule_logo(&self, r: &PermissionRule, p: Palette) -> AnyElement {
        let pat = r.pattern.trim();
        match r.kind {
            RuleKind::Url => {
                let host = pat.trim_start_matches("*.").to_owned();
                let page = tern_agent::PageRef {
                    title: String::new(),
                    url: format!("https://{host}/"),
                    site: host,
                    builtin: None,
                    favicon: None,
                };
                self.site_icon(&page, 14., p)
            }
            RuleKind::Path => crate::fileicon::folder_logo(pat, 14.),
            RuleKind::Tool => {
                let mut words = pat.split_whitespace();
                let tool = words.next().unwrap_or_default();
                if tool == "run_command" {
                    let rest: Vec<&str> = words.collect();
                    crate::fileicon::logo(
                        crate::fileicon::FileIcon::of(crate::toolcards::program_logo_name(
                            &rest.join(" "),
                        )),
                        14.,
                        p.text.l > 0.5,
                    )
                } else if let Some(server) = tool.strip_prefix("mcp__") {
                    super::brand_logo(server.split("__").next().unwrap_or(server), 14., p)
                } else if tool == "web_search" {
                    super::service_logo(
                        self.model
                            .providers
                            .prefs
                            .search
                            .as_deref()
                            .unwrap_or("wikipedia"),
                        14.,
                        p,
                        p.text,
                    )
                } else {
                    div()
                        .flex_none()
                        .text_color(p.muted)
                        .child(gpui_kit::component::Icon::new(AssetIcon::Wrench).size(px(14.)))
                        .into_any_element()
                }
            }
        }
    }

    /// Settings › Permissions.
    pub(super) fn permissions_page(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let prefs = &self.model.providers.prefs;
        let mode = prefs.approval();

        // ----- Approval mode -----
        let options: Vec<Segment> = ApprovalMode::ALL
            .iter()
            .map(|m| Segment::text(m.label()))
            .collect();
        let selected = ApprovalMode::ALL
            .iter()
            .position(|m| *m == mode)
            .unwrap_or(1);
        let pick: OnPick = Rc::new(
            move |this: &mut Workspace, i: usize, _: &mut Window, cx: &mut Context<Workspace>| {
                this.set_approval_mode(ApprovalMode::ALL[i.min(3)]);
                cx.notify();
            },
        );
        let mode_card = Self::settings_card(
            "Approval mode",
            p,
            vec![
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .px_3()
                    .py_2p5()
                    .child(div().flex().flex_wrap().child(self.segmented(
                        "perm-mode",
                        &options,
                        selected,
                        Look::default(),
                        p,
                        pick,
                        cx,
                    )))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_sm()
                            .text_color(p.muted)
                            .child(
                                gpui_kit::component::Icon::new(mode_icon(mode))
                                    .small()
                                    .text_color(p.muted),
                            )
                            .child(SharedString::from(mode.line())),
                    )
                    .child(div().text_xs().text_color(p.faint).child(
                        "Web addresses that carry a secret, or that only a page mentioned, still ask in every mode. The composer's shield chip changes this too.",
                    ))
                    .into_any_element(),
            ],
        );

        // ----- Rules -----
        let mut rule_rows: Vec<AnyElement> = Vec::new();
        for (i, r) in prefs.permission_rules.iter().enumerate() {
            let remove = r.clone();
            let ink = match r.action {
                tern_agent::Mode::Allow => p.text,
                tern_agent::Mode::Ask => super::work_view::approval_yellow(p),
                tern_agent::Mode::Deny => {
                    gpui_kit::hsla(0.0, 0.65, if p.canvas.l < 0.5 { 0.62 } else { 0.45 }, 1.)
                }
            };
            rule_rows.push(
                div()
                    .id(SharedString::from(format!("perm-rule-{i}")))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(p.border)
                    .child(self.rule_logo(r, p))
                    .child(
                        div()
                            .flex_none()
                            .w(px(46.))
                            .text_xs()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(ink)
                            .child(action_label(r.action)),
                    )
                    .child(
                        div()
                            .flex_none()
                            .w(px(110.))
                            .text_xs()
                            .text_color(p.faint)
                            .child(r.kind.label()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_family(crate::assets::MONO_FONT)
                            .text_xs()
                            .child(SharedString::from(r.pattern.clone())),
                    )
                    .child(
                        Button::new(SharedString::from(format!("perm-rule-x-{i}")))
                            .capsule()
                            .icon(AssetIcon::X)
                            .ghost()
                            .xsmall()
                            .tooltip("Remove this rule")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let r = remove.clone();
                                this.save_ui_prefs(move |p| p.permission_rules.retain(|x| *x != r));
                                cx.notify();
                            })),
                    )
                    .into_any_element(),
            );
        }
        if rule_rows.is_empty() {
            rule_rows.push(
                div()
                    .px_3()
                    .py_2p5()
                    .border_b_1()
                    .border_color(p.border)
                    .text_sm()
                    .text_color(p.faint)
                    .child("No rules yet. \"Always allow\" on an approval card adds one here.")
                    .into_any_element(),
            );
        }
        let kinds: Vec<Segment> = RuleKind::ALL
            .iter()
            .map(|k| Segment::text(k.label()))
            .collect();
        let actions: Vec<Segment> = ACTIONS
            .iter()
            .map(|a| Segment::text(action_label(*a)))
            .collect();
        let pick_kind: OnPick = Rc::new(
            move |this: &mut Workspace,
                  i: usize,
                  window: &mut Window,
                  cx: &mut Context<Workspace>| {
                this.work.rule_kind = i;
                let hint = pattern_hint(RuleKind::ALL[i.min(RuleKind::ALL.len() - 1)]);
                this.settings_inputs.rule_pattern.update(cx, |s, cx| {
                    s.set_placeholder(hint, window, cx);
                });
                cx.notify();
            },
        );
        let pick_action: OnPick = Rc::new(
            move |this: &mut Workspace, i: usize, _: &mut Window, cx: &mut Context<Workspace>| {
                this.work.rule_action = i;
                cx.notify();
            },
        );
        rule_rows.push(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .px_3()
                .py_2p5()
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .child(self.segmented(
                            "perm-new-kind",
                            &kinds,
                            self.work.rule_kind,
                            Look::default(),
                            p,
                            pick_kind,
                            cx,
                        ))
                        .child(self.segmented(
                            "perm-new-action",
                            &actions,
                            self.work.rule_action,
                            Look::default(),
                            p,
                            pick_action,
                            cx,
                        )),
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
                                .child(Input::new(&self.settings_inputs.rule_pattern).small()),
                        )
                        .child(
                            Button::new("perm-rule-add")
                                .capsule()
                                .icon(AssetIcon::Plus)
                                .label("Add rule")
                                .small()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.add_permission_rule(window, cx)
                                })),
                        ),
                )
                .into_any_element(),
        );
        let rules_card = Self::settings_card("Rules", p, rule_rows);

        // ----- Workbench -----
        let mut chips: Vec<AnyElement> = Vec::new();
        let dark = p.text.l > 0.5;
        let mut chip = |id: SharedString, name: &str, removable: bool| {
            let n = name.to_owned();
            div()
                .id(id.clone())
                .flex()
                .items_center()
                .gap_1()
                .px_1p5()
                .py(px(2.))
                .rounded_md()
                .border_1()
                .border_color(p.border)
                .text_xs()
                .child(crate::fileicon::logo(
                    crate::fileicon::FileIcon::of(crate::toolcards::program_logo_name(name)),
                    12.,
                    dark,
                ))
                .child(SharedString::from(n.clone()))
                .when(removable, |d| {
                    d.child(
                        Button::new(SharedString::from(format!("{id}-x")))
                            .capsule()
                            .icon(AssetIcon::X)
                            .ghost()
                            .xsmall()
                            .tooltip("Take it off the list")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let n = n.clone();
                                this.save_ui_prefs(move |p| {
                                    p.workbench_programs.retain(|x| !x.eq_ignore_ascii_case(&n))
                                });
                                tern_agent::workbench::set_settings(
                                    this.model.providers.prefs.workbench(),
                                );
                                cx.notify();
                            })),
                    )
                })
                .into_any_element()
        };
        for &name in tern_agent::sandbox::DEFAULT_PROGRAMS {
            chips.push(chip(
                SharedString::from(format!("perm-prog-{name}")),
                name,
                false,
            ));
        }
        for name in &prefs.workbench_programs {
            chips.push(chip(
                SharedString::from(format!("perm-own-{name}")),
                name.as_str(),
                true,
            ));
        }
        let network = prefs.workbench_network;
        let workbench_card = Self::settings_card(
            "Workbench",
            p,
            vec![
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .px_3()
                    .py_2p5()
                    .border_b_1()
                    .border_color(p.border)
                    .child(div().text_sm().child("Programs the research agent may run"))
                    .child(div().text_xs().text_color(p.faint).child(
                        "In Act mode, with a folder, Tern can convert a document, run a script over your data or tidy citations on its workbench. Each command runs in the chat's folder with a clean environment, limits on memory and time, and your keys and SSH folder out of reach.",
                    ))
                    .child(div().flex().flex_wrap().gap_1().children(chips))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(Input::new(&self.settings_inputs.program).small()),
                            )
                            .child(
                                Button::new("perm-prog-add")
                                    .capsule()
                                    .icon(AssetIcon::Plus)
                                    .label("Add program")
                                    .small()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.add_workbench_program(window, cx)
                                    })),
                            ),
                    )
                    .into_any_element(),
                Self::row(
                    "Network for commands",
                    if network {
                        "On: commands can reach the internet directly"
                    } else {
                        "Off: commands cannot reach the internet. Tern's own web tools still work"
                    },
                    Some(
                        Switch::new("perm-network")
                            .checked(network)
                            .small()
                            .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                let on = *checked;
                                this.save_ui_prefs(move |p| p.workbench_network = on);
                                tern_agent::workbench::set_settings(
                                    this.model.providers.prefs.workbench(),
                                );
                                cx.notify();
                            }))
                            .into_any_element(),
                    ),
                    p,
                ),
            ],
        );
        self.page(
            Section::Permissions,
            p,
            None,
            vec![
                mode_card,
                rules_card,
                workbench_card,
                div()
                    .text_xs()
                    .text_color(p.faint)
                    .child(
                        "A deny rule beats an ask rule, which beats an allow rule. Changes apply from your next message.",
                    )
                    .into_any_element(),
            ],
        )
    }

    // ----- The composer's chip -----

    /// The approval mode on the composer: a shield and the mode's name;
    /// opens the menu of modes.
    pub(super) fn approval_chip(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let mode = self.model.providers.prefs.approval();
        let open = self.model.composer.menu == Some(Menu::Approval);
        let compact = crate::layout::composer_compact(self.centre_w);
        let warn = matches!(mode, ApprovalMode::Full);
        div()
            .id("approval-chip")
            .flex()
            .items_center()
            .gap_1()
            .min_w_0()
            .max_w(px(150.))
            .px_1p5()
            .py(px(3.))
            .rounded_md()
            .text_xs()
            .cursor_pointer()
            .text_color(if open || warn { p.text } else { p.muted })
            .when(open, |d| d.bg(p.hover))
            .hover(|d| d.bg(p.hover).text_color(p.text))
            .child(
                gpui_kit::component::Icon::new(mode_icon(mode))
                    .small()
                    .text_color(if warn {
                        super::work_view::approval_yellow(p)
                    } else {
                        p.muted
                    }),
            )
            .when(!compact, |d| {
                d.child(
                    div()
                        .min_w_0()
                        .truncate()
                        .child(SharedString::from(mode.label())),
                )
            })
            .tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(format!(
                    "Approval mode: {}. {}",
                    mode.label(),
                    mode.line()
                ))
                .build(window, cx)
            })
            .on_click(cx.listener(|this, _, _, cx| {
                this.model
                    .composer
                    .toggle_at(Menu::Approval, std::time::Instant::now());
                cx.notify();
            }))
            .into_any_element()
    }

    /// The composer's menu of approval modes.
    pub(super) fn approval_menu(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let current = self.model.providers.prefs.approval();
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .px_2()
                    .pt_1()
                    .pb(px(2.))
                    .text_xs()
                    .text_color(p.faint)
                    .child("Approval mode"),
            )
            .children(ApprovalMode::ALL.iter().enumerate().map(|(i, &m)| {
                Self::menu_row(
                    ("approval-item", i),
                    gpui_kit::component::Icon::new(mode_icon(m))
                        .small()
                        .text_color(p.muted)
                        .into_any_element(),
                    m.label().into(),
                    m.line().into(),
                    m == current,
                    true,
                    p,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.set_approval_mode(m);
                    this.model.composer.menu = None;
                    cx.notify();
                }))
            }))
            .child(
                div()
                    .id("approval-rules")
                    .px_2()
                    .py_1()
                    .text_xs()
                    .text_color(p.faint)
                    .cursor_pointer()
                    .hover(|d| d.text_color(p.text))
                    .child("Rules and the workbench: Settings › Permissions")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.model.composer.menu = None;
                        this.open_settings(Section::Permissions, window, cx);
                    })),
            )
            .into_any_element()
    }
}
