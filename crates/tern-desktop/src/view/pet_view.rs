//! Scout as a floating pet over the whole window (a Codex-style
//! companion): the clay sprite from `crate::pet`, a soft shadow under the
//! feet, a speech bubble with what Tern is doing, a hover toolbar (new
//! chat, dictate, menu), dragging anywhere with the spot remembered, and
//! Hide/Show (Ctrl+. / Cmd+.).
//!
//! The pet is its own view beside the workspace in [`Shell`], and the
//! workspace is drawn as a cached view: Scout's frames (8 to 12 a second)
//! redraw only Scout, never the chat, Settings or the panels behind it.
//! Scout reads what it shows from the workspace ([`Workspace::pet_snapshot`]),
//! so it re-renders when the workspace changes and on its own clock.

use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::AnimationExt as _;
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, Context, Entity, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ObjectFit, Point, SharedString, StyleRefinement, Task, WeakEntity, Window, div, img, px,
};

use super::Workspace;
use crate::pet::{Animator, Frame, Manifest, State};
use crate::theme::Palette;

/// The window's root: the workspace (cached) with Scout floating over it.
pub struct Shell {
    workspace: Entity<Workspace>,
    pet: Entity<Pet>,
}

impl Shell {
    pub fn new(workspace: Entity<Workspace>, cx: &mut Context<Self>) -> Self {
        let pet = cx.new(|cx| {
            // Scout redraws when the workspace changes (a new state, a
            // hide, a preview) as well as on its own clock.
            cx.observe(&workspace, |_, _, cx| cx.notify()).detach();
            Pet::new(workspace.downgrade())
        });
        Self { workspace, pet }
    }
}

impl Render for Shell {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let mut full = StyleRefinement::default();
        full.size.width = Some(gpui_kit::relative(1.).into());
        full.size.height = Some(gpui_kit::relative(1.).into());
        div()
            .relative()
            .size_full()
            .child(self.workspace.clone().cached(full))
            .child(self.pet.clone())
    }
}

/// What the workspace tells Scout each time it draws.
#[derive(Clone, Debug)]
pub struct PetSnapshot {
    pub state: State,
    pub bubble: Option<String>,
    pub hidden: bool,
    pub variant: String,
    pub palette: Palette,
    pub reduced: bool,
    pub saved_pos: Option<(f32, f32)>,
    pub recording: bool,
}

pub struct Pet {
    workspace: WeakEntity<Workspace>,
    manifest: Rc<Manifest>,
    anim: Animator,
    /// Top-left in window px; `None` until placed (bottom-left corner).
    pos: Option<(f32, f32)>,
    /// While dragging: the press point's offset into the pet, and whether
    /// it has moved far enough to count as a drag (else it is a click).
    drag: Option<(Point<gpui_kit::Pixels>, bool)>,
    hover: bool,
    menu: bool,
    wake: Option<Task<()>>,
    /// When Scout last turned to Done; it settles to idle after a beat.
    done_at: Option<Instant>,
}

/// The pet's box: the sprite plus room for the shadow.
fn pet_size(m: &Manifest) -> (f32, f32) {
    let h = crate::pet::PET_H;
    (h * m.frame_w / m.frame_h, h + 6.)
}

impl Pet {
    fn new(workspace: WeakEntity<Workspace>) -> Self {
        Self {
            workspace,
            manifest: Rc::new(Manifest::embedded()),
            anim: Animator::new(Instant::now()),
            pos: None,
            drag: None,
            hover: false,
            menu: false,
            wake: None,
            done_at: None,
        }
    }

    fn snapshot(&self, cx: &mut Context<Self>) -> Option<PetSnapshot> {
        let ws = self.workspace.upgrade()?;
        Some(ws.read(cx).pet_snapshot())
    }

    fn with_workspace(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Workspace, &mut Window, &mut Context<Workspace>),
    ) {
        if let Some(ws) = self.workspace.upgrade() {
            ws.update(cx, |w, cx| f(w, window, cx));
        }
    }

    /// Asks for the next drawing when the animation next changes, on a
    /// timer (not every frame) unless a fade or jiggle is running.
    fn schedule(&mut self, reduced: bool, window: &Window, cx: &mut Context<Self>) {
        let Some(wait) = self.anim.next_change(&self.manifest, reduced) else {
            return;
        };
        if wait <= Duration::from_millis(16) {
            super::frame::next_frame(window, cx);
        } else if self.wake.is_none() {
            self.wake = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(wait).await;
                let _ = this.update(cx, |this, cx| {
                    this.wake = None;
                    cx.notify();
                });
            }));
        }
    }

    fn drag_move(&mut self, at: Point<gpui_kit::Pixels>, window: &Window, cx: &mut Context<Self>) {
        let Some((grab, moved)) = self.drag else {
            return;
        };
        let (w, h) = pet_size(&self.manifest);
        let vp = window.viewport_size();
        let p = crate::pet::clamp_pos(
            (f32::from(at.x - grab.x), f32::from(at.y - grab.y)),
            (w, h),
            (f32::from(vp.width), f32::from(vp.height)),
        );
        let was = self.pos;
        self.pos = Some(p);
        if !moved && was.is_some_and(|w| (w.0 - p.0).abs() + (w.1 - p.1).abs() > 3.) {
            self.drag = Some((grab, true));
        }
        cx.notify();
    }

    fn drag_end(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, moved)) = self.drag.take() else {
            return;
        };
        self.anim.poke(Instant::now());
        if moved && let Some((x, y)) = self.pos {
            self.with_workspace(window, cx, |w, _, _| {
                w.save_ui_prefs(|p| p.scout_pos = Some((x.round() as i32, y.round() as i32)));
            });
        }
        cx.notify();
    }
}

/// One frame of a strip at `h` px tall, squashed by `squash` (wider and
/// shorter, anchored at the feet) and faded to `alpha`.
pub(super) fn sprite(
    m: &Manifest,
    variant: &str,
    state: State,
    index: u32,
    h: f32,
    squash: f32,
    alpha: f32,
) -> AnyElement {
    let strip = m.strip(state);
    // Big drawings (Settings › Scout) use the large strips.
    let path = m.sized(&strip.file, variant, h > 100.);
    strip_frame(
        path,
        strip.frames,
        index,
        h * m.frame_w / m.frame_h,
        h,
        squash,
        alpha,
    )
}

/// The light rim behind Scout on dark themes, in step with its frame.
fn rim(
    m: &Manifest,
    state: State,
    index: u32,
    h: f32,
    squash: f32,
    alpha: f32,
) -> Option<AnyElement> {
    let strip = m.strip(state);
    let path = m.rim(&strip.file, h > 100.)?;
    Some(strip_frame(
        path,
        strip.frames,
        index,
        h * m.frame_w / m.frame_h,
        h,
        squash,
        alpha,
    ))
}

/// Frame `index` of a horizontal strip, `w`×`h`, squashed at the feet.
fn strip_frame(
    path: String,
    frames: u32,
    index: u32,
    w: f32,
    h: f32,
    squash: f32,
    alpha: f32,
) -> AnyElement {
    let (sw, sh) = (w * (1. + squash), h * (1. - squash * 0.8));
    let path = SharedString::from(path);
    let strip_frames = frames;
    div()
        .absolute()
        .bottom_0()
        .left(px((w - sw) / 2.))
        .w(px(sw))
        .h(px(sh))
        .overflow_hidden()
        .opacity(alpha)
        .child(
            img(path)
                .absolute()
                .top_0()
                .left(px(-(index as f32) * sw))
                .w(px(sw * strip_frames as f32))
                .h(px(sh))
                .object_fit(ObjectFit::Fill),
        )
        .into_any_element()
}

/// Scout drawn whole at `h` px: crossfading strips, bob, squash and the
/// shadow under the feet. Used by the pet and by Settings › Scout.
pub(super) fn figure(m: &Manifest, variant: &str, f: &Frame, h: f32, dark: bool) -> AnyElement {
    let w = h * m.frame_w / m.frame_h;
    let scale = h / m.frame_h;
    // Feet sit on the baseline; the frame's foot room goes under it.
    let foot = (m.frame_h - m.strip(f.state).baseline) * scale;
    let lift = f.lift * h / crate::pet::PET_H;
    // The shadow shrinks and fades as Scout rises (Codex's pet shadow).
    let k = (lift / 4.).clamp(0., 1.);
    let sh_w = w * 0.62 * (1. - 0.18 * k);
    let ink = if dark { 0.0 } else { 0.1 };
    let shadow = |grow: f32, a: f32| {
        div()
            .absolute()
            .bottom(px(foot - 3. - grow / 2.))
            .left(px((w - sh_w - grow) / 2.))
            .w(px(sh_w + grow))
            .h(px(7. + grow / 2.))
            .rounded_full()
            .bg(gpui_kit::hsla(0., 0., ink, a * (1. - 0.35 * k)))
    };
    let body = div()
        .absolute()
        .left_0()
        .bottom(px(lift))
        .w(px(w))
        .h(px(h))
        // On dark themes a light rim sits behind each drawing.
        .when(dark, |d| {
            d.children(
                f.from
                    .and_then(|(s, i)| rim(m, s, i, h, f.squash, 1. - f.mix)),
            )
            .children(rim(m, f.state, f.index, h, f.squash, f.mix.max(0.05)))
        })
        .children(
            f.from
                .map(|(s, i)| sprite(m, variant, s, i, h, f.squash, 1. - f.mix)),
        )
        .child(sprite(
            m,
            variant,
            f.state,
            f.index,
            h,
            f.squash,
            f.mix.max(0.05),
        ))
        // The state's flag (a ?, a ✓, a !) drawn over the frame.
        .children(m.strip(f.state).overlay.as_ref().map(|o| {
            img(SharedString::from(m.sized(o, "normal", h > 100.)))
                .absolute()
                .top_0()
                .left_0()
                .w(px(w))
                .h(px(h))
                .opacity(f.mix)
                .object_fit(ObjectFit::Fill)
        }));
    div()
        .relative()
        .w(px(w))
        .h(px(h))
        .flex_none()
        .child(shadow(10., if dark { 0.16 } else { 0.08 }))
        .child(shadow(4., if dark { 0.22 } else { 0.12 }))
        .child(shadow(0., if dark { 0.3 } else { 0.16 }))
        .child(body)
        .into_any_element()
}

impl Render for Pet {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(snap) = self.snapshot(cx) else {
            return div().into_any_element();
        };
        if snap.hidden {
            self.menu = false;
            return div().into_any_element();
        }
        let p = snap.palette;
        let now = Instant::now();
        let mut state = snap.state;
        if state == State::Done {
            let at = *self.done_at.get_or_insert(now);
            if now.saturating_duration_since(at).as_secs_f32() > crate::scout::READY_SECS + 0.6 {
                state = State::Idle;
            }
        } else {
            self.done_at = None;
        }
        let mut snap = snap;
        if state != snap.state {
            snap.state = state;
            snap.bubble = None;
        }
        self.anim.set(snap.state, now);
        let f = self.anim.frame(&self.manifest, now, snap.reduced);
        self.schedule(snap.reduced, window, cx);
        let (w, h) = pet_size(&self.manifest);
        let vp = window.viewport_size();
        let vp = (f32::from(vp.width), f32::from(vp.height));
        let pos = self
            .pos
            .or(snap.saved_pos)
            .unwrap_or((56. + 12., vp.1 - h - 18.));
        let pos = crate::pet::clamp_pos(pos, (w, h), vp);
        let dragging = self.drag.is_some_and(|d| d.1);
        let fig = figure(
            &self.manifest,
            &snap.variant,
            &f,
            crate::pet::PET_H,
            p.canvas.l < 0.5,
        );
        // The bubble: a capsule above Scout's head, to the side with room.
        let to_left = pos.0 > vp.0 - 260.;
        let bubble = snap.bubble.clone().filter(|_| !dragging).map(|text| {
            let needs = snap.state == State::NeedsYou;
            div()
                .absolute()
                .bottom(px(h - 6.))
                .when(to_left, |d| d.right(px(w * 0.45)))
                .when(!to_left, |d| d.left(px(w * 0.55)))
                .max_w(px(240.))
                .px_2p5()
                .py_1()
                .rounded_full()
                .border_1()
                .border_color(if needs { p.accent } else { p.border })
                .bg(p.raised)
                .shadow_md()
                .text_xs()
                .text_color(if needs { p.text } else { p.muted })
                .whitespace_nowrap()
                .truncate()
                .child(SharedString::from(text))
                .with_animation(
                    SharedString::from(format!("pet-bubble-{:?}", snap.state)),
                    gpui_kit::Animation::new(if snap.reduced {
                        Duration::ZERO
                    } else {
                        crate::motion::UI
                    })
                    .with_easing(crate::motion::ease),
                    |d, t| d.opacity(t).mb(px((1. - t) * 4.)),
                )
        });
        let show_tools = (self.hover || self.menu) && !dragging;
        let tool = |id: &'static str, icon: gpui_kit::component::Icon, tip: &'static str| {
            div()
                .id(id)
                .size(px(26.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .border_1()
                .border_color(p.border)
                .bg(p.raised)
                .shadow_sm()
                .cursor_pointer()
                .text_color(p.muted)
                .hover(move |d| d.text_color(p.text).bg(p.hover))
                .child(icon.size(px(13.)))
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(tip).build(window, cx)
                })
        };
        let recording = snap.recording;
        let toolbar = show_tools.then(|| {
            div()
                .absolute()
                .top(px(-30.))
                .left(px((w - 3. * 26. - 2. * 4.) / 2.))
                .flex()
                .gap_1()
                .occlude()
                .child(
                    tool(
                        "pet-new-chat",
                        gpui_kit::component::Icon::new(gpui_kit::component::IconName::Plus),
                        "New chat",
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        cx.stop_propagation();
                        this.with_workspace(window, cx, |w, window, cx| w.pet_new_chat(window, cx));
                    })),
                )
                .child(
                    tool(
                        "pet-dictate",
                        gpui_kit::component::Icon::new(if recording {
                            gpui_kit::assets::IconName::CircleStop
                        } else {
                            gpui_kit::assets::IconName::Mic
                        }),
                        "Dictate",
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        cx.stop_propagation();
                        this.with_workspace(window, cx, |w, window, cx| w.pet_dictate(window, cx));
                    })),
                )
                .child(
                    tool(
                        "pet-menu",
                        gpui_kit::component::Icon::new(gpui_kit::component::IconName::Ellipsis),
                        "Scout",
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.menu = !this.menu;
                        cx.notify();
                    })),
                )
        });
        let menu = self.menu.then(|| {
            let item = |id: &'static str, label: &'static str| {
                div()
                    .id(id)
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .text_sm()
                    .text_color(p.text)
                    .cursor_pointer()
                    .hover(move |d| d.bg(p.hover))
                    .child(label)
            };
            gpui_kit::deferred(
                div()
                    .id("pet-menu-list")
                    .absolute()
                    .when(pos.1 > 220., |d| d.bottom(px(h + 34.)))
                    .when(pos.1 <= 220., |d| d.top(px(h + 4.)))
                    .left_0()
                    .w(px(200.))
                    .p_1()
                    .flex()
                    .flex_col()
                    .rounded_xl()
                    .border_1()
                    .border_color(p.border)
                    .bg(p.raised)
                    .shadow_lg()
                    .occlude()
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.menu = false;
                        cx.notify();
                    }))
                    .child(
                        item("pet-hide", "Hide Scout   Ctrl+.").on_click(cx.listener(
                            |this, _, window, cx| {
                                this.menu = false;
                                this.with_workspace(window, cx, |w, _, cx| {
                                    w.set_scout_hidden(true, cx)
                                });
                            },
                        )),
                    )
                    .child(
                        item("pet-settings", "Scout settings…").on_click(cx.listener(
                            |this, _, window, cx| {
                                this.menu = false;
                                this.with_workspace(window, cx, |w, window, cx| {
                                    w.open_settings(crate::settings::Section::Scout, window, cx)
                                });
                            },
                        )),
                    )
                    .child(item("pet-home", "Back to the corner").on_click(cx.listener(
                        |this, _, window, cx| {
                            this.menu = false;
                            this.pos = None;
                            this.with_workspace(window, cx, |w, _, _| {
                                w.save_ui_prefs(|p| p.scout_pos = None)
                            });
                            cx.notify();
                        },
                    ))),
            )
            .with_priority(4)
        });
        let pet = div()
            .id("scout-pet")
            .absolute()
            .left(px(pos.0))
            .top(px(pos.1))
            .w(px(w))
            .h(px(h))
            .cursor(if dragging {
                gpui_kit::CursorStyle::ClosedHand
            } else {
                gpui_kit::CursorStyle::OpenHand
            })
            .on_hover(cx.listener(|this, on: &bool, _, cx| {
                this.hover = *on;
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                    let grab = e.position - gpui_kit::point(px(pos.0), px(pos.1));
                    this.pos = Some(pos);
                    this.drag = Some((grab, false));
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .tooltip({
                let label = snap.state.label();
                move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(format!("Scout · {label}"))
                        .build(window, cx)
                }
            })
            .child(fig)
            .children(bubble)
            .children(toolbar)
            .children(menu);
        // While dragging, a full-window layer follows the pointer and ends
        // the drag wherever the button comes up.
        let catcher = self.drag.is_some().then(|| {
            div()
                .id("scout-drag")
                .absolute()
                .inset_0()
                .occlude()
                .cursor(gpui_kit::CursorStyle::ClosedHand)
                .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, window, cx| {
                    this.drag_move(e.position, window, cx);
                }))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseUpEvent, window, cx| this.drag_end(window, cx)),
                )
        });
        div()
            .absolute()
            .inset_0()
            .children(catcher)
            .child(pet)
            .into_any_element()
    }
}

impl Workspace {
    /// What Scout shows: its state, its bubble, and where and how.
    pub(crate) fn pet_snapshot(&self) -> PetSnapshot {
        // A state picked on Settings › Scout (or by the tour) shows on the
        // floating Scout too; otherwise the rig's mood decides.
        let state = self
            .pet_preview
            .unwrap_or_else(|| State::of(self.scout.mood()));
        let prefs = &self.model.providers.prefs;
        let study = self.model.harness == tern_agent::Harness::Study;
        let variant = self
            .scout_variant
            .clone()
            .unwrap_or_else(|| if study { "study" } else { "normal" }.to_owned());
        let bubble = match state {
            State::NeedsYou => Some("needs you".to_owned()),
            State::Done => Some("done".to_owned()),
            _ => match self.model.working_line() {
                Some(w) if !w.phrase.trim().is_empty() => Some(lower_first(&w.phrase)),
                _ => crate::pet::bubble(state, 0, None),
            },
        };
        PetSnapshot {
            state,
            bubble,
            hidden: self.scout_hidden,
            variant,
            palette: crate::theme::Palette::get(self.model.dark),
            reduced: self.reduced_motion,
            saved_pos: prefs.scout_pos.map(|(x, y)| (x as f32, y as f32)),
            recording: self.recording.is_some(),
        }
    }

    /// Scout standing still at `h` px in its Study pose, inside a page
    /// (beside the flashcards); the floating pet is the animated one.
    pub(super) fn pet_figure(&self, h: f32, p: Palette) -> AnyElement {
        let f = Frame {
            state: State::Study,
            index: 0,
            from: None,
            mix: 1.,
            lift: 0.,
            squash: 0.,
        };
        figure(
            &self.pet_manifest,
            &self.pet_snapshot().variant,
            &f,
            h,
            p.canvas.l < 0.5,
        )
    }

    /// Ctrl+. (Cmd+. on macOS) hides or shows Scout.
    pub(super) fn scout_keys(
        &mut self,
        e: &gpui_kit::KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let k = &e.keystroke;
        let m = &k.modifiers;
        if !crate::pet::is_toggle_key(&k.key, m.control, m.platform, m.alt, m.shift) {
            return false;
        }
        self.set_scout_hidden(!self.scout_hidden, cx);
        cx.stop_propagation();
        true
    }

    pub(super) fn pet_new_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.new_chat(window, cx);
    }

    pub(super) fn pet_dictate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.model.screen != crate::model::Screen::Chat {
            self.model.screen = crate::model::Screen::Chat;
        }
        self.toggle_mic(window, cx);
    }

    /// Hides or shows Scout, and remembers it.
    pub(super) fn set_scout_hidden(&mut self, hidden: bool, cx: &mut Context<Self>) {
        self.scout_hidden = hidden;
        self.save_ui_prefs(|p| p.scout_hidden = hidden);
        self.model.toasts.info(
            if hidden {
                "Scout is hidden. Ctrl+. brings it back."
            } else {
                "Scout is back."
            },
            Instant::now(),
        );
        cx.notify();
    }

    /// Scout's look: `None` follows the harness.
    pub(super) fn set_scout_variant(&mut self, v: Option<String>, cx: &mut Context<Self>) {
        self.scout_variant = v.clone();
        self.save_ui_prefs(|p| p.scout_variant = v.clone());
        cx.notify();
    }
}

fn lower_first(s: &str) -> String {
    let s = s.trim();
    let mut c = s.chars();
    let out = match c.next() {
        Some(f) => f.to_lowercase().collect::<String>() + c.as_str(),
        None => String::new(),
    };
    if out.chars().count() > 40 {
        format!("{}…", out.chars().take(39).collect::<String>().trim_end())
    } else {
        out
    }
}
