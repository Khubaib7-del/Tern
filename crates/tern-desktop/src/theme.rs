//! Neutral greys until the palette decision (ADR 0002) is made.
//!
//! No accent hue on purpose: when the palette is chosen, it lands here and
//! nowhere else.

use gpui_kit::{Hsla, rgb};

/// The surfaces and text colours the shell uses.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub rail: Hsla,
    pub sidebar: Hsla,
    pub canvas: Hsla,
    pub raised: Hsla,
    pub hover: Hsla,
    pub border: Hsla,
    pub text: Hsla,
    pub muted: Hsla,
    pub faint: Hsla,
}

impl Palette {
    pub fn get(dark: bool) -> Self {
        if dark { Self::dark() } else { Self::light() }
    }

    pub fn dark() -> Self {
        Self {
            rail: rgb(0x111111).into(),
            sidebar: rgb(0x161616).into(),
            canvas: rgb(0x1b1b1b).into(),
            raised: rgb(0x242424).into(),
            hover: rgb(0x2c2c2c).into(),
            border: rgb(0x2e2e2e).into(),
            text: rgb(0xe8e8e8).into(),
            muted: rgb(0x9a9a9a).into(),
            faint: rgb(0x6a6a6a).into(),
        }
    }

    pub fn light() -> Self {
        Self {
            rail: rgb(0xeaeaea).into(),
            sidebar: rgb(0xf3f3f3).into(),
            canvas: rgb(0xfbfbfb).into(),
            raised: rgb(0xececec).into(),
            hover: rgb(0xe2e2e2).into(),
            border: rgb(0xdedede).into(),
            text: rgb(0x161616).into(),
            muted: rgb(0x5f5f5f).into(),
            faint: rgb(0x9a9a9a).into(),
        }
    }

    /// Every token part-way from `self` to `to` (0 to 1), for the theme
    /// switch.
    pub fn lerp(&self, to: &Self, t: f32) -> Self {
        let l = |a, b| crate::motion::lerp_color(a, b, t);
        Self {
            rail: l(self.rail, to.rail),
            sidebar: l(self.sidebar, to.sidebar),
            canvas: l(self.canvas, to.canvas),
            raised: l(self.raised, to.raised),
            hover: l(self.hover, to.hover),
            border: l(self.border, to.border),
            text: l(self.text, to.text),
            muted: l(self.muted, to.muted),
            faint: l(self.faint, to.faint),
        }
    }
}
