//! The title bar's window-control areas, kept alive outside the cached
//! workspace.
//!
//! On Windows, GPUI answers the OS hit test (`WM_NCHITTEST`) from the
//! areas elements marked with `window_control_area` in the last frame:
//! the kit's title bar marks itself as the caption (drag) and its three
//! buttons as minimize, maximize and close (`HTMINBUTTON`, `HTMAXBUTTON`,
//! `HTCLOSE`). The buttons have no click handler of their own on Windows;
//! the OS presses them because the hit test said so.
//!
//! The window's root (`view::Shell`) draws the workspace *cached*, so
//! Scout's frames redraw only Scout. GPUI copies a cached view's hitboxes,
//! mouse listeners and scene into the next frame, but not its
//! window-control areas, so every frame drawn with the workspace reused
//! (Scout animating, Scout's hover) had none: the hit test said "client"
//! and a click on minimize, maximize or close did nothing.
//!
//! So `Shell` marks the same areas again on every frame, in a layer drawn
//! *under* the workspace: anything in the workspace that occludes (the
//! title bar's own buttons, a popover, a modal, Scout's toolbar) still
//! cuts them out exactly as it does the kit's own areas. This module is
//! the geometry, GPUI-free so it is unit tested; the layer is in
//! `view/pet_view.rs`.

/// The kit's title bar height (`gpui_kit::component::TITLE_BAR_HEIGHT`),
/// which is also the width of each window button.
pub const BAR_H: f32 = 34.;

/// What a part of the title bar is to the OS.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Area {
    /// Drag to move the window; double click to maximize.
    Drag,
    Min,
    Max,
    Close,
}

/// A rectangle in window px: left, top, width, height.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[cfg(test)]
impl Rect {
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
}

/// Which window buttons the window offers (GPUI's `window_controls()`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Buttons {
    pub minimize: bool,
    pub maximize: bool,
}

/// The title bar's areas in a window `width` px wide, laid out as the
/// kit lays them out: the buttons at the right edge, each `BAR_H` square,
/// minimize then maximize then close (close is always there), and the
/// rest of the bar to its left is the caption.
pub fn areas(width: f32, buttons: Buttons) -> Vec<(Area, Rect)> {
    if !width.is_finite() || width <= 0. {
        return Vec::new();
    }
    let mut right: Vec<Area> = Vec::with_capacity(3);
    if buttons.minimize {
        right.push(Area::Min);
    }
    if buttons.maximize {
        right.push(Area::Max);
    }
    right.push(Area::Close);
    let controls_w = (right.len() as f32 * BAR_H).min(width);
    let mut out = Vec::with_capacity(right.len() + 1);
    let drag_w = width - controls_w;
    if drag_w > 0. {
        out.push((
            Area::Drag,
            Rect {
                x: 0.,
                y: 0.,
                w: drag_w,
                h: BAR_H,
            },
        ));
    }
    for (i, a) in right.into_iter().enumerate() {
        let x = drag_w + i as f32 * BAR_H;
        let w = BAR_H.min(width - x);
        if w > 0. {
            out.push((
                a,
                Rect {
                    x,
                    y: 0.,
                    w,
                    h: BAR_H,
                },
            ));
        }
    }
    out
}

/// The area at a point (window px), if it is on the title bar.
#[cfg(test)]
pub fn area_at(width: f32, buttons: Buttons, x: f32, y: f32) -> Option<Area> {
    areas(width, buttons)
        .into_iter()
        .find(|(_, r)| r.contains(x, y))
        .map(|(a, _)| a)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: Buttons = Buttons {
        minimize: true,
        maximize: true,
    };

    #[test]
    fn matches_the_kits_title_bar_height() {
        assert_eq!(
            f32::from(gpui_kit::component::TITLE_BAR_HEIGHT),
            BAR_H,
            "the layer must cover the kit's buttons exactly"
        );
    }

    #[test]
    fn the_three_buttons_sit_at_the_right_edge_close_last() {
        let a = areas(1280., ALL);
        assert_eq!(
            a.iter().map(|(a, _)| *a).collect::<Vec<_>>(),
            [Area::Drag, Area::Min, Area::Max, Area::Close]
        );
        let close = a[3].1;
        assert_eq!(close.x + close.w, 1280.);
        assert_eq!((close.w, close.h), (BAR_H, BAR_H));
        // The middle of each button (the screenshot's – ▢ ✕).
        let mid = BAR_H / 2.;
        assert_eq!(area_at(1280., ALL, 1280. - mid, mid), Some(Area::Close));
        assert_eq!(
            area_at(1280., ALL, 1280. - BAR_H - mid, mid),
            Some(Area::Max)
        );
        assert_eq!(
            area_at(1280., ALL, 1280. - 2. * BAR_H - mid, mid),
            Some(Area::Min)
        );
        // The rest of the bar drags; below it is the app.
        assert_eq!(area_at(1280., ALL, 600., 10.), Some(Area::Drag));
        assert_eq!(area_at(1280., ALL, 1280. - 5., BAR_H + 1.), None);
    }

    #[test]
    fn the_areas_tile_the_bar_without_gaps_or_overlap() {
        for w in [720., 1024.5, 1920.] {
            let a = areas(w, ALL);
            let mut x = 0.;
            for (_, r) in &a {
                assert_eq!(r.x, x, "{w}");
                x += r.w;
            }
            assert_eq!(x, w);
        }
    }

    #[test]
    fn close_is_always_offered_and_missing_buttons_leave_no_hole() {
        let only_close = Buttons {
            minimize: false,
            maximize: false,
        };
        let a = areas(800., only_close);
        assert_eq!(a.len(), 2);
        assert_eq!(a[1].0, Area::Close);
        assert_eq!(a[0].1.w, 800. - BAR_H);
        // A degenerate window draws nothing rather than a bad rectangle.
        assert!(areas(0., ALL).is_empty());
        assert!(areas(f32::NAN, ALL).is_empty());
        assert!(areas(50., ALL).iter().all(|(_, r)| r.w > 0. && r.x < 50.));
    }
}
