//! Drawing Scout: the rig's pose, painted as paths in the text colour on a
//! 48×48 grid, scaled to any size. The lantern's flame is the one place
//! the accent appears.

use std::time::{Duration, Instant};

use gpui_kit::prelude::*;
use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, App, Bounds, Context, Hsla, PathBuilder, Pixels,
    Point, SharedString, Window, canvas, div, point, px,
};

use super::Workspace;
use crate::scout::{Mood, Pose};
use crate::theme::Palette;

/// One point on the 48-unit grid, placed in `b`.
fn at(b: Bounds<Pixels>, x: f32, y: f32) -> Point<Pixels> {
    let s = f32::from(b.size.width) / 48.0;
    point(b.origin.x + px(x * s), b.origin.y + px(y * s))
}

fn poly(
    b: Bounds<Pixels>,
    pts: &[(f32, f32)],
    stroke: Option<f32>,
) -> Option<gpui_kit::Path<Pixels>> {
    let s = f32::from(b.size.width) / 48.0;
    let mut pb = match stroke {
        Some(w) => PathBuilder::stroke(px(w * s)),
        None => PathBuilder::fill(),
    };
    let (first, rest) = pts.split_first()?;
    pb.move_to(at(b, first.0, first.1));
    for &(x, y) in rest {
        pb.line_to(at(b, x, y));
    }
    if stroke.is_none() {
        pb.close();
    }
    pb.build().ok()
}

fn ellipse(cx: f32, cy: f32, rx: f32, ry: f32) -> Vec<(f32, f32)> {
    (0..24)
        .map(|i| {
            let a = i as f32 / 24.0 * std::f32::consts::TAU;
            (cx + rx * a.cos(), cy + ry * a.sin())
        })
        .collect()
}

fn rotate(pts: &[(f32, f32)], (ox, oy): (f32, f32), angle: f32) -> Vec<(f32, f32)> {
    let (s, c) = angle.sin_cos();
    pts.iter()
        .map(|&(x, y)| {
            let (dx, dy) = (x - ox, y - oy);
            (ox + dx * c - dy * s, oy + dx * s + dy * c)
        })
        .collect()
}

/// Quadratic curve as points, for the hood's outline.
fn quad(from: (f32, f32), ctrl: (f32, f32), to: (f32, f32), out: &mut Vec<(f32, f32)>) {
    for i in 1..=10 {
        let t = i as f32 / 10.0;
        let u = 1.0 - t;
        out.push((
            u * u * from.0 + 2.0 * u * t * ctrl.0 + t * t * to.0,
            u * u * from.1 + 2.0 * u * t * ctrl.1 + t * t * to.1,
        ));
    }
}

pub(super) fn paint_scout(
    b: Bounds<Pixels>,
    pose: Pose,
    ink: Hsla,
    face: Hsla,
    flame: Hsla,
    window: &mut Window,
) {
    let dy = pose.bob * 0.6 + pose.sit * 4.0;
    let y = |v: f32| v + dy;
    let base = 41.0 - pose.sit * 1.5;
    let spread = pose.sit * 2.0;

    // Hood and cloak, one ink shape.
    let mut hood = vec![(24.0, y(7.0))];
    quad((24.0, y(7.0)), (34.0, y(9.0)), (35.0, y(24.0)), &mut hood);
    quad(
        (35.0, y(24.0)),
        (38.0 + spread, y(32.0)),
        (37.0 + spread, base),
        &mut hood,
    );
    hood.push((11.0 - spread, base));
    quad(
        (11.0 - spread, base),
        (10.0 - spread, y(32.0)),
        (13.0, y(24.0)),
        &mut hood,
    );
    quad((13.0, y(24.0)), (14.0, y(9.0)), (24.0, y(7.0)), &mut hood);
    if let Some(p) = poly(b, &hood, None) {
        window.paint_path(p, ink);
    }

    // The face opening, and the eyes inside it. Turning to face you
    // (needs you) opens the face a little wider and spaces the eyes, the
    // way a head turned square-on reads at this size.
    let f = pose.front;
    let look = (pose.look_x * 1.6, pose.look_y * 1.4);
    if let Some(p) = poly(
        b,
        &ellipse(
            24.0 + look.0 * 0.3 * (1.0 - f),
            y(22.0),
            6.4 + f * 0.6,
            7.0 + f * 0.3,
        ),
        None,
    ) {
        window.paint_path(p, face);
    }
    let h = (2.6 * pose.eyes_open).max(0.45) * (1.0 + f * 0.12);
    for ex in [21.4 - f * 0.5, 26.6 + f * 0.5] {
        let (x0, y0) = (ex + look.0, y(21.6) + look.1);
        let eye = [
            (x0 - 0.85, y0 - h / 2.0),
            (x0 + 0.85, y0 - h / 2.0),
            (x0 + 0.85, y0 + h / 2.0),
            (x0 - 0.85, y0 + h / 2.0),
        ];
        if let Some(p) = poly(b, &eye, None) {
            window.paint_path(p, ink);
        }
    }

    // The lantern arm, from the shoulder to the hand.
    let shoulder = (33.5, y(28.0));
    let r = pose.raise;
    let hand = (38.5 + r * 1.5, y(34.0 - r * 19.0));
    if let Some(p) = poly(b, &[shoulder, hand], Some(1.6)) {
        window.paint_path(p, ink);
    }
    // The lantern hangs from the hand and swings on its spring.
    let hook = (hand.0 + 1.2, hand.1);
    let body = [
        (hook.0, hook.1),
        (hook.0, hook.1 + 2.4),
        (hook.0 - 2.2, hook.1 + 3.0),
        (hook.0 - 2.2, hook.1 + 7.8),
        (hook.0 + 2.2, hook.1 + 7.8),
        (hook.0 + 2.2, hook.1 + 3.0),
        (hook.0, hook.1 + 2.4),
    ];
    let body = rotate(&body, hook, pose.lantern);
    if let Some(p) = poly(b, &body, Some(1.2)) {
        window.paint_path(p, ink);
    }
    if pose.lit {
        // The flame fades up and down with the glow spring; at full glow
        // a soft halo shows the lantern is held up for you.
        let c = rotate(&[(hook.0, hook.1 + 5.4)], hook, pose.lantern)[0];
        let g = pose.glow;
        if g > 0.6
            && let Some(p) = poly(b, &ellipse(c.0, c.1, 3.4 * g, 3.8 * g), None)
        {
            window.paint_path(p, flame.opacity(0.18 * (g - 0.6) / 0.4));
        }
        if let Some(p) = poly(b, &ellipse(c.0, c.1, 0.5 + 0.6 * g, 0.6 + 0.8 * g), None) {
            window.paint_path(p, flame.opacity(g.clamp(0.0, 1.0)));
        }
    }

    // The scroll in the other hand: rolled up, or open to read.
    let w = 2.4 + pose.scroll * 9.0;
    let (sx, sy) = (12.5, y(31.0 + pose.sit));
    let sheet = [
        (sx - w / 2.0, sy - 3.6),
        (sx + w / 2.0, sy - 3.6),
        (sx + w / 2.0, sy + 3.6),
        (sx - w / 2.0, sy + 3.6),
    ];
    if let Some(p) = poly(b, &sheet, None) {
        window.paint_path(p, face);
    }
    let mut outline = sheet.to_vec();
    outline.push(sheet[0]);
    if let Some(p) = poly(b, &outline, Some(1.2)) {
        window.paint_path(p, ink);
    }
    if pose.scroll > 0.5 {
        let len = (w - 3.0) * ((pose.scroll - 0.5) * 2.0);
        for (i, ly) in [-1.6, 0.0, 1.6].into_iter().enumerate() {
            let l = len * [1.0, 0.8, 0.6][i];
            if let Some(p) = poly(
                b,
                &[
                    (sx - w / 2.0 + 1.5, sy + ly),
                    (sx - w / 2.0 + 1.5 + l, sy + ly),
                ],
                Some(0.7),
            ) {
                window.paint_path(p, ink);
            }
        }
    }
}

impl Workspace {
    /// What Scout should be doing now: a preview from Appearance, else the
    /// run's state.
    pub(super) fn scout_mood(&self) -> Mood {
        if self.model.screen == crate::model::Screen::Settings
            && self.model.settings.section == crate::settings::Section::Appearance
            && let Some(m) = self.scout_preview
        {
            return m;
        }
        self.model.scout_mood()
    }

    /// Advances Scout to now and asks for the next frame only while it
    /// moves; at rest it sleeps until its next blink.
    pub(super) fn tick_scout(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let now = Instant::now();
        let dt = self
            .scout_at
            .map(|t| now.duration_since(t).as_secs_f32())
            .unwrap_or(0.0);
        self.scout_at = Some(now);
        let mood = self.scout_mood();
        self.scout.set_mood(mood);
        let beat = self.model.working_line().map(|w| w.beat).unwrap_or(0);
        if beat != self.scout_beat {
            self.scout_beat = beat;
            self.scout.kick();
        }
        if self.reduced_motion {
            // Poses change, nothing moves between them.
            self.scout.step(3.0);
            return;
        }
        self.scout.step(dt);
        // Nothing to draw: Scout's corner is hidden (Settings, or the
        // sidebar is closed), so it never asks for frames. Before this, a
        // busy or drifting Scout redrew the whole window 15 to 60 times a
        // second behind Settings, which is what made its scrolling lag.
        if !self.scout_on_screen() {
            return;
        }
        if self.scout.busy() {
            super::frame::next_frame(window, cx);
        } else if self.scout_wake.is_none() {
            // A drifting mood wakes about 15 times a second; a still one
            // sleeps until its next blink.
            let wait = if self.scout.ambient() {
                Duration::from_millis(66)
            } else {
                Duration::from_secs_f32(self.scout.until_blink() + 0.02)
            };
            self.scout_wake = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(wait).await;
                let _ = this.update(cx, |this, cx| {
                    this.scout_wake = None;
                    cx.notify();
                });
            }));
        }
    }

    /// Whether Scout is drawn anywhere right now.
    pub(super) fn scout_on_screen(&self) -> bool {
        use crate::model::Screen;
        match self.model.screen {
            // Appearance draws Scout as a preview of its moods.
            Screen::Settings => self.model.settings.section == crate::settings::Section::Appearance,
            // The flashcard review draws Scout beside the card.
            Screen::Study if self.model.study.view == crate::study::StudyView::Cards => true,
            _ => self.model.sidebar_open,
        }
    }

    /// The pointer moved: Scout's eyes follow it while it is near (v2's
    /// corner). Only a pointer near Scout redraws anything.
    pub(super) fn scout_pointer(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(b) = self.scout_bounds.get() else {
            return;
        };
        if !self.scout_on_screen() {
            return;
        }
        let was = self.scout.watching();
        let face = point(
            b.origin.x + b.size.width * 0.5,
            b.origin.y + b.size.height * 0.45,
        );
        self.scout
            .set_pointer(Some((f32::from(at.x - face.x), f32::from(at.y - face.y))));
        if was || self.scout.watching() {
            cx.notify();
        }
    }

    /// Scout at `size` px, posed as of the last tick.
    pub(super) fn scout_figure(&self, size: f32, p: Palette) -> AnyElement {
        let pose = self.scout.pose();
        let (ink, face, flame) = (p.text, p.sidebar, p.accent);
        let seen = self.scout_bounds.clone();
        div()
            .size(px(size))
            .flex_none()
            .child(
                canvas(
                    |_, _, _: &mut App| (),
                    move |bounds, _, window, _: &mut App| {
                        seen.set(Some(bounds));
                        paint_scout(bounds, pose, ink, face, flame, window)
                    },
                )
                .size_full(),
            )
            .into_any_element()
    }

    /// Scout's corner at the foot of the chats sidebar. Nothing is printed
    /// under it; the pose carries the state. The tooltip names it.
    pub(super) fn scout_corner(&self, p: Palette) -> AnyElement {
        let mood = self.scout.mood();
        let state = crate::scout::state_line(mood);
        div()
            .id("scout")
            .px_3()
            .py_2()
            .flex()
            .items_end()
            .gap_2()
            .child(self.scout_figure(crate::scout::CORNER, p))
            // What Scout is doing, in a word, beside it (v2's corner). It
            // cross-fades when the state changes; idle says nothing.
            .when_some(state, |d, line| {
                d.child(
                    div()
                        .pb_1()
                        .text_xs()
                        .font_family(crate::assets::MONO_FONT)
                        .text_color(if mood == crate::scout::Mood::NeedsYou {
                            p.accent
                        } else {
                            p.faint
                        })
                        .child(line)
                        .with_animation(
                            SharedString::from(format!("scout-state-{line}")),
                            Animation::new(if self.reduced_motion {
                                Duration::ZERO
                            } else {
                                crate::motion::UI
                            })
                            .with_easing(crate::motion::ease),
                            |d, t| d.opacity(t),
                        ),
                )
            })
            .tooltip({
                let label = self.scout.mood().label();
                move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(format!("Scout · {label}"))
                        .build(window, cx)
                }
            })
            .into_any_element()
    }
}
