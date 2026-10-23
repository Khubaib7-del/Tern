//! The Appearance choices: theme, accent, interface font, and text size.
//! GPUI-free so applying and saving a choice is unit tested; the view
//! calls [`Appearance::apply`], then re-applies the kit theme and saves.
//!
//! Every choice stays inside Palette C: the accent is the one quiet colour
//! (well under 5% of the pixels), and every option meets the same contrast
//! floors as Lichen (`theme` tests).

use tern_agent::Preferences;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThemeChoice {
    /// Follow the operating system.
    System,
    Light,
    #[default]
    Dark,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Accent {
    /// The green-grey of ADR 0002.
    #[default]
    Lichen,
    /// A cool blue-grey.
    Slate,
    /// A warm brown-grey.
    Clay,
    /// No hue: the accent is a grey.
    Mono,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FontChoice {
    /// IBM Plex Sans, bundled.
    #[default]
    Plex,
    /// The operating system's interface font.
    System,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextSize {
    Small,
    #[default]
    Default,
    Large,
}

/// One click on the Appearance screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    Theme(ThemeChoice),
    Accent(Accent),
    Font(FontChoice),
    Size(TextSize),
}

/// The four choices together.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Appearance {
    pub theme: ThemeChoice,
    pub accent: Accent,
    pub font: FontChoice,
    pub size: TextSize,
}

impl ThemeChoice {
    pub const ALL: [Self; 3] = [Self::System, Self::Light, Self::Dark];
    pub fn label(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }
    fn id(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }
}

impl Accent {
    pub const ALL: [Self; 4] = [Self::Lichen, Self::Slate, Self::Clay, Self::Mono];
    pub fn label(self) -> &'static str {
        match self {
            Self::Lichen => "Lichen",
            Self::Slate => "Slate",
            Self::Clay => "Clay",
            Self::Mono => "Mono",
        }
    }
    fn id(self) -> &'static str {
        match self {
            Self::Lichen => "lichen",
            Self::Slate => "slate",
            Self::Clay => "clay",
            Self::Mono => "mono",
        }
    }
    /// The accent as 0xRRGGBB on a dark and on a light theme.
    pub fn hex(self, dark: bool) -> u32 {
        match (self, dark) {
            (Self::Lichen, true) => 0xA3AE8E,
            (Self::Lichen, false) => 0x56603F,
            (Self::Slate, true) => 0x9AA8B8,
            (Self::Slate, false) => 0x4A5868,
            (Self::Clay, true) => 0xB8A08E,
            (Self::Clay, false) => 0x6A5442,
            (Self::Mono, true) => 0xB4B4B4,
            (Self::Mono, false) => 0x555555,
        }
    }
    /// A small index for sharing the accent with the palette.
    pub fn index(self) -> u8 {
        Self::ALL.iter().position(|a| *a == self).unwrap_or(0) as u8
    }
    pub fn from_index(i: u8) -> Self {
        Self::ALL.get(usize::from(i)).copied().unwrap_or_default()
    }
}

impl FontChoice {
    pub const ALL: [Self; 2] = [Self::Plex, Self::System];
    pub fn label(self) -> &'static str {
        match self {
            Self::Plex => "IBM Plex Sans",
            Self::System => "System font",
        }
    }
    fn id(self) -> &'static str {
        match self {
            Self::Plex => "plex",
            Self::System => "system",
        }
    }
    /// The family name the kit theme takes. `.SystemUIFont` is GPUI's name
    /// for the operating system's interface font.
    pub fn family(self) -> &'static str {
        match self {
            Self::Plex => crate::assets::UI_FONT,
            Self::System => ".SystemUIFont",
        }
    }
}

impl TextSize {
    pub const ALL: [Self; 3] = [Self::Small, Self::Default, Self::Large];
    pub fn label(self) -> &'static str {
        match self {
            Self::Small => "Small",
            Self::Default => "Default",
            Self::Large => "Large",
        }
    }
    fn id(self) -> &'static str {
        match self {
            Self::Small => "small",
            Self::Default => "default",
            Self::Large => "large",
        }
    }
    /// The base size every rem-sized text scales from.
    pub fn px(self) -> f32 {
        match self {
            Self::Small => 14.,
            Self::Default => 16.,
            Self::Large => 18.,
        }
    }
}

fn parse<T: Copy + Default>(all: &[T], id: fn(T) -> &'static str, saved: Option<&str>) -> T {
    saved
        .and_then(|s| all.iter().copied().find(|v| id(*v) == s))
        .unwrap_or_default()
}

impl Appearance {
    /// The saved choices; unknown or missing values are the defaults.
    pub fn from_prefs(p: &Preferences) -> Self {
        Self {
            theme: parse(&ThemeChoice::ALL, ThemeChoice::id, p.theme.as_deref()),
            accent: parse(&Accent::ALL, Accent::id, p.accent.as_deref()),
            font: parse(&FontChoice::ALL, FontChoice::id, p.ui_font.as_deref()),
            size: parse(&TextSize::ALL, TextSize::id, p.text_size.as_deref()),
        }
    }

    /// Writes the choices into preferences; defaults are left unset.
    pub fn save_to(&self, p: &mut Preferences) {
        let keep = |id: &'static str, default: &'static str| (id != default).then(|| id.to_owned());
        p.theme = keep(self.theme.id(), ThemeChoice::default().id());
        p.accent = keep(self.accent.id(), Accent::default().id());
        p.ui_font = keep(self.font.id(), FontChoice::default().id());
        p.text_size = keep(self.size.id(), TextSize::default().id());
    }

    /// Applies one click; `true` when something changed.
    pub fn apply(&mut self, c: Choice) -> bool {
        let before = *self;
        match c {
            Choice::Theme(t) => self.theme = t,
            Choice::Accent(a) => self.accent = a,
            Choice::Font(f) => self.font = f,
            Choice::Size(s) => self.size = s,
        }
        *self != before
    }

    /// Whether the window is dark, given the system's own setting.
    pub fn is_dark(&self, system_dark: bool) -> bool {
        match self.theme {
            ThemeChoice::System => system_dark,
            ThemeChoice::Light => false,
            ThemeChoice::Dark => true,
        }
    }

    /// Whether a choice is the current one, for the selected state.
    pub fn is(&self, c: Choice) -> bool {
        match c {
            Choice::Theme(t) => self.theme == t,
            Choice::Accent(a) => self.accent == a,
            Choice::Font(f) => self.font == f,
            Choice::Size(s) => self.size == s,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_click_applies_at_once_and_survives_a_restart() {
        let mut prefs = Preferences::default();
        let mut look = Appearance::from_prefs(&prefs);
        assert_eq!(look, Appearance::default());
        assert!(look.is_dark(false), "dark by default, as before");

        assert!(look.apply(Choice::Theme(ThemeChoice::Light)));
        assert!(!look.is_dark(true));
        assert!(look.apply(Choice::Accent(Accent::Slate)));
        assert!(look.apply(Choice::Font(FontChoice::System)));
        assert!(look.apply(Choice::Size(TextSize::Large)));
        assert!(!look.apply(Choice::Size(TextSize::Large)), "no change");
        assert!(look.is(Choice::Accent(Accent::Slate)));
        assert_eq!(look.font.family(), ".SystemUIFont");
        assert_eq!(look.size.px(), 18.);

        look.save_to(&mut prefs);
        assert_eq!(prefs.theme.as_deref(), Some("light"));
        assert_eq!(prefs.accent.as_deref(), Some("slate"));
        // Through JSON, as the file stores it.
        let json = serde_json::to_string(&prefs).unwrap();
        let back: Preferences = serde_json::from_str(&json).unwrap();
        assert_eq!(Appearance::from_prefs(&back), look);

        // Back to the defaults leaves nothing saved.
        look = Appearance::default();
        look.save_to(&mut prefs);
        assert_eq!(prefs.theme, None);
        assert_eq!(prefs.text_size, None);
    }

    #[test]
    fn system_follows_the_os_and_bad_values_fall_back() {
        let mut look = Appearance::default();
        look.apply(Choice::Theme(ThemeChoice::System));
        assert!(look.is_dark(true));
        assert!(!look.is_dark(false));
        let prefs = Preferences {
            theme: Some("neon".into()),
            accent: Some("hotpink".into()),
            ..Preferences::default()
        };
        assert_eq!(Appearance::from_prefs(&prefs), Appearance::default());
        for a in Accent::ALL {
            assert_eq!(Accent::from_index(a.index()), a);
        }
    }
}
