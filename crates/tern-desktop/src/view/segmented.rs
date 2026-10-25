//! The sliding segmented control: a row of options with one highlight
//! pill that glides to the picked option (v2 s16's segmented control)
//! instead of swapping at once. Timing is `motion::UI` on the house curve;
//! reduced motion moves it at once.
//!
//! Widths come from `panels::segment_width`, so the pill's place is known
//! without measuring the drawn row. A `fill` control shares its width
//! equally instead, and the pill moves by fractions of it.

use std::rc::Rc;
use std::time::Instant;

use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, SharedString, Window, div, px, relative, svg};

use super::Workspace;
use crate::motion::Glide;
use crate::panels::{SEGMENT_PAD, segment_width};
use crate::theme::Palette;

/// One option: its label, and a glyph drawn before it.
#[derive(Clone)]
pub(super) struct Segment {
    pub label: SharedString,
    pub glyph: Option<SharedString>,
}

impl Segment {
    pub fn text(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            glyph: None,
        }
    }

    fn width(&self) -> f32 {
        segment_width(&self.label) + if self.glyph.is_some() { 18. } else { 0. }
    }
}

/// The pill's left edge and width at fractional position `at` among
/// segments of `widths` (an exact index lands on that segment).
pub(crate) fn pill_at(widths: &[f32], at: f32) -> (f32, f32) {
    if widths.is_empty() {
        return (0., 0.);
    }
    let last = widths.len() - 1;
    let at = at.clamp(0., last as f32);
    let i = (at.floor() as usize).min(last);
    let t = at - i as f32;
    let left = |k: usize| widths[..k].iter().sum::<f32>();
    let j = (i + 1).min(last);
    let x = left(i) + (left(j) - left(i)) * t;
    let w = widths[i] + (widths[j] - widths[i]) * t;
    (x, w)
}

/// What picking an option does, given the option's index.
pub(super) type OnPick = Rc<dyn Fn(&mut Workspace, usize, &mut Window, &mut Context<Workspace>)>;

/// How a control is drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Look {
    /// An outline round the row (the title bar's harness switch).
    pub framed: bool,
    /// Share the row's width equally instead of sizing to the labels.
    pub fill: bool,
    /// Text size: small for the title bar and Appearance, else extra small.
    pub small: bool,
    /// A 2 px underline under the picked tab instead of a pill (v2's
    /// rail tabs).
    pub underline: bool,
}

/// Which rows each growing list has shown, so only new ones grow in.
#[derive(Default)]
pub(super) struct Grown {
    seen: std::collections::HashMap<(&'static str, String), (Instant, usize)>,
    /// Rows first seen in the current frame, per group, for the stagger.
    fresh: std::collections::HashMap<&'static str, (Instant, usize)>,
    /// How many frames each group has drawn (the first ones load the list).
    settling_frames: std::collections::HashMap<&'static str, u32>,
    drawn_at: std::collections::HashMap<&'static str, Instant>,
}

impl Grown {
    fn fresh_in_frame(&mut self, group: &'static str, now: Instant) -> usize {
        let e = self.fresh.entry(group).or_insert((now, 0));
        if now.saturating_duration_since(e.0) > std::time::Duration::from_millis(8) {
            *e = (now, 0);
        }
        e.1 += 1;
        e.1 - 1
    }

    /// Counts frames per group: a list is "live" after its first second,
    /// so the rows a list loads with never animate.
    fn touch(&mut self, group: &'static str) {
        let now = Instant::now();
        let last = self.drawn_at.insert(group, now);
        if last
            .is_none_or(|t| now.saturating_duration_since(t) > std::time::Duration::from_millis(4))
        {
            *self.settling_frames.entry(group).or_default() += 1;
        }
    }

    fn live(&self, group: &'static str) -> bool {
        self.settling_frames.get(group).is_some_and(|n| *n >= 2)
    }
}

impl Workspace {
    /// Where control `key`'s highlight is this frame, heading for `target`.
    /// An invisible layer that records where its parent was laid out
    /// under (`group`, `i`), read next frame by [`Self::underline`].
    pub(super) fn measure(&self, group: &'static str, i: usize) -> impl IntoElement + use<> {
        let store = self.measured.clone();
        let view = self.view_id;
        gpui_kit::canvas(
            move |b, window, _| {
                // Moved or new: draw once more so the underline follows.
                if store.borrow_mut().insert((group, i), b) != Some(b) {
                    super::frame::next_frame_for(window, view);
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0()
    }

    /// A 2 px underline that springs to item `active` of `group`, placed
    /// in the row measured as (`group`, `usize::MAX`). One underline per
    /// row (v2's layoutId), so switching tabs slides rather than blinks.
    pub(super) fn underline(
        &self,
        group: &'static str,
        key_x: &'static str,
        key_w: &'static str,
        active: Option<usize>,
        color: gpui_kit::Hsla,
    ) -> Option<AnyElement> {
        let (row, tab) = {
            let m = self.measured.borrow();
            (
                m.get(&(group, usize::MAX)).copied()?,
                m.get(&(group, active?)).copied()?,
            )
        };
        let x = self.glide(key_x, f32::from(tab.origin.x - row.origin.x));
        let w = self.glide(key_w, f32::from(tab.size.width));
        Some(
            div()
                .absolute()
                .bottom_0()
                .left(px(x))
                .w(px(w.max(0.)))
                .h(px(2.))
                .bg(color)
                .into_any_element(),
        )
    }

    /// A row that grows in the first time it appears (v2's `appear`: a
    /// new sub-session or source row eases from 0 to its height and fades
    /// in), staggered 40 ms when several arrive together. Rows already
    /// there when the list first draws do not animate.
    pub(super) fn grow_in(&self, group: &'static str, key: String, row: AnyElement) -> AnyElement {
        let now = Instant::now();
        let mut rows = self.grown.borrow_mut();
        let k = (group, key);
        let (since, ix) = match rows.seen.get(&k) {
            Some(&v) => v,
            None => {
                // Rows the list loads with are just there; later ones grow.
                let v = if rows.live(group) {
                    (now, rows.fresh_in_frame(group, now))
                } else {
                    (
                        now.checked_sub(std::time::Duration::from_secs(5))
                            .unwrap_or(now),
                        0,
                    )
                };
                rows.seen.insert(k, v);
                v
            }
        };
        rows.touch(group);
        drop(rows);
        let f = crate::motion::staggered(
            since,
            ix,
            std::time::Duration::from_millis(40),
            crate::motion::UI,
            now,
            self.reduced_motion,
        );
        if f >= 1. {
            return row;
        }
        self.anim_pending.set(true);
        div()
            .overflow_hidden()
            .max_h(px(44. * f))
            .opacity(f)
            .child(row)
            .into_any_element()
    }

    pub(super) fn glide(&self, key: &'static str, target: f32) -> f32 {
        let mut map = self.glides.borrow_mut();
        let g = map.entry(key).or_insert_with(|| Glide::new(target));
        let at = g.frame(target, Instant::now(), self.reduced_motion);
        if g.moving() {
            self.anim_pending.set(true);
        }
        at
    }

    /// A segmented control with a sliding highlight.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn segmented(
        &self,
        key: &'static str,
        options: &[Segment],
        selected: usize,
        look: Look,
        p: Palette,
        on_pick: OnPick,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let at = self.glide(key, selected as f32);
        let widths: Vec<f32> = options
            .iter()
            .map(|o| o.width() + if look.small { 8. } else { 0. })
            .collect();
        let n = options.len().max(1) as f32;
        let pill = div()
            .absolute()
            .map(|d| {
                if look.underline {
                    d.bottom_0().h(px(2.)).bg(p.text)
                } else {
                    d.top(px(SEGMENT_PAD))
                        .bottom(px(SEGMENT_PAD))
                        .rounded_full()
                        .bg(p.hover)
                }
            })
            .map(|d| {
                if look.fill {
                    d.left(relative(at / n)).w(relative(1. / n))
                } else {
                    let (x, w) = pill_at(&widths, at);
                    d.left(px(x)).w(px(w))
                }
            });
        div()
            .relative()
            .flex()
            .items_center()
            .p(px(SEGMENT_PAD))
            .rounded_full()
            .when(look.framed, |d| d.border_1().border_color(p.border))
            .when(look.underline, |d| d.rounded_none())
            .map(|d| if look.fill { d.w_full() } else { d.flex_none() })
            // The pill's track is the padding box, so fractions line up.
            .child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(px(SEGMENT_PAD))
                    .right(px(SEGMENT_PAD))
                    .child(pill),
            )
            .children(options.iter().enumerate().map(|(i, o)| {
                let on = i == selected;
                let pick = on_pick.clone();
                let id = SharedString::from(format!("{key}-{i}"));
                let seg = div()
                    .id(id.clone())
                    .relative()
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap_1p5()
                    .py(px(3.))
                    .rounded_md()
                    .cursor_pointer()
                    .map(|d| if look.small { d.text_sm() } else { d.text_xs() })
                    .map(|d| {
                        if look.fill {
                            d.flex_1().min_w_0()
                        } else {
                            d.flex_none().w(px(widths[i]))
                        }
                    })
                    .text_color(if on { p.text } else { p.muted })
                    .hover(|d| d.text_color(p.text))
                    .children(o.glyph.clone().map(|path| {
                        svg()
                            .path(path)
                            .size(px(12.))
                            .flex_none()
                            .text_color(if on { p.accent } else { p.faint })
                    }))
                    .child(div().truncate().child(o.label.clone()))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        pick(this, i, window, cx);
                        cx.notify();
                    }));
                self.press(id, seg)
            }))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::pill_at;

    #[test]
    fn the_pill_lands_on_each_segment_and_glides_between() {
        let w = [40., 60., 30.];
        assert_eq!(pill_at(&w, 0.), (0., 40.));
        assert_eq!(pill_at(&w, 1.), (40., 60.));
        assert_eq!(pill_at(&w, 2.), (100., 30.));
        assert_eq!(pill_at(&w, 0.5), (20., 50.));
        assert_eq!(pill_at(&w, 9.), (100., 30.), "clamped");
        assert_eq!(pill_at(&[], 1.), (0., 0.));
    }
}
