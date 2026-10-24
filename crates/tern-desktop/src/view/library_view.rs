//! The Library page (`docs/02-ui-definition.md` §2.1): Reports, Sources,
//! Notes, Decks, Files, Explain in 3D, Sessions, All, and the Trash; grid
//! or list; `New ▾` (note, report, deck, folder, upload); search, sort,
//! bulk select, bulk delete to the trash, restore, and empty the trash.
//! The index lives in the store and is read a page at a time, so thousands
//! of items stay quick.

use std::time::Instant;

use gpui_kit::assets::IconName as AssetIcon;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{Disableable as _, Selectable as _, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, FontWeight, SharedString, Window, div, px};
use tern_agent::PageRef;
use tern_agent::library::{LibraryItem, LibraryKind};

use super::Workspace;
use super::study_view::serif;
use crate::bridge::LibraryChange;
use crate::continuity::LibraryTab;
use crate::model::{Screen, SiteIcon};
use crate::study::StudyView;
use crate::theme::Palette;

fn kind_icon(kind: LibraryKind) -> AssetIcon {
    match kind {
        LibraryKind::Report => AssetIcon::ScrollText,
        LibraryKind::Source => AssetIcon::Globe,
        LibraryKind::Note => AssetIcon::NotebookPen,
        LibraryKind::Deck => AssetIcon::Layers,
        LibraryKind::File => AssetIcon::FileText,
        LibraryKind::Scene => AssetIcon::Cuboid,
        LibraryKind::Session => AssetIcon::MessagesSquare,
    }
}

/// The site of a source row (its detail starts with it).
fn source_page(it: &LibraryItem) -> PageRef {
    let site = it.detail.split(" · ").next().unwrap_or_default().to_owned();
    PageRef {
        title: it.title.clone(),
        url: it.location.clone(),
        favicon: Some(format!("https://{site}/favicon.ico")),
        site,
        builtin: None,
    }
}

fn when_label(at: i64) -> String {
    if at <= 0 {
        return String::new();
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(at);
    tern_agent::study::ago(now - at)
}

impl Workspace {
    pub(super) fn open_library(&mut self, cx: &mut Context<Self>) {
        self.model.screen = Screen::Library;
        self.load_library(true, LibraryChange::None);
        cx.notify();
    }

    pub(super) fn close_library(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.model.screen = Screen::Chat;
        self.model.cont.library.new_menu = false;
        self.composer.update(cx, |c, cx| c.focus(window, cx));
        cx.notify();
    }

    /// Asks for the page shown; `sync` rebuilds the index first.
    pub(super) fn load_library(&mut self, sync: bool, change: LibraryChange) {
        let l = &mut self.model.cont.library;
        let request = l.begin();
        let query = l.query();
        self.bridge.library(request, query, sync, change);
    }

    /// The search field changed.
    pub(super) fn library_typed(&mut self, cx: &mut Context<Self>) {
        let text = self.library_input.read(cx).value().to_string();
        if text != self.model.cont.library.text {
            self.model.cont.library.text = text;
            self.model.cont.library.page = 0;
            self.load_library(false, LibraryChange::None);
            cx.notify();
        }
    }

    /// Favicons for the source rows on the page.
    pub(super) fn request_library_icons(&mut self) {
        let wanted: Vec<(String, String)> = self
            .model
            .cont
            .library
            .items
            .iter()
            .filter(|i| i.kind() == LibraryKind::Source)
            .map(source_page)
            .filter(|p| !p.site.is_empty() && !self.model.icons.contains_key(&p.site))
            .map(|p| (p.site.clone(), p.favicon.unwrap_or_default()))
            .collect();
        for (site, url) in wanted {
            if self.model.icons.contains_key(&site) {
                continue;
            }
            self.model.icons.insert(site.clone(), SiteIcon::Loading);
            self.bridge.favicon(site, url);
        }
    }

    fn open_library_item(&mut self, it: LibraryItem, window: &mut Window, cx: &mut Context<Self>) {
        match it.kind() {
            LibraryKind::Session => {
                if let Ok(id) = it.location.parse() {
                    self.model.screen = Screen::Chat;
                    self.open_chat(id, cx);
                }
            }
            LibraryKind::Report => {
                let chat = std::path::Path::new(&it.location)
                    .parent()
                    .and_then(|d| d.file_name())
                    .and_then(|n| n.to_str())
                    .and_then(|n| n.parse().ok());
                if let Some(id) = chat {
                    self.model.screen = Screen::Chat;
                    self.open_chat(id, cx);
                    self.model.set_run_view(crate::model::RunView::Report);
                    self.model.run_panel_open = true;
                }
            }
            LibraryKind::Source => {
                self.model.screen = Screen::Chat;
                self.browse(&it.location, window, cx);
            }
            LibraryKind::Note => {
                let path = it.id.strip_prefix("note:").unwrap_or_default().to_owned();
                self.open_editor(window, cx);
                self.open_note(&path, window, cx);
            }
            LibraryKind::Deck | LibraryKind::File => {
                self.choose_harness(tern_agent::Harness::Study, cx);
                if it.kind() == LibraryKind::Deck {
                    self.model.study.show(StudyView::Deck);
                }
            }
            LibraryKind::Scene => {
                let id = it.id.strip_prefix("scene:").unwrap_or_default();
                let loaded = tern_agent::study::StudyStore::discover()
                    .and_then(|s| tern_agent::explain::load(&s, id));
                match loaded {
                    Ok(scene) => {
                        self.next_explain += 1;
                        let key = self.next_explain;
                        let mut state = crate::explain::ExplainState::new(&scene.concept);
                        state.ready(Ok((scene, None)), Instant::now());
                        self.tabs.open_explain(key, &state.title());
                        self.explains.insert(key, state);
                        self.model.screen = Screen::Chat;
                        self.keys.focus(window, cx);
                    }
                    Err(e) => self.model.cont.library.note = Some(e),
                }
            }
        }
        cx.notify();
    }

    fn library_new(&mut self, what: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        self.model.cont.library.new_menu = false;
        match what {
            "note" => {
                match tern_agent::vault::Vault::open_default()
                    .and_then(|v| v.create("Untitled", "# Untitled\n"))
                {
                    Ok(path) => {
                        self.open_editor(window, cx);
                        self.model.editor.refresh();
                        self.open_note(&path, window, cx);
                    }
                    Err(e) => self.model.cont.library.note = Some(e),
                }
            }
            "folder" => {
                let made = tern_agent::vault::Vault::open_default().and_then(|v| {
                    let name = v.free_name("New folder/Index");
                    v.write(&name, "# Index\n").map(|_| name)
                });
                self.model.cont.library.note = Some(match made {
                    Ok(n) => format!("Made {n} in your vault."),
                    Err(e) => e,
                });
                self.load_library(true, LibraryChange::None);
            }
            "report" => {
                // Reports come from research runs: start one.
                self.close_library(window, cx);
                self.new_chat(window, cx);
            }
            "deck" => {
                self.choose_harness(tern_agent::Harness::Study, cx);
                self.model.study.show(StudyView::Deck);
            }
            _ => {
                // Upload: files become study materials (and Library files).
                let rx = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
                    files: true,
                    directories: false,
                    multiple: true,
                    prompt: None,
                });
                cx.spawn(async move |this, cx| {
                    if let Ok(Ok(Some(paths))) = rx.await {
                        let _ = this.update(cx, |this, cx| {
                            this.model.cont.library.note =
                                Some(format!("Adding {} files to your materials…", paths.len()));
                            this.bridge.study_add(paths);
                            cx.notify();
                        });
                    }
                })
                .detach();
            }
        }
        cx.notify();
    }

    fn library_row(
        &self,
        it: &LibraryItem,
        grid: bool,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = self.model.cont.library.selected.contains(&it.id);
        let id = it.id.clone();
        let open = it.clone();
        let trash = self.model.cont.library.tab == LibraryTab::Trash;
        let logo = if it.kind() == LibraryKind::Source {
            self.site_icon(&source_page(it), 16., p)
        } else {
            gpui_kit::component::Icon::new(kind_icon(it.kind()))
                .small()
                .text_color(p.muted)
                .into_any_element()
        };
        let check = div()
            .id(SharedString::from(format!("lib-check-{id}")))
            .cursor_pointer()
            .child(
                gpui_kit::component::Icon::new(if selected {
                    AssetIcon::SquareCheck
                } else {
                    AssetIcon::Square
                })
                .xsmall()
                .text_color(if selected { p.accent } else { p.faint }),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.model.cont.library.toggle(&id);
                cx.notify();
            }));
        let meta = format!(
            "{} · {}",
            tern_agent::library::size_label(it),
            when_label(if trash {
                it.trashed_at.unwrap_or(it.updated_at)
            } else {
                it.updated_at
            })
        );
        let body = div()
            .id(SharedString::from(format!("lib-item-{}", it.id)))
            .rounded_lg()
            .cursor_pointer()
            .when(selected, |d| d.bg(p.hover))
            .hover(move |d| d.bg(p.hover))
            .when(!trash, |d| {
                d.on_click(cx.listener(move |this, _, window, cx| {
                    this.open_library_item(open.clone(), window, cx)
                }))
            });
        if grid {
            body.w(px(200.))
                .h(px(120.))
                .p_3()
                .flex()
                .flex_col()
                .gap_1()
                .border_1()
                .border_color(p.border)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(logo)
                        .child(check),
                )
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .line_clamp(2)
                        .child(SharedString::from(it.title.clone())),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(p.faint)
                        .truncate()
                        .child(SharedString::from(it.detail.clone())),
                )
                .child(div().mt_auto().text_xs().text_color(p.faint).child(meta))
                .into_any_element()
        } else {
            body.flex()
                .items_center()
                .gap_3()
                .px_2()
                .py_1p5()
                .child(check)
                .child(logo)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .text_sm()
                                .truncate()
                                .child(SharedString::from(it.title.clone())),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(p.faint)
                                .truncate()
                                .child(SharedString::from(it.detail.clone())),
                        ),
                )
                .child(div().text_xs().text_color(p.faint).flex_none().child(meta))
                .into_any_element()
        }
    }

    pub(super) fn library_screen(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let l = &self.model.cont.library;
        let trash = l.tab == LibraryTab::Trash;
        let new_items: [(&'static str, &'static str, AssetIcon); 5] = [
            ("note", "Note", AssetIcon::NotebookPen),
            (
                "report",
                "Report (start a research chat)",
                AssetIcon::ScrollText,
            ),
            ("deck", "Deck", AssetIcon::Layers),
            ("folder", "Folder", AssetIcon::FolderPlus),
            ("upload", "Upload files", AssetIcon::Upload),
        ];
        let header = div()
            .flex()
            .items_center()
            .gap_2()
            .child(serif("Library", p).text_2xl().flex_1())
            .child(
                div()
                    .w(px(260.))
                    .child(Input::new(&self.library_input).small()),
            )
            .child(
                Button::new("lib-sort")
                    .icon(AssetIcon::ArrowDownUp)
                    .label(SharedString::from(format!("Sort: {}", l.sort.label())))
                    .ghost()
                    .small()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.model.cont.library.next_sort();
                        this.load_library(false, LibraryChange::None);
                        cx.notify();
                    })),
            )
            .child(
                Button::new("lib-list")
                    .icon(AssetIcon::List)
                    .ghost()
                    .small()
                    .selected(!l.grid)
                    .tooltip("List")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.model.cont.library.grid = false;
                        cx.notify();
                    })),
            )
            .child(
                Button::new("lib-grid")
                    .icon(AssetIcon::LayoutGrid)
                    .ghost()
                    .small()
                    .selected(l.grid)
                    .tooltip("Grid")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.model.cont.library.grid = true;
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .relative()
                    .child(
                        Button::new("lib-new")
                            .label("New")
                            .icon(AssetIcon::ChevronDown)
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.model.cont.library.new_menu =
                                    !this.model.cont.library.new_menu;
                                cx.notify();
                            })),
                    )
                    .when(l.new_menu, |d| {
                        d.child(
                            gpui_kit::deferred(
                                div()
                                    .id("lib-new-menu")
                                    .absolute()
                                    .top(px(30.))
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
                                    .children(new_items.into_iter().map(|(key, label, ic)| {
                                        div()
                                            .id(SharedString::from(format!("lib-new-{key}")))
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .px_2()
                                            .py_1()
                                            .rounded_md()
                                            .text_sm()
                                            .cursor_pointer()
                                            .hover(move |d| d.bg(p.hover))
                                            .child(
                                                gpui_kit::component::Icon::new(ic)
                                                    .xsmall()
                                                    .text_color(p.muted),
                                            )
                                            .child(label)
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.library_new(key, window, cx)
                                            }))
                                    })),
                            )
                            .with_priority(3),
                        )
                    }),
            )
            .child(
                Button::new("lib-done")
                    .label("Back to chat")
                    .ghost()
                    .small()
                    .on_click(cx.listener(|this, _, window, cx| this.close_library(window, cx))),
            );
        let tabs = div()
            .flex()
            .flex_wrap()
            .gap_1()
            .children(LibraryTab::SHOWN.into_iter().map(|t| {
                let n = l.count(t);
                Button::new(SharedString::from(format!("lib-tab-{}", t.label())))
                    .label(SharedString::from(if n > 0 {
                        format!("{} {n}", t.label())
                    } else {
                        t.label().to_owned()
                    }))
                    .ghost()
                    .small()
                    .selected(l.tab == t)
                    .when(t == LibraryTab::Trash, |b| b.icon(AssetIcon::Trash))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.model.cont.library.set_tab(t);
                        this.load_library(false, LibraryChange::None);
                        cx.notify();
                    }))
            }));
        let picked: Vec<String> = l.selected.iter().cloned().collect();
        let (to_trash, to_restore) = (picked.clone(), picked);
        let all_on_page = !l.items.is_empty() && l.items.iter().all(|i| l.selected.contains(&i.id));
        let bulk = div()
            .flex()
            .items_center()
            .gap_2()
            .min_h(px(28.))
            .text_sm()
            .child(
                div()
                    .id("lib-select-page")
                    .flex()
                    .items_center()
                    .gap_1()
                    .cursor_pointer()
                    .text_color(p.muted)
                    .child(
                        gpui_kit::component::Icon::new(if all_on_page {
                            AssetIcon::SquareCheck
                        } else {
                            AssetIcon::Square
                        })
                        .xsmall(),
                    )
                    .child(if l.selected.is_empty() {
                        "Select page".to_owned()
                    } else {
                        format!("{} selected", l.selected.len())
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.model.cont.library.toggle_all();
                        cx.notify();
                    })),
            )
            .when(!l.selected.is_empty() && !trash, |d| {
                d.child(
                    Button::new("lib-delete")
                        .icon(AssetIcon::Trash)
                        .label("Move to trash")
                        .small()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.model.cont.library.selected.clear();
                            this.load_library(false, LibraryChange::Trash(to_trash.clone()));
                            cx.notify();
                        })),
                )
            })
            .when(!l.selected.is_empty() && trash, |d| {
                d.child(
                    Button::new("lib-restore")
                        .icon(AssetIcon::Undo2)
                        .label("Restore")
                        .small()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.model.cont.library.selected.clear();
                            this.load_library(false, LibraryChange::Restore(to_restore.clone()));
                            cx.notify();
                        })),
                )
            })
            .when(trash && l.trash > 0, |d| {
                d.child(
                    Button::new("lib-empty")
                        .label("Empty trash")
                        .ghost()
                        .small()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.model.cont.library.selected.clear();
                            this.load_library(false, LibraryChange::Empty);
                            cx.notify();
                        })),
                )
            })
            .child(div().flex_1())
            .when(l.loading, |d| d.child(Spinner::new().xsmall()))
            .when_some(l.note.clone(), |d, n| {
                d.child(div().text_xs().text_color(p.faint).child(n))
            });
        let rows: Vec<AnyElement> = l
            .items
            .iter()
            .map(|it| self.library_row(it, l.grid, p, cx))
            .collect();
        let empty = rows.is_empty() && !l.loading;
        let pager = div()
            .flex()
            .items_center()
            .justify_center()
            .gap_2()
            .text_xs()
            .text_color(p.faint)
            .child(
                Button::new("lib-prev")
                    .icon(AssetIcon::ChevronLeft)
                    .ghost()
                    .xsmall()
                    .disabled(l.page == 0)
                    .on_click(cx.listener(|this, _, _, cx| {
                        let l = &mut this.model.cont.library;
                        l.page = l.page.saturating_sub(1);
                        this.load_library(false, LibraryChange::None);
                        cx.notify();
                    })),
            )
            .child(format!(
                "{} items · page {} of {}",
                l.total,
                l.page + 1,
                l.pages()
            ))
            .child(
                Button::new("lib-next")
                    .icon(AssetIcon::ChevronRight)
                    .ghost()
                    .xsmall()
                    .disabled(l.page + 1 >= l.pages())
                    .on_click(cx.listener(|this, _, _, cx| {
                        let l = &mut this.model.cont.library;
                        if l.page + 1 < l.pages() {
                            l.page += 1;
                        }
                        this.load_library(false, LibraryChange::None);
                        cx.notify();
                    })),
            );
        div()
            .id("library-screen")
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap_3()
            .px_6()
            .py_4()
            .child(header)
            .child(tabs)
            .child(bulk)
            .child(
                div()
                    .id("library-items")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .when(empty, |d| {
                        d.child(
                            div()
                                .py_8()
                                .text_sm()
                                .text_color(p.faint)
                                .child(if trash {
                                    "The trash is empty."
                                } else if l.text.is_empty() {
                                    "Nothing here yet. Reports, sources, notes, decks, files, explainers, and sessions show up as you make them."
                                } else {
                                    "Nothing matches."
                                }),
                        )
                    })
                    .child(
                        div()
                            .when(l.grid, |d| d.flex().flex_wrap().gap_3())
                            .when(!l.grid, |d| d.flex().flex_col().gap(px(2.)))
                            .children(rows),
                    ),
            )
            .child(pager)
            .into_any_element()
    }
}
