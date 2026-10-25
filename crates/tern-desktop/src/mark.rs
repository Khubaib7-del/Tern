//! The empty state's Tern mark: dithered sprite strips (dark and light
//! themes, plus a still for reduced motion) from `assets/mark/mark.json`,
//! played with the same strip timing as Scout ([`crate::pet::Strip`]), so
//! the mark beats its wings, flicks its tail and shivers in place. Until
//! the animated strips land, a single dithered still plays.
//!
//! ## The manifest (`mark.json`), read leniently
//!
//! ```json
//! { "frame": [192, 152], "frames": 12, "fps": 12, "durations_ms": [...],
//!   "loop": true,
//!   "themes": { "dark": "mark_dark.png", "light": "mark_light.png" },
//!   "still": { "dark": "mark_still_dark.png", "light": "mark_still_light.png" } }
//! ```
//!
//! `frame` may be `frame_w`/`frame_h`; `themes` may be `strips`, and each
//! entry a file name or `{ "file": … }`; `still` may be `reduced_motion`
//! or one file for both themes. A missing light strip uses the dark one.

use std::time::Instant;

use crate::pet::Strip;

include!(concat!(env!("OUT_DIR"), "/mark_assets.rs"));

/// How many times the loop plays when the empty state appears (or on a
/// harness switch or a hover) before the mark rests on its still: the
/// brand animates the mark only on real events.
pub const PLAYS: u64 = 3;

/// The asset path prefix the app's `AssetSource` serves the mark under.
pub const ASSET_PREFIX: &str = "tern/mark/";

/// An embedded mark file's bytes, for the asset source.
pub fn bytes(name: &str) -> Option<&'static [u8]> {
    MARK_FILES.iter().find(|(n, _)| *n == name).map(|(_, b)| *b)
}

/// Every embedded mark asset path.
pub fn asset_paths() -> impl Iterator<Item = String> {
    MARK_FILES.iter().map(|(n, _)| format!("{ASSET_PREFIX}{n}"))
}

/// The empty state's three suggestion chips per harness: the label, and
/// what it puts in the composer.
pub fn suggestions(study: bool) -> &'static [(&'static str, &'static str)] {
    if study {
        &[
            ("Quiz me", "Quiz me on "),
            ("Explain like I'm new", "Explain this like I'm new to it: "),
            (
                "Flashcards from a PDF",
                "Make flashcards from the attached PDF.",
            ),
        ]
    } else {
        &[
            ("Literature review", "Do a literature review on "),
            ("Fact-check a claim", "Fact-check this claim: "),
            ("Compare two papers", "Compare these two papers: "),
        ]
    }
}

/// The parsed `mark.json`.
#[derive(Clone, Debug, PartialEq)]
pub struct MarkAnim {
    pub frame_w: f32,
    pub frame_h: f32,
    pub strip: Strip,
    pub dark: String,
    pub light: String,
    pub still_dark: Option<String>,
    pub still_light: Option<String>,
}

fn file_of(v: Option<&serde_json::Value>) -> Option<String> {
    let v = v?;
    v.as_str()
        .or_else(|| v.get("file").and_then(|x| x.as_str()))
        .map(str::to_owned)
}

impl MarkAnim {
    pub fn parse(text: &str) -> Option<MarkAnim> {
        let v: serde_json::Value = serde_json::from_str(text).ok()?;
        let num = |v: Option<&serde_json::Value>| v.and_then(|x| x.as_f64()).map(|x| x as f32);
        let (fw, fh) = match v.get("frame").or_else(|| v.get("frame_size")) {
            Some(serde_json::Value::Array(a)) if a.len() == 2 => (num(a.first())?, num(a.get(1))?),
            Some(serde_json::Value::Object(o)) => (
                num(o.get("w").or_else(|| o.get("width")))?,
                num(o.get("h").or_else(|| o.get("height")))?,
            ),
            _ => (num(v.get("frame_w"))?, num(v.get("frame_h"))?),
        };
        if fw <= 0. || fh <= 0. {
            return None;
        }
        let themes = v.get("themes").or_else(|| v.get("strips"));
        let dark = file_of(themes.and_then(|t| t.get("dark")))
            .or_else(|| file_of(v.get("file")))
            .or_else(|| file_of(themes))?;
        let light = file_of(themes.and_then(|t| t.get("light"))).unwrap_or_else(|| dark.clone());
        let still = v.get("still").or_else(|| v.get("reduced_motion"));
        let still_dark = file_of(still.and_then(|s| s.get("dark"))).or_else(|| file_of(still));
        let still_light =
            file_of(still.and_then(|s| s.get("light"))).or_else(|| still_dark.clone());
        let frames = num(v.get("frames")).unwrap_or(1.).max(1.) as u32;
        let durations: Vec<u32> = v
            .get("durations_ms")
            .and_then(|x| x.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|d| d.as_f64())
                    .map(|d| d.max(16.) as u32)
                    .collect()
            })
            .filter(|d: &Vec<u32>| d.len() == frames as usize)
            .unwrap_or_default();
        Some(MarkAnim {
            frame_w: fw,
            frame_h: fh,
            strip: Strip {
                file: dark.clone(),
                frames,
                fps: num(v.get("fps")).unwrap_or(12.).clamp(0.5, 60.),
                looped: v.get("loop").and_then(|x| x.as_bool()).unwrap_or(true),
                overlay: None,
                baseline: fh,
                durations,
                loop_start: 0,
            },
            dark,
            light,
            still_dark,
            still_light,
        })
    }

    /// The embedded mark (always present: the dithered still at worst).
    pub fn embedded() -> MarkAnim {
        MARK_FILES
            .iter()
            .find(|(n, _)| *n == "mark.json")
            .and_then(|(_, b)| std::str::from_utf8(b).ok())
            .and_then(MarkAnim::parse)
            .unwrap_or_else(|| MarkAnim {
                frame_w: 192.,
                frame_h: 152.,
                strip: Strip {
                    file: "mark_still_dark.png".into(),
                    frames: 1,
                    fps: 1.,
                    looped: true,
                    overlay: None,
                    baseline: 152.,
                    durations: Vec::new(),
                    loop_start: 0,
                },
                dark: "mark_still_dark.png".into(),
                light: "mark_still_light.png".into(),
                still_dark: None,
                still_light: None,
            })
    }

    /// Whether there is anything to play.
    pub fn animated(&self) -> bool {
        self.strip.frames > 1
    }

    /// Whether the mark is still playing its loops, `PLAYS` passes from
    /// `since`; after that it rests on the still.
    pub fn playing(&self, since: Instant, now: Instant) -> bool {
        self.animated()
            && (now.saturating_duration_since(since).as_millis() as u64)
                < PLAYS * self.strip.pass_ms()
    }

    /// How often to redraw while it plays, in ms.
    pub fn frame_ms(&self) -> u64 {
        self.strip.min_frame_ms().max(40)
    }

    /// The mark `h` px tall, as (width, height).
    pub fn display_size(&self, h: f32) -> (f32, f32) {
        (h * self.frame_w / self.frame_h, h)
    }

    /// The asset path, frame index and frame count to draw now. Reduced
    /// motion, and the rest after `PLAYS` loops, draw the still (or the
    /// strip's first frame).
    pub fn frame(
        &self,
        dark: bool,
        since: Instant,
        now: Instant,
        reduced: bool,
    ) -> (String, u32, u32) {
        let reduced = reduced || !self.playing(since, now);
        if reduced {
            let still = if dark {
                &self.still_dark
            } else {
                &self.still_light
            };
            if let Some(s) = still {
                return (format!("{ASSET_PREFIX}{s}"), 0, 1);
            }
        }
        let file = if dark { &self.dark } else { &self.light };
        let index = if reduced {
            0
        } else {
            self.strip
                .index_at(now.saturating_duration_since(since).as_millis() as u64)
        };
        (format!("{ASSET_PREFIX}{file}"), index, self.strip.frames)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn the_embedded_mark_resolves() {
        let m = MarkAnim::embedded();
        assert!(bytes(&m.dark).is_some(), "{}", m.dark);
        assert!(bytes(&m.light).is_some(), "{}", m.light);
        let t = Instant::now();
        let (p, i, n) = m.frame(true, t, t, false);
        assert!(p.starts_with(ASSET_PREFIX));
        assert!(i < n);
    }

    #[test]
    fn the_mark_reader_is_lenient_and_plays() {
        let m = MarkAnim::parse(
            r#"{"frame_w": 100, "frame_h": 50, "frames": 4, "fps": 10,
                "strips": {"dark": {"file": "d.png"}, "light": "l.png"},
                "reduced_motion": "s.png"}"#,
        )
        .unwrap();
        assert_eq!((m.dark.as_str(), m.light.as_str()), ("d.png", "l.png"));
        assert!(m.animated());
        assert_eq!(m.display_size(25.), (50., 25.));
        let t0 = Instant::now();
        let (p, i, n) = m.frame(false, t0, t0 + Duration::from_millis(250), false);
        assert_eq!((p.as_str(), i, n), ("tern/mark/l.png", 2, 4));
        let (p, i, n) = m.frame(true, t0, t0 + Duration::from_millis(250), true);
        assert_eq!((p.as_str(), i, n), ("tern/mark/s.png", 0, 1));
        // Three passes (4 frames at 10 fps: 400 ms each), then the still.
        assert!(m.playing(t0, t0 + Duration::from_millis(1100)));
        assert!(!m.playing(t0, t0 + Duration::from_millis(1250)));
        let (p, i, n) = m.frame(false, t0, t0 + Duration::from_millis(1300), false);
        assert_eq!((p.as_str(), i, n), ("tern/mark/s.png", 0, 1));
        let one = MarkAnim::parse(r#"{"frame": [10, 10], "file": "x.png"}"#).unwrap();
        assert_eq!(one.light, "x.png");
        assert!(!one.animated());
        assert!(MarkAnim::parse("{}").is_none());
    }

    #[test]
    fn three_suggestions_per_harness() {
        assert_eq!(suggestions(false).len(), 3);
        assert_eq!(suggestions(true).len(), 3);
        assert_eq!(suggestions(false)[0].0, "Literature review");
        assert_eq!(suggestions(true)[2].0, "Flashcards from a PDF");
    }
}
