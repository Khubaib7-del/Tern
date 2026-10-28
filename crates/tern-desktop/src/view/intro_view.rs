//! Draws the first-run intro (#76) from `crate::intro`'s timeline: the
//! dithered field (one canvas, a quad per lit cell, so nothing is laid
//! out per cell), the flying tern from the rig's frame sheet, the metal
//! turn, and the TERN lockup revealed by a wipe. It covers the window,
//! takes every click (there is no skip) and follows the cursor.

use std::time::Instant;

use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, Bounds, Context, MouseMoveEvent, SharedString, Window, canvas, div, hsla, img,
    point, px, size, svg,
};

use super::Workspace;
use crate::intro;

/// The intro's floor and ink: always dark, like the launch teaser.
const FLOOR: u32 = 0x0A0A0A;
/// A background cell, in px.
const CELL: f32 = 10.;
/// The sprite sheets (`assets/mark`, served under `tern/mark/`).
const BIRD_SHEET: &str = "tern/mark/intro_bird_dark.png";
const METAL_SHEET: &str = "tern/mark/intro_metal.png";
const WORD: &str = "tern/mark/tern-word.svg";
/// The TERN wordmark's own width / height (`tern-word.svg`).
const WORD_ASPECT: f32 = 1076. / 252.;
/// A frame of the rig: 128x102 cells.
const BIRD_ASPECT: f32 = 128. / 102.;

impl Workspace {
    /// At launch, once: starts the intro instead of onboarding when it has
    /// not played yet. Returns whether it started (onboarding then waits
    /// for it). The tour and reduced motion never play it on their own.
    pub(super) fn start_intro_once(&mut self) -> bool {
        if self.model.providers.prefs.intro_seen || super::tour::enabled() {
            return false;
        }
        if self.reduced_motion {
            self.save_ui_prefs(|p| p.intro_seen = true);
            return false;
        }
        tern_agent::diag::note("intro: first launch, playing the intro");
        self.intro = Some(Instant::now());
        true
    }

    /// Called at the top of every render: ends the intro when its time is
    /// up (marks it seen, then onboarding gets its turn) and otherwise
    /// asks for the next frame.
    pub(super) fn tick_intro(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(start) = self.intro else {
            return;
        };
        if intro::playing(start.elapsed().as_secs_f32()) {
            super::frame::next_frame(window, cx);
            return;
        }
        self.intro = None;
        self.intro_cursor = None;
        if !super::tour::enabled() {
            self.save_ui_prefs(|p| p.intro_seen = true);
            self.onboarding_checked = false;
            self.consider_onboarding(window, cx);
        }
        cx.notify();
    }

    /// The intro over everything, while it plays.
    pub(super) fn intro_overlay(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let start = self.intro?;
        let t = start.elapsed().as_secs_f32();
        let pose = intro::pose(t);
        let vs = window.viewport_size();
        let (w, h) = (f32::from(vs.width).max(1.), f32::from(vs.height).max(1.));
        // The settled bird, and the lockup it becomes part of.
        let bh = (w.min(h) * 0.26).clamp(110., 250.);
        let bw = bh * BIRD_ASPECT;
        let th = bh * 0.34;
        let tw = th * WORD_ASPECT;
        let gap = bh * 0.14;
        let lock_w = bw + gap + tw;
        let rest_cx = w / 2. - lock_w / 2. + bw / 2.;
        let cx0 = w / 2. + pose.bird.0 * bh + (rest_cx - w / 2.) * pose.slide;
        let cy0 = h / 2. + pose.bird.1 * bh;
        let (sw, sh) = (bw * pose.scale, bh * pose.scale);
        let ink = gpui_kit::Hsla::from(gpui_kit::rgb(0xEDEDED));
        let floor = gpui_kit::Hsla::from(gpui_kit::rgb(FLOOR));

        let cursor = self.intro_cursor;
        let (reveal, field) = (pose.reveal, pose.field);
        let background = canvas(
            |_, _, _: &mut gpui_kit::App| (),
            move |b, _, window, _: &mut gpui_kit::App| {
                let (bw, bh) = (f32::from(b.size.width), f32::from(b.size.height));
                let aspect = bw / bh.max(1.);
                let (cols, rows) = ((bw / CELL).ceil() as i32, (bh / CELL).ceil() as i32);
                for iy in 0..rows {
                    let y = (iy as f32 + 0.5) * CELL / bh;
                    for ix in 0..cols {
                        let x = (ix as f32 + 0.5) * CELL / bw;
                        let r = intro::revealed(x, y, reveal, aspect);
                        if r <= 0. {
                            continue;
                        }
                        let v = intro::field(x, y, t, cursor, aspect) * field * r;
                        if v <= intro::bayer(ix, iy) {
                            continue;
                        }
                        let lit = cursor.map_or(0., |(cx, cy)| {
                            let d2 = ((x - cx) * aspect).powi(2) + (y - cy).powi(2);
                            (-d2 / 0.02).exp()
                        });
                        let origin = point(
                            b.origin.x + px(ix as f32 * CELL),
                            b.origin.y + px(iy as f32 * CELL),
                        );
                        window.paint_quad(gpui_kit::fill(
                            Bounds::new(origin, size(px(CELL - 2.), px(CELL - 2.))),
                            hsla(0., 0., 0.93, (0.13 + 0.30 * lit) * field),
                        ));
                    }
                }
            },
        )
        .absolute()
        .inset_0();

        let mut stage = div()
            .id("intro")
            .absolute()
            .inset_0()
            .bg(floor)
            .occlude()
            .opacity(pose.alpha)
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, window, cx| {
                let vs = window.viewport_size();
                let (w, h) = (f32::from(vs.width).max(1.), f32::from(vs.height).max(1.));
                this.intro_cursor =
                    Some((f32::from(e.position.x) / w, f32::from(e.position.y) / h));
                cx.notify();
            }))
            .child(background);

        // The dithered bird: one frame of the 12 x 6 sheet.
        if let Some(f) = pose.frame {
            let (col, row) = ((f % 12) as f32, (f / 12) as f32);
            stage = stage.child(
                div()
                    .absolute()
                    .left(px(cx0 - sw / 2.))
                    .top(px(cy0 - sh / 2.))
                    .w(px(sw))
                    .h(px(sh))
                    .overflow_hidden()
                    .child(
                        img(SharedString::from(BIRD_SHEET))
                            .absolute()
                            .left(px(-col * sw))
                            .top(px(-row * sh))
                            .w(px(sw * 12.))
                            .h(px(sh * 6.))
                            .object_fit(gpui_kit::ObjectFit::Fill),
                    ),
            );
        }
        // Metal, then white: one frame of the 4 x 3 sheet over it.
        if let Some((i, a)) = pose.metal {
            let (col, row) = ((i % 4) as f32, (i / 4) as f32);
            stage = stage.child(
                div()
                    .absolute()
                    .left(px(cx0 - sw / 2.))
                    .top(px(cy0 - sh / 2.))
                    .w(px(sw))
                    .h(px(sh))
                    .overflow_hidden()
                    .opacity(a)
                    .child(
                        img(SharedString::from(METAL_SHEET))
                            .absolute()
                            .left(px(-col * sw))
                            .top(px(-row * sh))
                            .w(px(sw * 4.))
                            .h(px(sh * 3.))
                            .object_fit(gpui_kit::ObjectFit::Fill),
                    ),
            );
        }
        // TERN, revealed left to right by the wipe, with its bright edge.
        if pose.wipe > 0. {
            let x0 = rest_cx + bw / 2. + gap;
            let top = h / 2. - th * 0.78;
            stage = stage
                .child(
                    div()
                        .absolute()
                        .left(px(x0))
                        .top(px(top))
                        .w(px(tw * pose.wipe))
                        .h(px(th))
                        .overflow_hidden()
                        .child(
                            svg()
                                .path(WORD)
                                .absolute()
                                .left_0()
                                .top_0()
                                .w(px(tw))
                                .h(px(th))
                                .text_color(ink),
                        ),
                )
                .when(pose.wipe < 1., |d| {
                    d.child(
                        div()
                            .absolute()
                            .left(px(x0 + tw * pose.wipe - 1.))
                            .top(px(top - th * 0.12))
                            .w(px(2.))
                            .h(px(th * 1.24))
                            .bg(ink)
                            .opacity((1. - pose.wipe).min(pose.wipe * 6.).min(1.)),
                    )
                })
                .when(pose.subtitle > 0., |d| {
                    d.child(
                        div()
                            .absolute()
                            .left(px(x0 + th * 0.04))
                            .top(px(top + th * 1.12 + (1. - pose.subtitle) * 8.))
                            .opacity(pose.subtitle)
                            .font_family(crate::assets::SERIF_FONT)
                            .italic()
                            .text_size(px((th * 0.46).max(14.)))
                            .text_color(hsla(0., 0., 0.93, 0.78))
                            .child("Research · Study"),
                    )
                });
        }
        Some(stage.into_any_element())
    }
}
