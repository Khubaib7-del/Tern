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
}

impl Workspace {
    /// Where control `key`'s highlight is this frame, heading for `target`.
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
            .top(px(SEGMENT_PAD))
            .bottom(px(SEGMENT_PAD))
            .rounded_md()
            .bg(p.hover)
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
            .rounded_lg()
            .when(look.framed, |d| d.border_1().border_color(p.border))
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
                div()
                    .id(SharedString::from(format!("{key}-{i}")))
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
                    }))
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
