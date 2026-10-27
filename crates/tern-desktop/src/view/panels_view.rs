//! Panel chrome: drag handles on the inner panel edges, maximise and
//! restore, the tab strip with its overflow menu, the Archived chats
//! screen, the sidebar row's ⋯ menu, and removing a watch with Undo.
//! State lives in `crate::panels`.

use gpui_kit::assets::IconName as AssetIcon;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{IconName, Selectable as _, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, SharedString, Window, div, px};
use tern_core::SessionId;

use super::press::Capsule as _;
use super::{RAIL_W, Workspace};
use crate::model::Screen;
use crate::panels::{self, ArchiveTab, Edge, Focus};
use crate::theme::Palette;

impl Workspace {
    /// The 4 px handle on an inner panel edge. Drag to resize, double-click
    /// to reset. It sits between two panes and overlaps both by 2 px.
    pub(super) fn drag_handle(&self, edge: Edge, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let active = self.model.panels.dragging() == Some(edge);
        let id: &'static str = match edge {
            Edge::Sidebar => "drag-sidebar",
            Edge::Right => "drag-right",
        };
        div()
            .id(id)
            .flex_none()
            .h_full()
            .w(px(panels::HANDLE_W))
            .ml(px(-panels::HANDLE_W / 2.))
            .mr(px(-panels::HANDLE_W / 2.))
            .relative()
            .cursor_col_resize()
            .when(active, |d| d.bg(p.border))
            .hover(|d| d.bg(p.border))
            .on_mouse_down(
                gpui_kit::MouseButton::Left,
                cx.listener(move |this, e: &gpui_kit::MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    if e.click_count >= 2 {
                        this.model.panels.reset(edge);
                        this.save_panel_widths();
                    } else {
                        this.model.panels.start_drag(edge, f32::from(e.position.x));
                    }
                    cx.notify();
                }),
            )
            .into_any_element()
    }

    /// The pointer moved anywhere in the window while a handle is held.
    pub(super) fn panel_drag_move(&mut self, x: f32, window: &mut Window, cx: &mut Context<Self>) {
        let Some(edge) = self.model.panels.dragging() else {
            return;
        };
        let window_w = f32::from(window.viewport_size().width);
        // Beside the browser the divider sets its share, not a width.
        if edge == Edge::Right && self.model.browser.visible {
            let left = self.centre_x
                + if self.centre_x > RAIL_W {
                    crate::panels::HANDLE_W
                } else {
                    0.
                };
            self.browser_share = crate::browser::split_share(x, left, window_w);
            cx.notify();
            return;
        }
        // The width the other side panel takes, 0 when it is closed.
        let other = match edge {
            Edge::Sidebar if self.model.run_panel_open => self.model.panels.right,
            Edge::Right if self.model.sidebar_open => self.model.panels.sidebar,
            _ => 0.,
        };
        self.model.panels.drag_to(x, window_w, RAIL_W, other);
        cx.notify();
    }

    /// The pointer was released: the drag ends and the widths are saved.
    pub(super) fn panel_drag_end(&mut self, cx: &mut Context<Self>) {
        if self.model.panels.dragging().is_none() {
            return;
        }
        if self.model.panels.end_drag() {
            self.save_panel_widths();
        }
        cx.notify();
    }

    fn save_panel_widths(&mut self) {
        let (sidebar, right) = self.model.panels.to_prefs();
        self.save_ui_prefs(move |prefs| {
            prefs.sidebar_width = sidebar;
            prefs.right_pane_width = right;
        });
    }

    /// Ctrl+\ maximises or restores the right pane; Ctrl+Alt+\ shows the
    /// chat alone. Returns whether the key was taken.
    pub(super) fn panel_keys(
        &mut self,
        e: &gpui_kit::KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let k = &e.keystroke;
        if k.key != "\\" || !(k.modifiers.control || k.modifiers.platform) {
            return false;
        }
        if k.modifiers.alt {
            self.model.panels.toggle_centre_max();
        } else {
            self.toggle_right_max();
        }
        cx.stop_propagation();
        cx.notify();
        true
    }

    /// Maximise or restore the right pane, opening it first when closed.
    pub(super) fn toggle_right_max(&mut self) {
        if !self.model.run_panel_open && self.model.panels.focus != Focus::RightMax {
            self.model.run_panel_open = true;
        }
        self.model.panels.toggle_right_max();
    }

    /// The title-bar button beside the run panel toggle.
    pub(super) fn maximise_button(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.model.run_panel_open && self.model.panels.focus == Focus::Normal {
            return None;
        }
        let focus = self.model.panels.focus;
        let (icon, tip) = match focus {
            Focus::Normal => (AssetIcon::Maximize2, "Maximise the right pane (Ctrl+\\)"),
            _ => (AssetIcon::Minimize2, "Restore the panels (Ctrl+\\)"),
        };
        Some(
            Button::new("maximise-right")
                .capsule()
                .icon(icon)
                .ghost()
                .small()
                .tooltip(tip)
                .on_click(cx.listener(move |this, _, _, cx| {
                    if focus == Focus::CentreMax {
                        this.model.panels.toggle_centre_max();
                    } else {
                        this.toggle_right_max();
                    }
                    cx.notify();
                }))
                .into_any_element(),
        )
    }

    /// Open tabs as quiet text tabs; those that do not fit go into "▾".
    pub(super) fn tab_strip(&self, p: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let active = self.tabs.active;
        let widths: Vec<f32> = self
            .tabs
            .list
            .iter()
            .map(|t| panels::tab_width(&t.title))
            .collect();
        // Room for tabs: the centre less padding and the + button.
        let room = (self.centre_w - 16. - 28.).max(0.);
        let (shown, hidden) = panels::tab_overflow(&widths, room, 40., active);
        let tab = |i: usize, cx: &mut Context<Self>| {
            let t = &self.tabs.list[i];
            let on = i == active;
            div()
                .id(("tab", i))
                .group("tab")
                .flex()
                .flex_shrink_0()
                .items_center()
                .gap_1()
                .max_w(px(200.))
                .h(px(30.))
                .pl_3()
                .pr_1()
                .text_sm()
                .cursor_pointer()
                .relative()
                .border_b_2()
                .border_color(p.canvas.opacity(0.))
                .child(self.measure("tabs", i))
                .text_color(if on { p.text } else { p.muted })
                .when(!on, |d| d.hover(move |d| d.text_color(p.text)))
                // Other chats stay full strength while this one runs: each
                // runs on its own (#71). A chat running out of sight shows
                // a small spinner.
                .when(
                    !on && t.id.is_some_and(|id| self.runs_in_background(id)),
                    |d| d.child(div().flex_none().child(Spinner::new().xsmall())),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .child(SharedString::from(t.title.clone())),
                )
                .child(
                    div()
                        .opacity(if on { 1. } else { 0. })
                        .group_hover("tab", |d| d.opacity(1.))
                        .child(
                            Button::new(("close-tab", i))
                                .capsule()
                                .icon(IconName::Close)
                                .ghost()
                                .xsmall()
                                .tooltip("Close tab")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.close_tab(i, window, cx);
                                })),
                        ),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.tabs_menu_open = false;
                    this.focus_tab(i, window, cx);
                }))
                .into_any_element()
        };
        let shown_tabs: Vec<AnyElement> = shown.iter().map(|&i| tab(i, cx)).collect();
        let more = (!hidden.is_empty()).then(|| {
            let open = self.tabs_menu_open;
            let n = hidden.len();
            div()
                .relative()
                .flex_none()
                .child(
                    div()
                        .id("tabs-more")
                        .flex()
                        .items_center()
                        .gap(px(2.))
                        .px_2()
                        .h(px(30.))
                        .text_sm()
                        .cursor_pointer()
                        .text_color(if open { p.text } else { p.muted })
                        .hover(move |d| d.text_color(p.text))
                        .child(SharedString::from(format!("{n}")))
                        .child(gpui_kit::component::Icon::new(AssetIcon::ChevronDown).xsmall())
                        .on_click(cx.listener(|this, _, _, cx| {
                            if !this.popover_just_closed() {
                                this.tabs_menu_open = !this.tabs_menu_open;
                            }
                            cx.notify();
                        })),
                )
                .when(open, |d| {
                    d.child(
                        gpui_kit::deferred(
                            div()
                                .id("tabs-more-menu")
                                .absolute()
                                .top(px(32.))
                                .right_0()
                                .w(px(240.))
                                .p_1()
                                .flex()
                                .flex_col()
                                .rounded_lg()
                                .border_1()
                                .border_color(p.border)
                                .bg(p.raised)
                                .shadow_lg()
                                .occlude()
                                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                    this.tabs_menu_open = false;
                                    this.popover_closed_at = Some(std::time::Instant::now());
                                    cx.notify();
                                }))
                                .children(hidden.iter().map(|&i| {
                                    let title = self.tabs.list[i].title.clone();
                                    div()
                                        .id(("tab-hidden", i))
                                        .px_2()
                                        .py_1()
                                        .rounded_md()
                                        .text_sm()
                                        .truncate()
                                        .text_color(p.muted)
                                        .cursor_pointer()
                                        .hover(move |d| d.bg(p.hover).text_color(p.text))
                                        .child(SharedString::from(title))
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.tabs_menu_open = false;
                                            this.focus_tab(i, window, cx);
                                        }))
                                })),
                        )
                        .with_priority(3),
                    )
                })
                .into_any_element()
        });
        Some(
            div()
                .id("tabs")
                .flex()
                .items_end()
                .gap_1()
                .px_2()
                .border_b_1()
                .border_color(p.border)
                .child(
                    div()
                        .relative()
                        .flex()
                        .items_end()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .child(self.measure("tabs", usize::MAX))
                        .children(shown_tabs)
                        // One underline that springs between tabs.
                        .children(self.underline(
                            "tabs",
                            "tabs-underline-x",
                            "tabs-underline-w",
                            shown.contains(&active).then_some(active),
                            p.accent,
                        )),
                )
                .children(more)
                .child(
                    div()
                        .relative()
                        .pb(px(3.))
                        .child(
                            Button::new("add-tab")
                                .capsule()
                                .icon(IconName::Plus)
                                .ghost()
                                .xsmall()
                                .selected(self.tab_picker.is_some())
                                .tooltip("New chat or open one in a tab")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.toggle_tab_picker(window, cx)
                                })),
                        )
                        // Drawn in the overlay layer, over the page.
                        .children(
                            self.tab_picker_view(p, cx)
                                .map(|m| gpui_kit::deferred(m).with_priority(3)),
                        ),
                )
                .into_any_element(),
        )
    }

    /// Opens or closes the + popover on the tab strip.
    /// Whether a tab-strip popover closed on this same press.
    fn popover_just_closed(&mut self) -> bool {
        self.popover_closed_at
            .take()
            .is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(300))
    }

    pub(super) fn toggle_tab_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tab_picker.take().is_some() || self.popover_just_closed() {
            cx.notify();
            return;
        }
        let input = cx.new(|cx| {
            gpui_kit::component::input::InputState::new(window, cx)
                .placeholder("Search this harness's chats")
        });
        input.update(cx, |i, cx| i.focus(window, cx));
        let sub = cx.observe(&input, |this, _, cx| {
            // A new search starts at its first row.
            this.tab_pick_sel = 0;
            cx.notify();
        });
        self.tab_picker = Some((input, sub));
        self.tab_pick_sel = 0;
        cx.notify();
    }

    /// The + popover's keys: Up and Down move, Enter opens, Esc closes.
    fn tab_picker_key(
        &mut self,
        e: &gpui_kit::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((input, _)) = &self.tab_picker else {
            return;
        };
        let query = input.read(cx).value().to_string();
        let rows = panels::pick_rows_for(&self.model.sessions, &self.tabs, &query);
        // Row 0 is New chat, then the chats.
        let n = rows.len() + 1;
        match e.keystroke.key.as_str() {
            "escape" => {
                self.tab_picker = None;
            }
            "down" => self.tab_pick_sel = (self.tab_pick_sel + 1).min(n - 1),
            "up" => self.tab_pick_sel = self.tab_pick_sel.saturating_sub(1),
            "enter" => {
                let sel = self.tab_pick_sel.min(n - 1);
                self.tab_picker = None;
                if sel == 0 {
                    self.add_tab(window, cx);
                } else if let Some(r) = rows.get(sel - 1) {
                    let (id, title) = (r.id, r.title.clone());
                    self.open_in_new_tab(id, &title, window, cx);
                }
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// The + popover: a new chat, one of this harness's chats (searchable;
    /// it opens in a new tab, or focuses its tab), and the other things
    /// that open beside the chat.
    fn tab_picker_view(&self, p: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (input, _) = self.tab_picker.as_ref()?;
        let query = input.read(cx).value().to_string();
        let rows = panels::pick_rows_for(&self.model.sessions, &self.tabs, &query);
        let sel = self.tab_pick_sel;
        let row = move |id: SharedString,
                        icon: AssetIcon,
                        label: SharedString,
                        meta: Option<&'static str>,
                        at: Option<usize>| {
            div()
                .id(id)
                .when(at.is_some() && at == Some(sel), |d| d.bg(p.hover))
                .flex()
                .items_center()
                .gap_2()
                .px_2()
                .py_1()
                .rounded_md()
                .text_sm()
                .cursor_pointer()
                .text_color(p.text)
                .hover(move |d| d.bg(p.hover))
                .child(
                    div()
                        .text_color(p.muted)
                        .child(gpui_kit::component::Icon::new(icon).small()),
                )
                .child(div().flex_1().min_w_0().truncate().child(label))
                .children(meta.map(|m| div().text_xs().text_color(p.faint).child(m)))
        };
        let head = move |t: &'static str| {
            div()
                .px_2()
                .pt_2()
                .pb_1()
                .text_xs()
                .font_family(crate::assets::MONO_FONT)
                .text_color(p.faint)
                .child(t)
        };
        let chats: Vec<AnyElement> = rows
            .into_iter()
            .enumerate()
            .map(|(k, r)| {
                let id = r.id;
                let title = r.title.clone();
                row(
                    SharedString::from(format!("pick-{id}")),
                    AssetIcon::MessageCircle,
                    SharedString::from(r.title),
                    r.open.then_some("open"),
                    Some(k + 1),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.tab_picker = None;
                    this.open_in_new_tab(id, &title, window, cx);
                }))
                .into_any_element()
            })
            .collect();
        let empty = chats.is_empty();
        let has_report = self
            .model
            .entries
            .iter()
            .any(|e| matches!(e, crate::model::Entry::Assistant { .. }));
        Some(
            div()
                .id("tab-picker")
                .absolute()
                .top(px(30.))
                .right_0()
                .w(px(320.))
                .max_h(px(440.))
                .p_1()
                .flex()
                .flex_col()
                .rounded_lg()
                .border_1()
                .border_color(p.border)
                .bg(p.raised)
                .shadow_lg()
                .occlude()
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.tab_picker = None;
                    this.popover_closed_at = Some(std::time::Instant::now());
                    cx.notify();
                }))
                .capture_key_down(cx.listener(|this, e: &gpui_kit::KeyDownEvent, window, cx| {
                    this.tab_picker_key(e, window, cx);
                }))
                .child(
                    row(
                        "pick-new".into(),
                        AssetIcon::Plus,
                        "New chat".into(),
                        Some("Ctrl+T"),
                        Some(0),
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.tab_picker = None;
                        this.add_tab(window, cx);
                    })),
                )
                .child(head("OPEN EXISTING CHAT"))
                .child(
                    div()
                        .px_1()
                        .pb_1()
                        .child(gpui_kit::component::input::Input::new(input).small()),
                )
                .child(
                    div()
                        .id("pick-list")
                        .flex()
                        .flex_col()
                        .max_h(px(220.))
                        .overflow_y_scrollbar()
                        .children(chats)
                        .when(empty, |d| {
                            d.child(
                                div()
                                    .px_2()
                                    .py_1()
                                    .text_sm()
                                    .text_color(p.faint)
                                    .child("No chats match."),
                            )
                        }),
                )
                .child(head("ALSO OPEN"))
                .when(has_report, |d| {
                    d.child(
                        row(
                            "pick-report".into(),
                            AssetIcon::FileText,
                            "Report of this chat".into(),
                            None,
                            None,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.tab_picker = None;
                            if let Some(i) =
                                this.model.entries.iter().rposition(|e| {
                                    matches!(e, crate::model::Entry::Assistant { .. })
                                })
                            {
                                this.model.show_run_of(i);
                                this.model.run_panel.view = crate::model::RunView::Report;
                                this.model.browser.visible = false;
                                this.model.run_panel_open = true;
                            }
                            cx.notify();
                        })),
                    )
                })
                .child(
                    row(
                        "pick-library".into(),
                        AssetIcon::Library,
                        "Library item".into(),
                        None,
                        None,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.tab_picker = None;
                        this.open_library(cx);
                    })),
                )
                .child(
                    row(
                        "pick-browser".into(),
                        AssetIcon::Globe,
                        "Browser".into(),
                        None,
                        None,
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.tab_picker = None;
                        if !(this.model.browser.visible && this.model.run_panel_open) {
                            this.toggle_browser(window, cx);
                        }
                    })),
                )
                .into_any_element(),
        )
    }

    /// Opens the Archived chats screen and loads its lists.
    pub(super) fn open_archived(&mut self, cx: &mut Context<Self>) {
        self.model.screen = Screen::Archived;
        self.model.row_menu = None;
        self.bridge.load_archive();
        cx.notify();
    }

    /// A chat goes to the trash, with Undo.
    pub(super) fn delete_chat(
        &mut self,
        id: SessionId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.current == Some(id) {
            if self.model.is_busy() {
                return;
            }
            self.current = None;
            self.model.entries.clear();
            self.new_chat(window, cx);
        }
        if let Some(tab) = self.tabs.forget(id).cloned()
            && tab.id.is_some()
        {
            self.show_tab(tab, window, cx);
        }
        self.model.chat_deleted(id);
        self.harness_memory.forget(id);
        self.parked.retain(|_, s| *s != id);
        let memory = self.harness_memory.clone();
        self.save_ui_prefs(|p| memory.save_into(p));
        self.bridge.delete_chat(id);
        self.start_undo_timer(cx);
        cx.notify();
    }

    /// The sidebar row with its ⋯ menu and delete confirm under it.
    pub(super) fn with_row_menu(
        &self,
        row: AnyElement,
        ix: usize,
        id: SessionId,
        sub: bool,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let confirming = self.model.confirm_delete == Some(id);
        if self.model.row_menu != Some(id) && !confirming {
            return row;
        }
        let item = |key: &'static str, icon: AssetIcon, label: &'static str, danger: bool| {
            div()
                .id((key, ix))
                .flex()
                .items_center()
                .gap_2()
                .px_2()
                .py_1()
                .rounded_md()
                .text_sm()
                .cursor_pointer()
                .text_color(if danger { p.text } else { p.muted })
                .hover(move |d| d.bg(p.hover).text_color(p.text))
                .child(gpui_kit::component::Icon::new(icon).xsmall())
                .child(label)
        };
        let body = if confirming {
            div()
                .flex()
                .flex_col()
                .gap_2()
                .p_2()
                .child(
                    div()
                        .text_sm()
                        .text_color(p.text)
                        .child("Delete this chat? It goes to the trash; you can restore it from Archived until you empty the trash."),
                )
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap_1()
                        .child(
                            Button::new(("row-del-cancel", ix)).capsule()
                                .label("Cancel")
                                .ghost()
                                .xsmall()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.model.confirm_delete = None;
                                    this.model.row_menu = None;
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new(("row-del-ok", ix)).capsule()
                                .label("Delete")
                                .danger()
                                .xsmall()
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.delete_chat(id, window, cx);
                                })),
                        ),
                )
                .into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .p_1()
                .child(
                    item("row-rename", AssetIcon::Pencil, "Rename", false).on_click(cx.listener(
                        move |this, _, window, cx| {
                            this.model.row_menu = None;
                            this.start_rename(id, window, cx);
                        },
                    )),
                )
                .when(!sub, |d| {
                    d.child(
                        item(
                            "row-move",
                            AssetIcon::FolderInput,
                            "Move to project…",
                            false,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.model.row_menu = None;
                            this.toggle_move_menu(id, cx);
                        })),
                    )
                })
                .child(
                    item("row-tab", AssetIcon::SquarePlus, "Open in new tab", false).on_click(
                        cx.listener(move |this, _, window, cx| {
                            this.model.row_menu = None;
                            this.open_in_tab(id, window, cx);
                        }),
                    ),
                )
                .child(
                    item("row-handoff", AssetIcon::FileOutput, "Hand off", false).on_click(
                        cx.listener(move |this, _, _, cx| {
                            this.model.row_menu = None;
                            this.hand_off(id, cx);
                        }),
                    ),
                )
                .child(
                    item("row-archive", AssetIcon::Archive, "Archive", false).on_click(
                        cx.listener(move |this, _, window, cx| {
                            this.model.row_menu = None;
                            this.archive_chat(id, window, cx);
                        }),
                    ),
                )
                .child(
                    item("row-delete", AssetIcon::Trash, "Delete…", true).on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.model.confirm_delete = Some(id);
                            cx.notify();
                        },
                    )),
                )
                .into_any_element()
        };
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(row)
            .child(
                div()
                    .mx_1()
                    .rounded_lg()
                    .border_1()
                    .border_color(p.border)
                    .bg(p.raised)
                    .shadow_md()
                    .child(body),
            )
            .into_any_element()
    }

    /// The ⋯ button on a sidebar row.
    pub(super) fn row_menu_button(
        &self,
        ix: usize,
        id: SessionId,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        Button::new(("row-menu", ix))
            .capsule()
            .icon(AssetIcon::Ellipsis)
            .ghost()
            .xsmall()
            .tooltip("Rename, move, archive, delete")
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.model.confirm_delete = None;
                this.model.row_menu = if this.model.row_menu == Some(id) {
                    None
                } else {
                    Some(id)
                };
                cx.notify();
            }))
            .into_any_element()
    }

    /// The sidebar footer link to Archived chats.
    pub(super) fn archived_link(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let on = self.model.screen == Screen::Archived;
        div()
            .id("archived-link")
            .flex()
            .items_center()
            .gap_2()
            .mx_2()
            .mb_1()
            .px_2()
            .py_1()
            .rounded_md()
            .text_xs()
            .cursor_pointer()
            .text_color(if on { p.text } else { p.faint })
            .when(on, |d| d.bg(p.hover))
            .hover(move |d| d.bg(p.hover).text_color(p.text))
            .child(gpui_kit::component::Icon::new(AssetIcon::Archive).xsmall())
            .child("Archived chats")
            .on_click(cx.listener(|this, _, _, cx| this.open_archived(cx)))
            .into_any_element()
    }

    /// Archived chats, and deleted ones until the trash is emptied.
    pub(super) fn archived_screen(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let a = &self.model.archive;
        let (n_archived, n_deleted) = a.counts();
        let deleted_tab = a.tab == ArchiveTab::Deleted;
        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .flex_wrap()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().text_xl().text_color(p.text).child("Archived chats"))
                    .child(
                        div()
                            .text_sm()
                            .text_color(p.muted)
                            .child("Archived chats leave the sidebar but keep everything. Deleted chats wait in the trash until you empty it."),
                    ),
            )
            .child(
                Button::new("archived-back").capsule()
                    .icon(AssetIcon::ArrowLeft)
                    .label("Back to chat")
                    .ghost()
                    .small()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.model.screen = Screen::Chat;
                        cx.notify();
                    })),
            );
        let tabs = div()
            .flex()
            .items_end()
            .justify_between()
            .border_b_1()
            .border_color(p.border)
            .child(div().pb_1().child(self.segmented(
                "archive-tabs",
                &[
                    super::segmented::Segment::text(n_archived),
                    super::segmented::Segment::text(n_deleted),
                ],
                usize::from(a.tab == ArchiveTab::Deleted),
                super::segmented::Look {
                    small: true,
                    ..Default::default()
                },
                p,
                std::rc::Rc::new(|this: &mut Self, i, _, _| {
                    this.model.archive.tab = if i == 0 {
                        ArchiveTab::Archived
                    } else {
                        ArchiveTab::Deleted
                    };
                    this.model.archive.confirm = None;
                    this.model.archive.confirm_empty = false;
                }),
                cx,
            )))
            .when(deleted_tab && !a.deleted.is_empty(), |d| {
                d.child(div().pb_1().child(if a.confirm_empty {
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .text_xs()
                        .text_color(p.muted)
                        .child("Delete every chat in the trash for good?")
                        .child(
                            Button::new("empty-cancel")
                                .capsule()
                                .label("Cancel")
                                .ghost()
                                .xsmall()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.model.archive.confirm_empty = false;
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("empty-ok")
                                .capsule()
                                .label("Empty trash")
                                .danger()
                                .xsmall()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.model.archive.confirm_empty = false;
                                    this.model.archive.deleted.clear();
                                    this.bridge.empty_trash();
                                    cx.notify();
                                })),
                        )
                        .into_any_element()
                } else {
                    Button::new("empty-trash")
                        .capsule()
                        .icon(AssetIcon::Trash)
                        .label("Empty trash")
                        .ghost()
                        .xsmall()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.model.archive.confirm_empty = true;
                            cx.notify();
                        }))
                        .into_any_element()
                }))
            });
        let rows: Vec<AnyElement> = a
            .rows()
            .iter()
            .enumerate()
            .map(|(ix, s)| {
                let id = s.id;
                let confirm = a.confirm == Some(id);
                let when = s.created_at.get(..10).unwrap_or("").to_owned();
                let meta = format!(
                    "{} · {} event{}{}",
                    match s.harness {
                        tern_agent::Harness::Study => "Study",
                        _ => "Research",
                    },
                    s.events,
                    if s.events == 1 { "" } else { "s" },
                    if when.is_empty() {
                        String::new()
                    } else {
                        format!(" · {when}")
                    }
                );
                let actions = if confirm {
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .text_xs()
                        .text_color(p.muted)
                        .child("Delete for good?")
                        .child(
                            Button::new(("erase-cancel", ix))
                                .capsule()
                                .label("Cancel")
                                .ghost()
                                .xsmall()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.model.archive.confirm = None;
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new(("erase-ok", ix))
                                .capsule()
                                .label("Delete")
                                .danger()
                                .xsmall()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.model.archive.drop_row(id);
                                    this.bridge.erase_chat(id);
                                    cx.notify();
                                })),
                        )
                } else if deleted_tab {
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(
                            Button::new(("restore", ix))
                                .capsule()
                                .icon(AssetIcon::ArchiveRestore)
                                .label("Restore")
                                .ghost()
                                .xsmall()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.model.archive.drop_row(id);
                                    this.bridge.restore_chat(id);
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new(("erase", ix))
                                .capsule()
                                .icon(AssetIcon::Trash)
                                .label("Delete for good")
                                .ghost()
                                .xsmall()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.model.archive.confirm = Some(id);
                                    cx.notify();
                                })),
                        )
                } else {
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(
                            Button::new(("unarchive", ix))
                                .capsule()
                                .icon(AssetIcon::ArchiveRestore)
                                .label("Unarchive")
                                .ghost()
                                .xsmall()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.model.archive.drop_row(id);
                                    this.bridge.unarchive_chat(id);
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new(("archived-delete", ix))
                                .capsule()
                                .icon(AssetIcon::Trash)
                                .label("Delete")
                                .ghost()
                                .xsmall()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.model.archive.drop_row(id);
                                    this.bridge.delete_chat(id);
                                    cx.notify();
                                })),
                        )
                };
                div()
                    .id(("archived-row", ix))
                    .flex()
                    .items_center()
                    .justify_between()
                    .flex_wrap()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .hover(move |d| d.bg(p.hover))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(200.))
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(p.text)
                                    .truncate()
                                    .child(SharedString::from(s.title.clone())),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(p.faint)
                                    .child(SharedString::from(meta)),
                            ),
                    )
                    .child(actions)
                    .into_any_element()
            })
            .collect();
        let empty = match a.tab {
            ArchiveTab::Archived => {
                "No archived chats. Archive one from a chat's ⋯ menu in the sidebar."
            }
            ArchiveTab::Deleted => "The trash is empty.",
        };
        div()
            .id("archived-screen")
            .flex_1()
            .min_h_0()
            .overflow_y_scrollbar()
            .flex()
            .justify_center()
            .child(
                div()
                    .w_full()
                    .max_w(px(super::COLUMN_MAX_W))
                    .px_4()
                    .py_6()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(header)
                    .child(tabs)
                    .when_some(a.note.clone(), |d, n| {
                        d.child(
                            div()
                                .text_xs()
                                .text_color(p.muted)
                                .child(SharedString::from(n)),
                        )
                    })
                    .when(!a.loaded, |d| {
                        d.child(div().text_sm().text_color(p.faint).child("Loading…"))
                    })
                    .when(a.loaded && rows.is_empty(), |d| {
                        d.child(div().text_sm().text_color(p.faint).child(empty))
                    })
                    .child(div().flex().flex_col().gap_1().children(rows)),
            )
            .into_any_element()
    }

    /// Removes a watch from the list, with Undo.
    pub(super) fn remove_watch(&mut self, id: String, cx: &mut Context<Self>) {
        if self.model.remove_watch_row(&id) {
            self.bridge.remove_watch(id);
            self.start_undo_timer(cx);
        }
        cx.notify();
    }
}
