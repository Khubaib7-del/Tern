//! Drawing Scout: the rig's pose, painted as paths in the text colour on a
//! 48×48 grid, scaled to any size. The lantern's flame is the one place
//! the accent appears.

use std::time::{Duration, Instant};

use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, Bounds, Context, Hsla, PathBuilder, Pixels, Point, Window, canvas, div, point,
    px,
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

    // The face opening, and the eyes inside it.
    let look = (pose.look_x * 1.6, pose.look_y * 1.4);
    if let Some(p) = poly(b, &ellipse(24.0 + look.0 * 0.3, y(22.0), 6.4, 7.0), None) {
        window.paint_path(p, face);
    }
    let h = (2.6 * pose.eyes_open).max(0.45);
    for ex in [21.4, 26.6] {
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
        let c = rotate(&[(hook.0, hook.1 + 5.4)], hook, pose.lantern)[0];
        if let Some(p) = poly(b, &ellipse(c.0, c.1, 1.1, 1.4), None) {
            window.paint_path(p, flame);
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
        if self.scout.busy() {
            window.request_animation_frame();
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

    /// Scout at `size` px, posed as of the last tick.
    pub(super) fn scout_figure(&self, size: f32, p: Palette) -> AnyElement {
        let pose = self.scout.pose();
        let (ink, face, flame) = (p.text, p.sidebar, p.accent);
        div()
            .size(px(size))
            .flex_none()
            .child(
                canvas(
                    |_, _, _: &mut App| (),
                    move |bounds, _, window, _: &mut App| {
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
        div()
            .id("scout")
            .px_3()
            .py_2()
            .flex()
            .items_end()
            .child(self.scout_figure(40., p))
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
