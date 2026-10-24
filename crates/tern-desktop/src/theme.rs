//! Palette C, Mono & Lichen (ADR 0002): a black and white base and one
//! quiet green-grey accent, kept to well under 5% of the pixels (focus
//! rings, the selected harness, the context ring, Scout's lantern).
//!
//! Every colour the shell uses lives here and nowhere else.

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{App, Hsla, Window, rgb};

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use crate::appearance::{Accent, Appearance};
use crate::assets::MONO_FONT;

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

/// The accent the Appearance screen chose, as `Accent::index`. One
/// process-wide value, so every `Palette::get` (the main window, the
/// popped-out side chat, the theme wipe) agrees.
static ACCENT: AtomicU8 = AtomicU8::new(0);

/// Whether motion is reduced, for the scrollbar's timing.
static REDUCED: AtomicBool = AtomicBool::new(false);

/// Turns reduced motion on or off for the scrollbars, and re-installs them.
pub fn set_reduced_motion(on: bool, cx: &mut App) {
    REDUCED.store(on, Ordering::Relaxed);
    let dark = gpui_kit::component::Theme::global(cx).is_dark();
    install_scrollbar(dark, cx);
    cx.refresh_windows();
}

/// Tern's scrollbar (`crate::scrollbar`) on the kit's Base layer: thin
/// and overlaid, hidden until the area scrolls, a rounded thumb in the
/// theme's text colour at a low strength that widens under the pointer,
/// no track.
fn install_scrollbar(dark: bool, cx: &mut App) {
    use crate::scrollbar::{self as sb, Thumb};
    use gpui_kit::base::{
        ScrollbarEntrance, ScrollbarMode, ScrollbarMotion, ScrollbarStyles, ScrollbarTheme,
    };
    let p = Palette::get(dark);
    let t = sb::timing(REDUCED.load(Ordering::Relaxed));
    let thumb = |s: Thumb| p.text.opacity(sb::thumb_alpha(dark, s));
    let clear = gpui_kit::transparent_black();
    let px = gpui_kit::px;
    let styles = ScrollbarStyles::default()
        .track(|s| s.bg(clear))
        .track_hover(|s| s.bg(clear))
        .track_active(|s| s.bg(clear))
        .thumb(|s| {
            s.bg(thumb(Thumb::Idle))
                .width(px(sb::IDLE_W))
                .inset(px(sb::INSET))
                .radius(px(sb::radius(sb::IDLE_W)))
                .min_length(px(sb::MIN_THUMB))
        })
        .thumb_hover(|s| {
            s.bg(thumb(Thumb::Hover))
                .width(px(sb::HOVER_W))
                .inset(px(sb::INSET))
                .radius(px(sb::radius(sb::HOVER_W)))
                .min_length(px(sb::MIN_THUMB))
        })
        .thumb_active(|s| {
            s.bg(thumb(Thumb::Drag))
                .width(px(sb::HOVER_W))
                .inset(px(sb::INSET))
                .radius(px(sb::radius(sb::HOVER_W)))
                .min_length(px(sb::MIN_THUMB))
        });
    let motion = ScrollbarMotion::default()
        .with_idle(t.idle)
        .with_enter(t.enter)
        .with_exit(t.exit)
        .with_expand(t.expand)
        .with_entrance(ScrollbarEntrance::Fade)
        .with_thumb_hover_entrance(ScrollbarEntrance::Fade);
    gpui_kit::base::Theme::global_mut(cx).scrollbar = ScrollbarTheme::new()
        .with_mode(ScrollbarMode::Scrolling)
        .with_motion(motion)
        .with_styles(styles);
}

/// Sets the accent every palette uses from now on.
pub fn set_accent(a: Accent) {
    ACCENT.store(a.index(), Ordering::Relaxed);
}

impl Palette {
    pub fn get(dark: bool) -> Self {
        let mut p = if dark { Self::dark() } else { Self::light() };
        let accent = Accent::from_index(ACCENT.load(Ordering::Relaxed));
        p.accent = rgb(accent.hex(dark)).into();
        p
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

/// Sets the kit's theme to match: light or dark, the chosen font and text
/// size, and the accent on focus rings, selection, the caret, and links.
pub fn apply_kit(dark: bool, look: &Appearance, window: Option<&mut Window>, cx: &mut App) {
    set_accent(look.accent);
    let mode = if dark {
        ThemeMode::Dark
    } else {
        ThemeMode::Light
    };
    Theme::change(mode, window, cx);
    let p = Palette::get(dark);
    let theme = Theme::global_mut(cx);
    theme.font_family = look.font.family().into();
    theme.font_size = gpui_kit::px(look.size.px());
    theme.mono_font_family = MONO_FONT.into();
    theme.colors.ring = p.accent;
    theme.colors.caret = p.text;
    theme.colors.selection = p.accent.opacity(0.32);
    theme.colors.link = p.text;
    theme.colors.link_hover = p.accent;
    theme.colors.link_active = p.accent;
    // The edits above go straight into the kit's theme; push them down to
    // the Base layer, then put Tern's scrollbar over the kit's.
    Theme::sync_base(cx);
    install_scrollbar(dark, cx);
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
    fn every_accent_reads_like_lichen() {
        for a in Accent::ALL {
            for (dark, p) in [(true, DARK), (false, LIGHT)] {
                for surface in [0, 1, 2, 3] {
                    let r = contrast(a.hex(dark), p[surface]);
                    assert!(r >= 4.5, "{a:?} dark={dark} on {surface}: {r:.2}");
                }
            }
        }
        assert_eq!(Accent::Lichen.hex(true), DARK[9]);
        assert_eq!(Accent::Lichen.hex(false), LIGHT[9]);
    }

    #[test]
    fn contrast_matches_the_wcag_reference() {
        assert!((contrast(0x000000, 0xFFFFFF) - 21.0).abs() < 0.01);
        assert!((contrast(0x777777, 0xFFFFFF) - 4.48).abs() < 0.02);
    }
}
