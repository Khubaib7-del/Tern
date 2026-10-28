//! The right rail's Files tab and the title bar's Outputs popover (#74):
//! the files this chat made, newest first, and a live tree of its folder
//! (`crate::files`). A row opens its file with the system's app; its ⋯
//! (or a right click) offers Reveal and Copy path.

use std::path::{Path, PathBuf};

use gpui_kit::assets::IconName as AssetIcon;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Selectable as _, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, Hsla, MouseButton, SharedString, div, px};

use super::Workspace;
use super::press::Capsule as _;
use crate::files::{self, Row};
use crate::theme::Palette;

/// A small icon in a colour.
fn icon(name: AssetIcon, color: Hsla) -> AnyElement {
    div()
        .flex_none()
        .text_color(color)
        .child(gpui_kit::component::Icon::new(name).xsmall())
        .into_any_element()
}

/// Outputs the popover lists before "Show all in Files".
const POPOVER_ROWS: usize = 8;

/// How often the folder is checked for changes while the tab shows.
pub(super) const FOLDER_POLL: std::time::Duration = std::time::Duration::from_secs(2);

impl Workspace {
    /// Whether the folder tree is on screen (the Files tab is open).
    pub(super) fn files_visible(&self) -> bool {
        self.model.run_panel_open
            && !self.model.browser.visible
            && self.model.run_panel.view == crate::model::RunView::Files
    }

    /// Called every [`FOLDER_POLL`]: re-reads the tree when a folder on
    /// screen changed.
    pub(super) fn poll_folder(&mut self, cx: &mut Context<Self>) {
        if self.files_visible() && self.model.files.changed() {
            cx.notify();
        }
    }

    fn open_file(&mut self, path: &Path, cx: &mut Context<Self>) {
        self.model.file_menu = None;
        if !path.exists() {
            self.model
                .toast_error(format!("{} was moved or deleted.", path.display()), None);
        } else if let Err(e) = crate::reveal::open(path) {
            self.model
                .toast_error(format!("Couldn't open it: {e}"), None);
        }
        cx.notify();
    }

    /// One file or folder row: its logo, name, a detail line and the ⋯.
    #[allow(clippy::too_many_arguments)]
    fn file_row(
        &self,
        key: String,
        path: PathBuf,
        name: String,
        detail: Option<String>,
        right: Option<String>,
        depth: usize,
        dir: Option<bool>,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let gone = !path.exists();
        let menu_open = self.model.file_menu.as_deref() == Some(path.as_path());
        let logo = match dir {
            Some(_) if path.is_dir() || dir == Some(true) => {
                crate::fileicon::folder_logo(&path.to_string_lossy(), 16.)
            }
            _ => crate::fileicon::logo(crate::fileicon::FileIcon::of(&name), 16., self.model.dark),
        };
        let chevron = dir.map(|open| {
            if path.is_dir() {
                icon(
                    if open {
                        AssetIcon::ChevronDown
                    } else {
                        AssetIcon::ChevronRight
                    },
                    p.faint,
                )
            } else {
                div().w(px(14.)).into_any_element()
            }
        });
        let tip: SharedString = path.display().to_string().into();
        let (p1, p2, p3) = (path.clone(), path.clone(), path.clone());
        let is_dir = path.is_dir() && dir.is_some();
        let row = div()
            .id(SharedString::from(key.clone()))
            .group("file-row")
            .flex()
            .items_center()
            .gap_1p5()
            .pl(px(6. + depth as f32 * 14.))
            .pr_1()
            .py(px(3.))
            .rounded_md()
            .text_sm()
            .text_color(if gone { p.faint } else { p.text })
            .cursor_pointer()
            .hover(|d| d.bg(p.hover))
            .when(menu_open, |d| d.bg(p.hover))
            .tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                if is_dir {
                    this.model.files.toggle(&p1);
                    cx.notify();
                } else {
                    this.open_file(&p1, cx);
                }
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _, _, cx| {
                    this.model.file_menu = Some(p2.clone());
                    cx.notify();
                }),
            )
            .children(chevron)
            .child(logo)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(div().truncate().child(SharedString::from(name)))
                    .children(detail.map(|d| {
                        div()
                            .truncate()
                            .text_xs()
                            .text_color(p.faint)
                            .child(SharedString::from(d))
                    })),
            )
            .children(right.map(|r| {
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(p.faint)
                    .child(SharedString::from(r))
            }))
            .child(
                div()
                    .flex_none()
                    .opacity(if menu_open { 1. } else { 0. })
                    .group_hover("file-row", |d| d.opacity(1.))
                    .child(
                        Button::new(SharedString::from(format!("{key}-more")))
                            .capsule()
                            .icon(AssetIcon::Ellipsis)
                            .ghost()
                            .xsmall()
                            .selected(menu_open)
                            .tooltip("More")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                let same = this.model.file_menu.as_deref() == Some(p3.as_path());
                                this.model.file_menu = (!same).then(|| p3.clone());
                                cx.notify();
                            })),
                    ),
            );
        let mut out = vec![row.into_any_element()];
        if menu_open {
            out.push(self.file_menu(&key, path, depth, p, cx));
        }
        out
    }

    fn file_menu(
        &self,
        key: &str,
        path: PathBuf,
        depth: usize,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let item = |k: &str, glyph: AssetIcon, label: &str| {
            div()
                .id(SharedString::from(format!("{key}-{k}")))
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
        let (a, b) = (path.clone(), path);
        div()
            .ml(px(22. + depth as f32 * 14.))
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
                item(
                    "reveal",
                    AssetIcon::FolderOpen,
                    super::folders_view::reveal_label(),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.model.file_menu = None;
                    if let Err(e) = crate::reveal::reveal(&a) {
                        this.model
                            .toast_error(format!("Couldn't open the folder: {e}"), None);
                    }
                    cx.notify();
                })),
            )
            .child(
                item("copy", AssetIcon::Copy, "Copy path").on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.model.file_menu = None;
                        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(
                            b.display().to_string(),
                        ));
                        this.model.toast_info("Copied the path.");
                        cx.notify();
                    },
                )),
            )
            .into_any_element()
    }

    /// The Outputs rows, newest first (at most `limit`).
    fn output_rows(&self, limit: usize, p: Palette, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let root = self.model.workspace.as_ref().map(|(_, r)| r.clone());
        self.model
            .outputs
            .iter()
            .rev()
            .take(limit)
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .enumerate()
            .flat_map(|(i, o)| {
                let name = o
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| o.path.display().to_string());
                let bytes = std::fs::metadata(&o.path).ok().map(|m| m.len());
                let in_root = root.as_deref().is_some_and(|r| o.path.parent() == Some(r));
                let detail = if bytes.is_none() {
                    "Moved or deleted".to_owned()
                } else if in_root {
                    "In this chat's folder".to_owned()
                } else {
                    files::short_path(o.path.parent().unwrap_or(&o.path), root.as_deref())
                };
                let right = format!(
                    "{}{}",
                    bytes
                        .map(|b| format!("{} · ", files::size_label(b)))
                        .unwrap_or_default(),
                    files::ago(o.age_secs())
                );
                self.file_row(
                    format!("output-{i}"),
                    o.path,
                    name,
                    Some(detail),
                    Some(right),
                    0,
                    None,
                    p,
                    cx,
                )
            })
            .collect()
    }

    fn section_head(&self, title: &str, n: Option<usize>, p: Palette) -> gpui_kit::Div {
        div()
            .flex()
            .items_center()
            .gap_1()
            .pt_1()
            .pb(px(2.))
            .text_xs()
            .text_color(p.faint)
            .child(SharedString::from(title.to_owned()))
            .children(n.filter(|n| *n > 0).map(|n| {
                div()
                    .text_color(p.faint)
                    .child(SharedString::from(n.to_string()))
            }))
    }

    /// The Files tab: Outputs, then the folder's tree.
    pub(super) fn files_panel(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let outputs = self.output_rows(usize::MAX, p, cx);
        let n_out = self.model.outputs.len();
        let mut col = div()
            .id("files-scroll")
            .size_full()
            .flex()
            .flex_col()
            .gap(px(2.))
            .overflow_y_scroll()
            .track_scroll(&self.files_scroll)
            .child(self.section_head("Outputs", Some(n_out), p));
        if outputs.is_empty() {
            col = col.child(
                div()
                    .px_1()
                    .pb_2()
                    .text_sm()
                    .text_color(p.faint)
                    .child("Files this chat saves (Export, Save as…) or writes show here."),
            );
        } else {
            col = col.children(outputs);
        }
        let root = self.model.files.root.clone();
        let show_ignored = self.model.files.show_ignored;
        let folder_head = div()
            .mt_3()
            .flex()
            .items_center()
            .gap_1()
            .child(self.section_head("Folder", None, p).flex_1())
            .when(root.is_some(), |d| {
                d.child(
                    Button::new("files-hidden")
                        .capsule()
                        .icon(if show_ignored {
                            AssetIcon::Eye
                        } else {
                            AssetIcon::EyeOff
                        })
                        .ghost()
                        .xsmall()
                        .selected(show_ignored)
                        .tooltip(if show_ignored {
                            "Hide .git, node_modules, target and other build folders"
                        } else {
                            "Show .git, node_modules, target and other build folders"
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            let on = !this.model.files.show_ignored;
                            this.model.files.set_show_ignored(on);
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("files-refresh")
                        .capsule()
                        .icon(AssetIcon::RefreshCw)
                        .ghost()
                        .xsmall()
                        .tooltip("Refresh")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.model.files.reload();
                            cx.notify();
                        })),
                )
            });
        col = col.child(folder_head);
        let Some(root) = root else {
            return col
                .child(
                    div()
                        .px_1()
                        .text_sm()
                        .text_color(p.faint)
                        .child("This chat has no folder. Pick one under the composer when you start a new chat."),
                )
                .into_any_element();
        };
        let root_name = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| root.display().to_string());
        let (r1, tip) = (root.clone(), SharedString::from(root.display().to_string()));
        col = col.child(
            div()
                .id("files-root")
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
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Err(e) = crate::reveal::open(&r1) {
                        this.model
                            .toast_error(format!("Couldn't open the folder: {e}"), None);
                    }
                    cx.notify();
                }))
                .child(crate::fileicon::folder_logo(&root.to_string_lossy(), 16.))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(SharedString::from(root_name)),
                ),
        );
        let rows = self.model.files.rows();
        let ignored = self.model.files.ignored_at_root();
        if rows.is_empty() {
            col = col.child(
                div()
                    .pl(px(20.))
                    .text_sm()
                    .text_color(p.faint)
                    .child("Empty folder"),
            );
        }
        for r in rows {
            match r {
                Row::Entry { depth, entry, open } => {
                    let right = entry.bytes.map(files::size_label);
                    let key = format!("tree-{}", entry.path.display());
                    let handle = self.files_scroll.clone();
                    // Long trees: rows far from view are boxes of their
                    // last height (`crate::cull`), built only near it.
                    let row = self.culled(key.clone(), &handle, || {
                        div()
                            .flex()
                            .flex_col()
                            .children(self.file_row(
                                key,
                                entry.path,
                                entry.name,
                                None,
                                right,
                                depth + 1,
                                Some(open && entry.dir),
                                p,
                                cx,
                            ))
                            .into_any_element()
                    });
                    col = col.child(row);
                }
                Row::More { depth, n } => {
                    col = col.child(
                        div()
                            .pl(px(6. + (depth + 1) as f32 * 14. + 20.))
                            .text_xs()
                            .text_color(p.faint)
                            .child(SharedString::from(format!("{n} more not listed"))),
                    );
                }
                Row::Error { depth, text } => {
                    col = col.child(
                        div()
                            .pl(px(6. + (depth + 1) as f32 * 14. + 20.))
                            .text_xs()
                            .text_color(p.warm)
                            .child(SharedString::from(text)),
                    );
                }
            }
        }
        if ignored > 0 && !show_ignored {
            col = col.child(
                div()
                    .pl(px(26.))
                    .pt_1()
                    .text_xs()
                    .text_color(p.faint)
                    .child(SharedString::from(format!(
                        "{ignored} build or cache folder{} hidden",
                        if ignored == 1 { "" } else { "s" }
                    ))),
            );
        }
        col.into_any_element()
    }

    /// The title bar's Outputs button, with its popover.
    pub(super) fn outputs_button(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let n = self.model.outputs.len();
        let open = self.model.outputs_open;
        let mut b = Button::new("outputs")
            .capsule()
            .icon(AssetIcon::FileOutput)
            .ghost()
            .small()
            .selected(open)
            .tooltip("Outputs: files this chat made")
            .on_click(cx.listener(|this, _, _, cx| {
                this.model.outputs_open = !this.model.outputs_open;
                this.model.file_menu = None;
                cx.notify();
            }));
        if n > 0 {
            b = b.label(SharedString::from(n.to_string()));
        }
        let pop = open.then(|| {
            let rows = self.output_rows(POPOVER_ROWS, p, cx);
            let empty = rows.is_empty();
            div()
                .id("outputs-pop")
                .absolute()
                .top(px(32.))
                .right_0()
                .w(px(340.))
                .p_2()
                .flex()
                .flex_col()
                .gap(px(2.))
                .rounded_lg()
                .border_1()
                .border_color(p.border)
                .bg(p.canvas)
                .shadow_lg()
                .occlude()
                .child(self.section_head("Outputs", Some(n), p))
                .when(empty, |d| {
                    d.child(
                        div()
                            .px_1()
                            .pb_1()
                            .text_sm()
                            .text_color(p.faint)
                            .child("Nothing yet. Export a table, a diagram or a formula, or let the agent write a file."),
                    )
                })
                .children(rows)
                .child(div().my(px(3.)).h(px(1.)).bg(p.border))
                .child(
                    div()
                        .id("outputs-all")
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
                        .child(icon(AssetIcon::ListTree, p.muted))
                        .child("Open the Files panel")
                        .on_click(cx.listener(|this, _, _, cx| this.show_files(cx))),
                )
        });
        div()
            .relative()
            .child(b)
            .children(pop.map(|m| gpui_kit::deferred(m).with_priority(3)))
            .into_any_element()
    }
}
