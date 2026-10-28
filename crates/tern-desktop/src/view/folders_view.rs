//! Folders as projects (#74): opening a folder makes (or finds) the
//! project linked to it, and its chats are grouped under it in the
//! sidebar. A new chat picks its folder in the tray under the composer;
//! once it has a message the folder is fixed.

use std::path::PathBuf;

use gpui_kit::assets::IconName as AssetIcon;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Selectable as _, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, Hsla, SharedString, Window, div, px};
use tern_core::ProjectId;

use super::Workspace;
use super::press::Capsule as _;
use crate::bridge::ProjectChange;
use crate::theme::Palette;

/// A small icon in a colour.
fn icon(name: AssetIcon, color: Hsla) -> AnyElement {
    div()
        .flex_none()
        .text_color(color)
        .child(gpui_kit::component::Icon::new(name).xsmall())
        .into_any_element()
}

/// What "show this folder" is called on this system.
pub(crate) fn reveal_label() -> &'static str {
    if cfg!(target_os = "windows") {
        "Reveal in Explorer"
    } else if cfg!(target_os = "macos") {
        "Reveal in Finder"
    } else {
        "Show in file manager"
    }
}

/// The tooltip of a chat's fixed folder.
pub(crate) const FIXED: &str =
    "Folder is fixed for this chat; start a new chat to use another folder";

/// Folder projects the tray offers at most.
const RECENT_FOLDERS: usize = 6;

impl Workspace {
    /// Opens the system folder picker; the folder becomes a project.
    pub(super) fn open_folder_dialog(&mut self, cx: &mut Context<Self>) {
        self.model.folder_menu_open = false;
        let picked = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open folder".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(mut paths))) = picked.await
                && let Some(dir) = paths.pop()
            {
                let _ = this.update(cx, |this, cx| {
                    this.bridge.projects(ProjectChange::Folder(dir));
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// The bridge made or found the folder's project.
    pub(super) fn folder_opened(
        &mut self,
        r: Result<(ProjectId, String, PathBuf), String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match r {
            Ok((id, name, dir)) => {
                self.model.projects.collapsed.remove(&id.to_string());
                self.use_folder(id, name, dir, window, cx);
            }
            Err(e) => self
                .model
                .toast_error(format!("Couldn't open the folder: {e}"), None),
        }
        cx.notify();
    }

    /// Whether the open chat can still take a folder: it has no message.
    fn can_pick_folder(&self) -> bool {
        !self.model.folder_locked()
            && !self.model.is_busy()
            && self.session.as_ref().is_some_and(|s| s.is_draft())
    }

    /// Puts the chat in folder project `id`: the open chat when it is
    /// still empty, else a new chat there.
    pub(super) fn use_folder(
        &mut self,
        id: ProjectId,
        name: String,
        dir: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.model.folder_menu_open = false;
        if self.can_pick_folder()
            && let Some(s) = self.session.as_mut()
        {
            match s.set_draft_project(id, Some(&dir)) {
                Ok(()) => {
                    self.model.projects.new_in = Some(name);
                    self.set_workspace(Some(dir), cx);
                }
                Err(e) => self.model.toast_error(e, None),
            }
            cx.notify();
            return;
        }
        self.new_chat_in(Some(name), window, cx);
    }

    /// The empty chat goes back to the Inbox with no folder.
    fn use_no_folder(&mut self, cx: &mut Context<Self>) {
        self.model.folder_menu_open = false;
        if !self.can_pick_folder() {
            return;
        }
        let inbox = self.model.projects.inbox().map(|p| p.id);
        if let (Some(s), Some(inbox)) = (self.session.as_mut(), inbox)
            && let Err(e) = s.set_draft_project(inbox, None)
        {
            self.model.toast_error(e, None);
        }
        self.model.projects.new_in = None;
        self.set_workspace(None, cx);
    }

    /// A new chat in a folder project opens in its folder (after the
    /// chat connects; `chat_shown` saw no workspace yet).
    pub(super) fn default_folder(&mut self, cx: &mut Context<Self>) {
        let Some(s) = self.session.as_ref() else {
            return;
        };
        if !s.is_draft() || s.workspace().is_some() {
            return;
        }
        let Some(root) = s.project_root().map(std::path::Path::to_path_buf) else {
            return;
        };
        if root.is_dir() {
            self.set_workspace(Some(root), cx);
        }
    }

    /// The project the open chat is filed under, when it is a folder
    /// project: its name (a new chat from inside it starts there too).
    pub(super) fn current_folder_project(&self) -> Option<String> {
        let id = self.current?;
        let project = self
            .model
            .sessions
            .iter()
            .find(|s| s.id == id)
            .and_then(|s| s.project)?;
        let p = self.model.projects.find(project)?;
        p.row.root.as_ref().map(|_| p.row.name.clone())
    }

    /// A folder project's row in the sidebar: chevron, the folder's logo,
    /// its name, and on hover New chat here and ⋯ (Reveal, Rename,
    /// Remove from sidebar).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn folder_project_row(
        &self,
        id: ProjectId,
        name: String,
        root: PathBuf,
        collapsed: bool,
        has_rules: bool,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let menu_open = self.model.project_menu == Some(id);
        let confirming = self.model.projects.confirm_delete == Some(id);
        let chat_name = name.clone();
        let full: SharedString = root.display().to_string().into();
        let row = div()
            .id(SharedString::from(format!("project-{id}")))
            .group("project-row")
            .flex()
            .items_center()
            .gap_1p5()
            .px_1()
            .py(px(3.))
            .rounded_md()
            .text_sm()
            .text_color(p.text)
            .cursor_pointer()
            .hover(|d| d.bg(p.hover))
            .when(menu_open, |d| d.bg(p.hover))
            .drag_over::<super::customize_view::DraggedChat>(move |s, _, _, _| {
                s.bg(p.hover).border_1().border_color(p.accent)
            })
            .on_drop(
                cx.listener(move |this, d: &super::customize_view::DraggedChat, _, cx| {
                    this.bridge.projects(ProjectChange::Move(d.id, id));
                    cx.notify();
                }),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                let saved = this.model.projects.toggle(id);
                let mut prefs = tern_agent::Preferences::load();
                prefs.collapsed_projects = saved;
                this.bridge.save_prefs(prefs);
                cx.notify();
            }))
            .tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(full.clone()).build(window, cx)
            })
            .child(icon(
                if collapsed {
                    AssetIcon::ChevronRight
                } else {
                    AssetIcon::ChevronDown
                },
                p.faint,
            ))
            .child(crate::fileicon::folder_logo(&root.to_string_lossy(), 16.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(SharedString::from(name.clone())),
            )
            .when(has_rules, |d| {
                d.child(div().text_xs().text_color(p.faint).child("TERN.md"))
            })
            .child(
                div()
                    .flex()
                    .opacity(if menu_open || confirming { 1. } else { 0. })
                    .group_hover("project-row", |d| d.opacity(1.))
                    .child(
                        Button::new(SharedString::from(format!("project-chat-{id}")))
                            .capsule()
                            .icon(AssetIcon::MessageSquarePlus)
                            .ghost()
                            .xsmall()
                            .tooltip("New chat in this folder")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.new_chat_in(Some(chat_name.clone()), window, cx);
                            })),
                    )
                    .child(
                        Button::new(SharedString::from(format!("project-menu-{id}")))
                            .capsule()
                            .icon(AssetIcon::Ellipsis)
                            .ghost()
                            .xsmall()
                            .selected(menu_open)
                            .tooltip("More")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.model.project_menu =
                                    (this.model.project_menu != Some(id)).then_some(id);
                                this.model.projects.confirm_delete = None;
                                cx.notify();
                            })),
                    ),
            );
        let mut out = vec![row.into_any_element()];
        if menu_open {
            out.push(self.folder_project_menu(id, root, confirming, p, cx));
        }
        out
    }

    fn folder_project_menu(
        &self,
        id: ProjectId,
        root: PathBuf,
        confirming: bool,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let item = |key: &str, glyph: AssetIcon, label: &str| {
            div()
                .id(SharedString::from(format!("project-menu-{key}-{id}")))
                .flex()
                .items_center()
                .gap_2()
                .px_2()
                .py_1()
                .rounded_md()
                .text_sm()
                .text_color(p.text)
                .cursor_pointer()
                .hover(move |d| d.bg(p.hover))
                .child(icon(glyph, p.muted))
                .child(SharedString::from(label.to_owned()))
        };
        div()
            .ml(px(22.))
            .mr_1()
            .flex()
            .flex_col()
            .gap(px(2.))
            .p_1()
            .rounded_lg()
            .border_1()
            .border_color(p.border)
            .bg(p.raised)
            .shadow_md()
            .child(
                item("reveal", AssetIcon::FolderOpen, reveal_label()).on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.model.project_menu = None;
                        if let Err(e) = crate::reveal::open(&root) {
                            this.model
                                .toast_error(format!("Couldn't open the folder: {e}"), None);
                        }
                        cx.notify();
                    },
                )),
            )
            .child(
                item("rename", AssetIcon::Pencil, "Rename").on_click(cx.listener(
                    move |this, _, window, cx| {
                        this.model.project_menu = None;
                        this.start_project_edit(Some(id), window, cx);
                    },
                )),
            )
            .child(
                item(
                    "remove",
                    AssetIcon::X,
                    if confirming {
                        "Click again to remove"
                    } else {
                        "Remove from sidebar"
                    },
                )
                .when(confirming, |d| d.text_color(p.warm))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let t = &mut this.model.projects;
                    if t.confirm_delete == Some(id) {
                        t.confirm_delete = None;
                        this.model.project_menu = None;
                        this.bridge.projects(ProjectChange::Delete(id));
                    } else {
                        t.confirm_delete = Some(id);
                    }
                    cx.notify();
                })),
            )
            .child(
                div()
                    .px_2()
                    .pb_1()
                    .text_xs()
                    .text_color(p.faint)
                    .child("Removing moves its chats to the Inbox. Files on disk are not touched."),
            )
            .into_any_element()
    }

    /// "Show more" under a folder project's first chats.
    pub(super) fn show_more_row(
        &self,
        id: ProjectId,
        hidden: usize,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .id(SharedString::from(format!("project-more-{id}")))
            .pl(px(30.))
            .py(px(3.))
            .rounded_md()
            .text_sm()
            .text_color(p.faint)
            .cursor_pointer()
            .hover(|d| d.text_color(p.text))
            .child(SharedString::from(format!("Show more ({hidden})")))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.model.projects.more.insert(id);
                cx.notify();
            }))
            .into_any_element()
    }

    /// The tray under an empty chat's composer: which folder the chat
    /// works in. Gone once the chat has a message (its folder is fixed).
    pub(super) fn folder_tray(&self, p: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.model.folder_locked() || self.session.is_none() {
            return None;
        }
        let ws = self.model.workspace.clone();
        let label: SharedString = ws
            .as_ref()
            .map_or("No folder".to_owned(), |(n, _)| n.clone())
            .into();
        let logo = match &ws {
            Some((_, path)) if path.is_dir() => {
                crate::fileicon::folder_logo(&path.to_string_lossy(), 14.)
            }
            Some((n, _)) => {
                crate::fileicon::logo(crate::fileicon::FileIcon::of(n), 14., self.model.dark)
            }
            None => icon(AssetIcon::FolderClosed, p.faint),
        };
        let open = self.model.folder_menu_open;
        let chip = div()
            .id("folder-tray-chip")
            .flex()
            .items_center()
            .gap_1p5()
            .px_2()
            .py(px(3.))
            .rounded_md()
            .text_xs()
            .text_color(if ws.is_some() { p.text } else { p.muted })
            .cursor_pointer()
            .hover(|d| d.bg(p.hover))
            .when(open, |d| d.bg(p.hover))
            .child(logo)
            .child(div().max_w(px(260.)).truncate().child(label))
            .child(icon(AssetIcon::ChevronDown, p.faint))
            .tooltip(|window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(
                    "The folder this chat works in. It is fixed once you send the first message.",
                )
                .build(window, cx)
            })
            .on_click(cx.listener(|this, _, _, cx| {
                this.model.folder_menu_open = !this.model.folder_menu_open;
                if !this.model.projects.loaded {
                    this.bridge.projects(ProjectChange::Reload);
                }
                cx.notify();
            }));
        let menu = open.then(|| self.folder_menu(p, cx));
        Some(
            div()
                .w_full()
                .max_w(px(super::COLUMN_MAX_W))
                .mx_auto()
                .px_4()
                .pb_2()
                .child(
                    div()
                        .relative()
                        .mx_3()
                        .px_2()
                        .py_1()
                        .flex()
                        .items_center()
                        .gap_2()
                        .rounded_b_lg()
                        .border_1()
                        .border_t_0()
                        .border_color(p.border)
                        .bg(p.raised.opacity(0.5))
                        .child(chip)
                        .child(div().flex_1())
                        .child(
                            div()
                                .text_xs()
                                .text_color(p.faint)
                                .child("Folder for this chat"),
                        )
                        .children(menu.map(|m| {
                            gpui_kit::deferred(
                                div()
                                    .absolute()
                                    .bottom(px(34.))
                                    .left(px(4.))
                                    .occlude()
                                    .child(m),
                            )
                            .with_priority(3)
                        })),
                )
                .into_any_element(),
        )
    }

    /// The tray's picker: this chat's folder projects, Open folder…, No
    /// folder.
    fn folder_menu(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let current = self
            .model
            .workspace
            .as_ref()
            .map(|(_, path)| tern_agent::projects::folder_key(path));
        let folders: Vec<(ProjectId, String, PathBuf)> = self
            .model
            .projects
            .projects
            .iter()
            .filter(|pr| !pr.row.inbox)
            .filter_map(|pr| {
                pr.row
                    .root
                    .clone()
                    .filter(|r| r.is_dir())
                    .map(|r| (pr.row.id, pr.row.name.clone(), r))
            })
            .take(RECENT_FOLDERS)
            .collect();
        let row = |key: SharedString, selected: bool| {
            div()
                .id(key)
                .flex()
                .items_center()
                .gap_2()
                .px_2()
                .py_1()
                .rounded_md()
                .text_sm()
                .text_color(p.text)
                .cursor_pointer()
                .hover(move |d| d.bg(p.hover))
                .when(selected, |d| d.bg(p.hover))
        };
        let mut m = div()
            .id("folder-menu")
            .w(px(300.))
            .p_1()
            .flex()
            .flex_col()
            .gap(px(2.))
            .rounded_lg()
            .border_1()
            .border_color(p.border)
            .bg(p.canvas)
            .shadow_lg();
        if !folders.is_empty() {
            m = m.child(
                div()
                    .px_2()
                    .pt_1()
                    .text_xs()
                    .text_color(p.faint)
                    .child("Folders"),
            );
        }
        for (id, name, root) in folders {
            let selected =
                current.as_deref() == Some(tern_agent::projects::folder_key(&root).as_str());
            let path: SharedString = root.display().to_string().into();
            let (n2, r2) = (name.clone(), root.clone());
            m = m.child(
                row(SharedString::from(format!("folder-pick-{id}")), selected)
                    .child(crate::fileicon::folder_logo(&root.to_string_lossy(), 16.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().truncate().child(SharedString::from(name)))
                            .child(div().truncate().text_xs().text_color(p.faint).child(path)),
                    )
                    .when(selected, |d| d.child(icon(AssetIcon::Check, p.muted)))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.use_folder(id, n2.clone(), r2.clone(), window, cx);
                    })),
            );
        }
        m.child(div().my(px(3.)).h(px(1.)).bg(p.border))
            .child(
                row("folder-pick-open".into(), false)
                    .child(icon(AssetIcon::FolderPlus, p.muted))
                    .child("Open folder…")
                    .on_click(cx.listener(|this, _, _, cx| this.open_folder_dialog(cx))),
            )
            .child(
                row("folder-pick-none".into(), current.is_none())
                    .child(icon(AssetIcon::FolderClosed, p.muted))
                    .child("No folder")
                    .when(current.is_none(), |d| {
                        d.child(icon(AssetIcon::Check, p.muted))
                    })
                    .on_click(cx.listener(|this, _, _, cx| this.use_no_folder(cx))),
            )
            .into_any_element()
    }
}
