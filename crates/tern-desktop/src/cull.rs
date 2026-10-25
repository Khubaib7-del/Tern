//! Rows of a long scrolling page that are far outside the window are
//! drawn as empty boxes of the height they had when last drawn.
//!
//! GPUI re-renders, lays out and prepaints a scroll area's whole content
//! on every wheel step (its scroll listener notifies the view), and a
//! cached view cannot help: its cache is keyed by its bounds on screen,
//! which move as the page scrolls. Settings › Providers has a row per
//! provider (logo, name, status, buttons, the default provider's model
//! list), most of them out of sight at a time. Each row records where it
//! sits in the page's content and how tall it is (in its prepaint), and
//! the next render builds only the rows within [`MARGIN`] of the
//! viewport; the rest are boxes of their last height, so the page keeps
//! its length and the scrollbar does not jump. A row that has never been
//! drawn, or a page that has not been laid out yet, is drawn whole.
//!
//! GPUI-free so it is unit tested.

use std::collections::HashMap;

/// How far past the viewport's edges rows are still drawn, px: a wheel
/// notch or two, so a row is drawn before it scrolls into view.
pub const MARGIN: f32 = 320.;

/// Where a row sat when last drawn: its top in the page's content (0 at
/// the top of the content, not of the window) and its height.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spot {
    pub top: f32,
    pub h: f32,
}

impl Spot {
    /// A row drawn with its top at `window_top` while the page was
    /// scrolled to `offset_y` (GPUI's sign: 0 at the top, negative
    /// further down).
    pub fn at(window_top: f32, h: f32, offset_y: f32) -> Self {
        Self {
            top: window_top - offset_y,
            h,
        }
    }

    /// Whether the row is far enough outside the viewport (its top and
    /// height in window px) to draw as an empty box, at scroll offset
    /// `offset_y`.
    pub fn hidden(&self, offset_y: f32, view_top: f32, view_h: f32) -> bool {
        if view_h <= 0. || self.h <= 0. {
            return false;
        }
        let top = self.top + offset_y;
        top + self.h < view_top - MARGIN || top > view_top + view_h + MARGIN
    }
}

/// The rows of every culled page, by key.
#[derive(Debug, Default)]
pub struct Rows {
    spots: HashMap<String, Spot>,
}

impl Rows {
    pub fn get(&self, key: &str) -> Option<Spot> {
        self.spots.get(key).copied()
    }

    pub fn set(&mut self, key: &str, spot: Spot) {
        match self.spots.get_mut(key) {
            Some(s) => *s = spot,
            None => {
                self.spots.insert(key.to_owned(), spot);
            }
        }
    }

    /// Whether the row `key` draws as an empty box (see [`Spot::hidden`]).
    pub fn hidden(&self, key: &str, offset_y: f32, view_top: f32, view_h: f32) -> Option<f32> {
        self.get(key)
            .filter(|s| s.hidden(offset_y, view_top, view_h))
            .map(|s| s.h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_far_outside_the_viewport_are_boxes_and_near_ones_are_drawn() {
        // The viewport is 600 px tall from y = 100 in the window.
        let (view_top, view_h) = (100., 600.);
        let mut rows = Rows::default();
        // Laid out at the top of the page (offset 0): rows of 60 px.
        for i in 0..40 {
            rows.set(&format!("r{i}"), Spot::at(100. + i as f32 * 60., 60., 0.));
        }
        let shown = |rows: &Rows, off: f32| -> Vec<usize> {
            (0..40)
                .filter(|i| {
                    rows.hidden(&format!("r{i}"), off, view_top, view_h)
                        .is_none()
                })
                .collect()
        };
        // At the top: the first ten rows on screen plus MARGIN below.
        let top = shown(&rows, 0.);
        assert_eq!(top.first(), Some(&0));
        assert_eq!(top.last(), Some(&15), "{top:?}");
        // Scrolled down 1200 px: rows above the margin are boxes too.
        let mid = shown(&rows, -1200.);
        assert_eq!(mid.first(), Some(&14), "{mid:?}");
        assert_eq!(mid.last(), Some(&35), "{mid:?}");
        // A box keeps the row's height.
        assert_eq!(rows.hidden("r39", 0., view_top, view_h), Some(60.));
        // Unknown rows and an unmeasured page draw whole.
        assert_eq!(rows.hidden("new", 0., view_top, view_h), None);
        assert_eq!(rows.hidden("r39", 0., 0., 0.), None);
    }

    #[test]
    fn a_spot_is_kept_in_content_coordinates() {
        // Drawn at y = -500 in the window while scrolled 800 px down: its
        // place in the content is 300.
        let s = Spot::at(-500., 40., -800.);
        assert_eq!(s.top, 300.);
        // Back at the top, it is on screen.
        assert!(!s.hidden(0., 0., 600.));
        // Far below the fold, it is not.
        assert!(s.hidden(-2000., 0., 600.));
    }
}
