//! Palette C, Mono & Lichen (ADR 0002): a black and white base and one
//! quiet green-grey accent, kept to well under 5% of the pixels (focus
//! rings, the selected harness, the context ring, Scout's lantern).
//!
//! Every colour the shell uses lives here and nowhere else.

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{App, Hsla, Window, rgb};

use crate::assets::{MONO_FONT, UI_FONT};

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
    /// Lichen. Only for selection, focus, and the one live thing.
    pub accent: Hsla,
    /// The Study harness's warmer paper, behind its screens.
    pub warm: Hsla,
}

/// The hex values, kept for the contrast tests and the docs.
const DARK: [u32; 10] = [
    0x0C0C0C, // rail (background)
    0x101010, // sidebar
    0x131313, // canvas (surface)
    0x1C1C1C, // raised
    0x222222, // hover
    0x272727, // border
    0xEDEDED, // text
    0xA1A1A1, // muted
    0x707070, // faint
    0xA3AE8E, // accent
];
/// Study's warmer surface (dark, light): a touch of paper over the
/// canvas, so the harness reads by tone, never by hue.
const WARM: [u32; 2] = [0x16140F, 0xF8F5EE];
const LIGHT: [u32; 10] = [
    0xEFEFEC, 0xF5F5F3, 0xFBFBFA, 0xEFEFEC, 0xE7E7E3, 0xDCDCD8, 0x141414, 0x5A5A57, 0x8C8C87,
    0x56603F,
];

impl Palette {
    pub fn get(dark: bool) -> Self {
        if dark { Self::dark() } else { Self::light() }
    }

    fn from_hex(h: [u32; 10], warm: u32) -> Self {
        let c = |i: usize| -> Hsla { rgb(h[i]).into() };
        Self {
            rail: c(0),
            sidebar: c(1),
            canvas: c(2),
            raised: c(3),
            hover: c(4),
            border: c(5),
            text: c(6),
            muted: c(7),
            faint: c(8),
            accent: c(9),
            warm: rgb(warm).into(),
        }
    }

    pub fn dark() -> Self {
        Self::from_hex(DARK, WARM[0])
    }

    pub fn light() -> Self {
        Self::from_hex(LIGHT, WARM[1])
    }
}

/// Sets the kit's theme to match: light or dark, Tern's fonts, and the
/// accent on focus rings, selection, the caret, and links.
pub fn apply_kit(dark: bool, window: Option<&mut Window>, cx: &mut App) {
    let mode = if dark {
        ThemeMode::Dark
    } else {
        ThemeMode::Light
    };
    Theme::change(mode, window, cx);
    let p = Palette::get(dark);
    let theme = Theme::global_mut(cx);
    theme.font_family = UI_FONT.into();
    theme.mono_font_family = MONO_FONT.into();
    theme.colors.ring = p.accent;
    theme.colors.caret = p.text;
    theme.colors.selection = p.accent.opacity(0.32);
    theme.colors.link = p.text;
    theme.colors.link_hover = p.accent;
    theme.colors.link_active = p.accent;
}

#[cfg(test)]
/// WCAG contrast ratio of two sRGB colours given as 0xRRGGBB.
pub fn contrast(a: u32, b: u32) -> f32 {
    fn lum(c: u32) -> f32 {
        let ch = |v: u32| {
            let s = v as f32 / 255.0;
            if s <= 0.03928 {
                s / 12.92
            } else {
                ((s + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * ch((c >> 16) & 255) + 0.7152 * ch((c >> 8) & 255) + 0.0722 * ch(c & 255)
    }
    let (x, y) = (lum(a), lum(b));
    let (hi, lo) = if x > y { (x, y) } else { (y, x) };
    (hi + 0.05) / (lo + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_and_accent_read_in_both_modes() {
        for (name, p) in [("dark", DARK), ("light", LIGHT)] {
            for surface in [0, 1, 2, 3] {
                for (role, ix, min) in [("text", 6, 7.0), ("muted", 7, 4.5), ("accent", 9, 4.5)] {
                    let r = contrast(p[ix], p[surface]);
                    assert!(r >= min, "{name} {role} on surface {surface}: {r:.2}");
                }
            }
            // Faint is for hints and timestamps: readable, never the only
            // carrier of meaning.
            assert!(contrast(p[8], p[2]) >= 3.0, "{name} faint");
        }
    }

    #[test]
    fn text_reads_on_the_study_surface() {
        for (p, warm) in [(DARK, WARM[0]), (LIGHT, WARM[1])] {
            assert!(contrast(p[6], warm) >= 7.0);
            assert!(contrast(p[7], warm) >= 4.5);
            assert!(contrast(p[9], warm) >= 4.5);
            assert!(contrast(p[8], warm) >= 3.0);
        }
    }

    #[test]
    fn contrast_matches_the_wcag_reference() {
        assert!((contrast(0x000000, 0xFFFFFF) - 21.0).abs() < 0.01);
        assert!((contrast(0x777777, 0xFFFFFF) - 4.48).abs() < 0.02);
    }
}
