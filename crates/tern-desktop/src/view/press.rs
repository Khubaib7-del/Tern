//! Capsule controls and the press jiggle (v2's buttons, PR #58).
//!
//! Every button is a capsule ([`Capsule::capsule`]). Controls that are
//! pressed a lot (segmented controls, chips, Send, the plan card's
//! choices) dip a touch on press and spring back with a small overshoot
//! on release ([`Workspace::press`]), on [`SpringSpec::PRESS`]. The dip
//! moves the control with a relative offset, so nothing around it moves.
//! Reduced motion keeps it still.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

use gpui_kit::component::button::{Button, ButtonRounded};
use gpui_kit::prelude::*;
use gpui_kit::{MouseButton, SharedString, px};

use super::Workspace;
use crate::motion::{Spring, SpringSpec};

/// Each pressed control's spring, by key.
pub(super) type Presses = Rc<RefCell<HashMap<SharedString, Spring>>>;

/// How far a pressed control dips, in px.
pub(crate) const DIP: f32 = 1.6;

/// A fully rounded button.
pub(crate) trait Capsule {
    fn capsule(self) -> Self;
}

impl Capsule for Button {
    fn capsule(self) -> Self {
        // Past half the shorter side the renderer clamps it: a pill, or a
        // circle for an icon button.
        self.rounded(ButtonRounded::Size(px(9999.)))
    }
}

/// The dip for a spring value (1 pressed, 0 at rest; below 0 is the
/// release overshoot).
pub(crate) fn dip(v: f32) -> f32 {
    (v * DIP).clamp(-DIP, DIP)
}

impl Workspace {
    /// Makes `el` dip on press and spring back with a jiggle on release.
    /// `key` must be stable across frames (the control's id).
    pub(super) fn press<E: InteractiveElement + Styled>(
        &self,
        key: impl Into<SharedString>,
        el: E,
    ) -> E {
        let key: SharedString = key.into();
        let v = {
            let mut map = self.presses.borrow_mut();
            match map.get_mut(&key) {
                Some(s) => {
                    let v = s.sample(Instant::now());
                    if s.moving() {
                        self.anim_pending.set(true);
                    }
                    v
                }
                None => 0.,
            }
        };
        if self.reduced_motion {
            return el;
        }
        let view = self.view_id;
        let down = (self.presses.clone(), key.clone());
        let up = (self.presses.clone(), key.clone());
        let out = (self.presses.clone(), key);
        let set =
            move |(map, key): &(Presses, SharedString), to: f32, window: &mut gpui_kit::Window| {
                let mut map = map.borrow_mut();
                if to == 0. && !map.contains_key(key) {
                    return;
                }
                let s = map
                    .entry(key.clone())
                    .or_insert_with(|| Spring::new(0., SpringSpec::PRESS));
                s.retarget(to, Instant::now());
                super::frame::next_frame_for(window, view);
            };
        el.relative()
            .top(px(dip(v)))
            .on_mouse_down(MouseButton::Left, move |_, window, _| {
                set(&down, 1., window)
            })
            .on_mouse_up(MouseButton::Left, move |_, window, _| set(&up, 0., window))
            .on_mouse_up_out(MouseButton::Left, move |_, window, _| set(&out, 0., window))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn a_press_dips_and_the_release_overshoots_then_settles() {
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        let mut s = Spring::new(0., SpringSpec::PRESS);
        s.retarget(1., t0);
        assert!(s.sample(ms(120)) > 0.8, "dips quickly");
        s.retarget(0., ms(200));
        let mut low = 1f32;
        for t in (200..800).step_by(8) {
            low = low.min(s.sample(ms(t)));
        }
        assert!(low < -0.05 && low > -0.6, "a small jiggle past rest: {low}");
        assert!(s.sample(ms(1500)).abs() < 0.01);
        assert!(dip(low).abs() < DIP);
        assert_eq!(dip(5.), DIP);
    }
}
