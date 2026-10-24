//! Scout as a floating pet (a Codex-style companion): raster sprite strips
//! of the clay Scout, played from `assets/scout/scout.json`, with spring
//! crossfades between states, a procedural bob and squash, and a soft
//! shadow under the feet. GPUI-free so the timing is unit tested; the view
//! is `view/pet_view.rs`.
//!
//! ## The manifest (`scout.json`)
//!
//! ```json
//! { "format": 1, "frame": [160, 192], "baseline": 184,
//!   "variants": { "normal": "", "study": ".study" },
//!   "states": { "idle": { "file": "idle.png", "frames": 8, "fps": 8,
//!                         "loop": true, "overlay": null, "baseline": 184 }, … } }
//! ```
//!
//! Each state's file is a horizontal strip of `frames` frames, each
//! `frame` px (width, height), feet on `baseline` (px from the frame's
//! top). A variant's suffix goes before `.png` (`idle.study.png`); a strip
//! missing for a variant falls back to the normal one, and a state missing
//! from the manifest falls back to `idle`. The reader is lenient: `frame`
//! may be `frame_w`/`frame_h` or `frame_size`, `loop` defaults to true,
//! `fps` to 8, `frames` to 1, and `overlay` (a file drawn above the head,
//! such as a `?` bubble) is optional.

use std::time::{Duration, Instant};

use crate::motion::{Spring, SpringSpec};
use crate::scout::Mood;

include!(concat!(env!("OUT_DIR"), "/scout_assets.rs"));

/// The asset path prefix the app's `AssetSource` serves strips under.
pub const ASSET_PREFIX: &str = "tern/scout/";

/// Scout's on-screen height as a pet, in px (the user asked for about 80).
pub const PET_H: f32 = 84.;

/// Scout's sprite states, as the art pipeline names them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum State {
    Idle,
    Thinking,
    Searching,
    Reading,
    Verifying,
    NeedsYou,
    Done,
    Blocked,
    Study,
    Sleeping,
}

impl State {
    pub const ALL: [State; 10] = [
        State::Idle,
        State::Thinking,
        State::Searching,
        State::Reading,
        State::Verifying,
        State::NeedsYou,
        State::Done,
        State::Blocked,
        State::Study,
        State::Sleeping,
    ];

    pub fn key(self) -> &'static str {
        match self {
            State::Idle => "idle",
            State::Thinking => "thinking",
            State::Searching => "searching",
            State::Reading => "reading",
            State::Verifying => "verifying",
            State::NeedsYou => "needs_you",
            State::Done => "done",
            State::Blocked => "blocked",
            State::Study => "study",
            State::Sleeping => "sleeping",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            State::Idle => "Idle",
            State::Thinking => "Thinking",
            State::Searching => "Searching",
            State::Reading => "Reading",
            State::Verifying => "Verifying",
            State::NeedsYou => "Needs you",
            State::Done => "Done",
            State::Blocked => "Blocked",
            State::Study => "Study",
            State::Sleeping => "Sleeping",
        }
    }

    /// The sprite for what Tern is doing.
    pub fn of(mood: Mood) -> State {
        match mood {
            Mood::NeedsYou => State::NeedsYou,
            Mood::Blocked => State::Blocked,
            Mood::Searching => State::Searching,
            Mood::Reading => State::Reading,
            Mood::Verifying => State::Verifying,
            Mood::Thinking => State::Thinking,
            Mood::Studying => State::Study,
            Mood::Ready => State::Done,
            Mood::Idle => State::Idle,
        }
    }

    /// How much the body bobs (px at 84 px tall) and how fast (cycles/s).
    fn bob(self) -> (f32, f32) {
        match self {
            State::Searching => (2.6, 1.4),
            State::NeedsYou => (2.2, 1.1),
            State::Done => (3.0, 1.6),
            State::Thinking | State::Reading | State::Verifying => (1.2, 0.5),
            State::Blocked | State::Sleeping => (0.6, 0.22),
            State::Idle | State::Study => (1.0, 0.33),
        }
    }
}

/// One state's strip.
#[derive(Clone, Debug, PartialEq)]
pub struct Strip {
    pub file: String,
    pub frames: u32,
    pub fps: f32,
    pub looped: bool,
    pub overlay: Option<String>,
    pub baseline: f32,
    /// Per-frame times in ms (the art's preferred timing); empty plays at
    /// `fps`.
    pub durations: Vec<u32>,
    /// Where a loop starts again after the first pass (an intro such as
    /// needs-you's turn to face you plays once).
    pub loop_start: u32,
}

impl Strip {
    /// The frame shown `ms` after the state began: per-frame durations
    /// when the art gives them, else `fps`; a loop goes back to
    /// `loop_start`, a one-shot holds its last frame.
    pub fn index_at(&self, ms: u64) -> u32 {
        let n = self.frames.max(1);
        let dur = |i: u32| -> u64 {
            match self.durations.get(i as usize) {
                Some(&d) => d as u64,
                None => (1000. / self.fps).max(16.) as u64,
            }
        };
        let mut t = ms;
        for i in 0..n {
            if t < dur(i) {
                return i;
            }
            t -= dur(i);
        }
        if !self.looped {
            return n - 1;
        }
        let start = self.loop_start.min(n - 1);
        let cycle: u64 = (start..n).map(dur).sum::<u64>().max(1);
        let mut t = t % cycle;
        for i in start..n {
            if t < dur(i) {
                return i;
            }
            t -= dur(i);
        }
        n - 1
    }

    /// The shortest frame time in ms, for scheduling redraws.
    pub fn min_frame_ms(&self) -> u64 {
        self.durations
            .iter()
            .map(|&d| d as u64)
            .min()
            .unwrap_or((1000. / self.fps) as u64)
            .max(16)
    }
}

/// The parsed `scout.json`.
#[derive(Clone, Debug, PartialEq)]
pub struct Manifest {
    pub frame_w: f32,
    pub frame_h: f32,
    pub baseline: f32,
    /// Variant name → file suffix, in the manifest's order.
    pub variants: Vec<(String, String)>,
    pub states: Vec<(State, Strip)>,
    pub placeholder: bool,
    /// The file suffix of the large strips (`"@256"`), drawn when Scout is
    /// shown big (Settings › Scout); the small strips serve the pet.
    pub hi: Option<String>,
}

impl Manifest {
    /// Reads a manifest leniently; `None` only when it is not JSON or has
    /// no usable states.
    pub fn parse(text: &str) -> Option<Manifest> {
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
        let baseline = num(v.get("baseline"))
            .or_else(|| num(v.get("anchor_baseline_y")))
            .unwrap_or(fh);
        let mut variants = vec![("normal".to_owned(), String::new())];
        if let Some(o) = v.get("variants").and_then(|x| x.as_object()) {
            for (k, s) in o {
                let suffix = s
                    .as_str()
                    .or_else(|| s.get("suffix").and_then(|x| x.as_str()))
                    .unwrap_or("")
                    .to_owned();
                if k == "normal" {
                    variants[0].1 = suffix;
                } else {
                    variants.push((k.clone(), suffix));
                }
            }
        }
        let mut states = Vec::new();
        let obj = v.get("states").and_then(|x| x.as_object())?;
        for st in State::ALL {
            let Some(s) = obj.get(st.key()) else { continue };
            let file = match s {
                serde_json::Value::String(f) => f.clone(),
                _ => match s.get("file").and_then(|x| x.as_str()) {
                    Some(f) => f.to_owned(),
                    None => continue,
                },
            };
            let frames = num(s.get("frames")).unwrap_or(1.).max(1.) as u32;
            let durations: Vec<u32> = s
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
            // An overlay is a file name, or `{ "file": … }`.
            let overlay = s.get("overlay").and_then(|o| {
                o.as_str()
                    .or_else(|| o.get("file").and_then(|x| x.as_str()))
                    .map(str::to_owned)
            });
            states.push((
                st,
                Strip {
                    file,
                    frames,
                    fps: num(s.get("fps")).unwrap_or(8.).clamp(0.5, 60.),
                    looped: s.get("loop").and_then(|x| x.as_bool()).unwrap_or(true),
                    overlay,
                    baseline: num(s.get("baseline"))
                        .or_else(|| num(s.get("anchor_baseline_y")))
                        .unwrap_or(baseline),
                    durations,
                    loop_start: (num(s.get("loop_start")).unwrap_or(0.).max(0.) as u32)
                        .min(frames - 1),
                },
            ));
        }
        if states.is_empty() {
            return None;
        }
        Some(Manifest {
            frame_w: fw,
            frame_h: fh,
            baseline,
            variants,
            states,
            placeholder: v
                .get("placeholder")
                .and_then(|x| x.as_bool())
                .unwrap_or(false),
            hi: v.get("hi").and_then(|x| x.as_str()).map(str::to_owned),
        })
    }

    /// The embedded manifest (always present: placeholders at worst).
    pub fn embedded() -> Manifest {
        SCOUT_FILES
            .iter()
            .find(|(n, _)| *n == "scout.json")
            .and_then(|(_, b)| std::str::from_utf8(b).ok())
            .and_then(Manifest::parse)
            .unwrap_or_else(|| Manifest {
                frame_w: 160.,
                frame_h: 192.,
                baseline: 184.,
                variants: vec![("normal".into(), String::new())],
                states: vec![(
                    State::Idle,
                    Strip {
                        file: "idle.png".into(),
                        frames: 1,
                        fps: 1.,
                        looped: true,
                        overlay: None,
                        baseline: 184.,
                        durations: Vec::new(),
                        loop_start: 0,
                    },
                )],
                placeholder: true,
                hi: None,
            })
    }

    /// The strip for `state`, or idle's.
    pub fn strip(&self, state: State) -> &Strip {
        self.states
            .iter()
            .find(|(s, _)| *s == state)
            .or_else(|| self.states.iter().find(|(s, _)| *s == State::Idle))
            .or(self.states.first())
            .map(|(_, s)| s)
            .expect("a manifest has at least one state")
    }

    /// The asset path of `file` in `variant`, falling back to the normal
    /// file when the variant has no strip of its own.
    #[cfg(test)]
    pub fn path(&self, file: &str, variant: &str) -> String {
        let suffix = self
            .variants
            .iter()
            .find(|(n, _)| n == variant)
            .map(|(_, s)| s.as_str())
            .unwrap_or("");
        if !suffix.is_empty() {
            let (stem, ext) = file.rsplit_once('.').unwrap_or((file, "png"));
            let v = format!("{stem}{suffix}.{ext}");
            if bytes(&v).is_some() {
                return format!("{ASSET_PREFIX}{v}");
            }
        }
        format!("{ASSET_PREFIX}{file}")
    }

    /// The asset path of `file` in `variant` at the size it is drawn:
    /// the large strip when `big` and there is one, the variant's own
    /// strip before the normal one.
    pub fn sized(&self, file: &str, variant: &str, big: bool) -> String {
        let vs = self
            .variants
            .iter()
            .find(|(n, _)| n == variant)
            .map(|(_, s)| s.as_str())
            .unwrap_or("");
        let hs = if big {
            self.hi.as_deref().unwrap_or("")
        } else {
            ""
        };
        let (stem, ext) = file.rsplit_once('.').unwrap_or((file, "png"));
        [
            format!("{stem}{vs}{hs}.{ext}"),
            format!("{stem}{vs}.{ext}"),
            format!("{stem}{hs}.{ext}"),
        ]
        .into_iter()
        .find(|f| bytes(f).is_some())
        .map(|f| format!("{ASSET_PREFIX}{f}"))
        .unwrap_or_else(|| format!("{ASSET_PREFIX}{file}"))
    }

    /// The light rim drawn behind `file` on dark themes (a silhouette one
    /// pixel wider, `<stem>.rim.png`), so the charcoal cloak keeps its
    /// edge; `None` when the art has none.
    pub fn rim(&self, file: &str, big: bool) -> Option<String> {
        let (stem, ext) = file.rsplit_once('.').unwrap_or((file, "png"));
        let hs = if big {
            self.hi.as_deref().unwrap_or("")
        } else {
            ""
        };
        [format!("{stem}.rim{hs}.{ext}"), format!("{stem}.rim.{ext}")]
            .into_iter()
            .find(|f| bytes(f).is_some())
            .map(|f| format!("{ASSET_PREFIX}{f}"))
    }

    #[cfg(test)]
    pub fn variant_names(&self) -> impl Iterator<Item = &str> {
        self.variants.iter().map(|(n, _)| n.as_str())
    }
}

/// An embedded Scout file's bytes, for the asset source.
pub fn bytes(name: &str) -> Option<&'static [u8]> {
    SCOUT_FILES
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, b)| *b)
}

/// Every embedded Scout asset path.
pub fn asset_paths() -> impl Iterator<Item = String> {
    SCOUT_FILES
        .iter()
        .map(|(n, _)| format!("{ASSET_PREFIX}{n}"))
}

/// What to draw this frame.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    /// The current state and its frame index.
    pub state: State,
    pub index: u32,
    /// The state being left and its frame, while the crossfade runs, and
    /// how much of the new state shows (0 to 1).
    pub from: Option<(State, u32)>,
    pub mix: f32,
    /// Vertical lift in px (up is positive) and the squash (positive is
    /// wider and shorter), both from the bob and the landing jiggle.
    pub lift: f32,
    pub squash: f32,
}

/// Scout's animation clock: strips, crossfades, bob, squash.
#[derive(Clone, Debug)]
pub struct Animator {
    state: State,
    since: Instant,
    from: Option<(State, Instant)>,
    fade: Spring,
    /// The squash jiggle: kicked on every state change and on a press.
    jiggle: Spring,
    start: Instant,
}

impl Animator {
    pub fn new(now: Instant) -> Self {
        Self {
            state: State::Idle,
            since: now,
            from: None,
            fade: Spring::new(1., SpringSpec::UI),
            jiggle: Spring::new(0., SpringSpec::SCOUT),
            start: now,
        }
    }

    /// Moves to `state` with a crossfade and a small hop of a jiggle.
    pub fn set(&mut self, state: State, now: Instant) {
        if state == self.state {
            return;
        }
        self.from = Some((self.state, self.since));
        self.state = state;
        self.since = now;
        self.fade = Spring::new(0., SpringSpec::UI);
        self.fade.retarget(1., now);
        self.poke(now);
    }

    /// A squash-and-stretch hop (a state change, a click, a drop).
    pub fn poke(&mut self, now: Instant) {
        let mut j = Spring::new(1., SpringSpec::SCOUT);
        j.retarget(0., now);
        self.jiggle = j;
    }

    fn index(strip: &Strip, since: Instant, now: Instant) -> u32 {
        strip.index_at(now.saturating_duration_since(since).as_millis() as u64)
    }

    /// The frame for `now`; reduced motion shows the first frame, no bob,
    /// and swaps states at once.
    pub fn frame(&mut self, m: &Manifest, now: Instant, reduced: bool) -> Frame {
        if reduced {
            self.from = None;
            return Frame {
                state: self.state,
                index: 0,
                from: None,
                mix: 1.,
                lift: 0.,
                squash: 0.,
            };
        }
        let mix = self.fade.sample(now).clamp(0., 1.);
        if !self.fade.moving() {
            self.from = None;
        }
        let (amp, hz) = self.state.bob();
        let t = now.saturating_duration_since(self.start).as_secs_f32();
        let phase = (t * hz * std::f32::consts::TAU).sin();
        // Lifts on the up-beat; squashes a touch as it lands.
        let lift = amp * (phase * 0.5 + 0.5);
        let jig = self.jiggle.sample(now);
        let squash = (1. - phase) * 0.012 * amp.min(2.) + jig * 0.06;
        Frame {
            state: self.state,
            index: Self::index(m.strip(self.state), self.since, now),
            from: self
                .from
                .map(|(s, since)| (s, Self::index(m.strip(s), since, now))),
            mix,
            lift,
            squash,
        }
    }

    /// How long until the drawing next changes: soon while a crossfade or
    /// jiggle runs, else the strip's frame time (bob included), capped so
    /// an idle Scout redraws at most 12 times a second.
    pub fn next_change(&self, m: &Manifest, reduced: bool) -> Option<Duration> {
        if reduced {
            return None;
        }
        if self.fade.moving() || self.jiggle.moving() {
            return Some(Duration::from_millis(16));
        }
        // The next strip frame, but at least ~8 a second for the bob and
        // at most ~12 (an idle Scout stays cheap).
        let ms = m.strip(self.state).min_frame_ms().clamp(83, 125);
        Some(Duration::from_millis(ms))
    }
}

/// The speech bubble's words while Tern works: the state and a short
/// status ("reading 3 sources"); `None` at rest.
pub fn bubble(state: State, sources: usize, detail: Option<&str>) -> Option<String> {
    let plural = |n: usize, one: &str| {
        if n == 1 {
            format!("1 {one}")
        } else {
            format!("{n} {one}s")
        }
    };
    let line = match state {
        State::Idle | State::Sleeping => return None,
        State::Thinking => "thinking…".to_owned(),
        State::Searching => match detail {
            Some(d) if !d.is_empty() => format!("searching “{}”", clip(d, 28)),
            _ => "searching…".to_owned(),
        },
        State::Reading if sources > 0 => format!("reading {}", plural(sources, "source")),
        State::Reading => "reading…".to_owned(),
        State::Verifying => "checking claims…".to_owned(),
        State::NeedsYou => "needs you".to_owned(),
        State::Done => "done".to_owned(),
        State::Blocked => match detail {
            Some(d) if !d.is_empty() => format!("blocked · {}", clip(d, 28)),
            _ => "blocked".to_owned(),
        },
        State::Study => "studying with you".to_owned(),
    };
    Some(line)
}

fn clip(s: &str, n: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= n {
        s.to_owned()
    } else {
        format!("{}…", s.chars().take(n - 1).collect::<String>().trim_end())
    }
}

/// Whether a key press is Hide/Show Scout: `.` with Ctrl or Cmd, and no
/// Alt or Shift.
pub fn is_toggle_key(key: &str, ctrl: bool, cmd: bool, alt: bool, shift: bool) -> bool {
    key == "." && (ctrl || cmd) && !alt && !shift
}

/// Keeps the pet inside the window: `pos` is its top-left, `size` its box.
pub fn clamp_pos(pos: (f32, f32), size: (f32, f32), window: (f32, f32)) -> (f32, f32) {
    let max_x = (window.0 - size.0 - 4.).max(4.);
    let max_y = (window.1 - size.1 - 4.).max(40.);
    (pos.0.clamp(4., max_x), pos.1.clamp(40., max_y))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_embedded_manifest_has_every_state() {
        let m = Manifest::embedded();
        assert!(m.frame_w > 0. && m.frame_h > 0.);
        for st in State::ALL {
            let s = m.strip(st);
            assert!(bytes(&s.file).is_some(), "{st:?} -> {}", s.file);
        }
        assert!(m.variant_names().any(|v| v == "study"));
        assert!(m.path("idle.png", "study").ends_with("idle.study.png"));
        assert!(m.path("idle.png", "nope").ends_with("/idle.png"));
        for big in [false, true] {
            for v in ["normal", "study"] {
                let p = m.sized("idle.png", v, big);
                assert!(
                    bytes(p.strip_prefix(ASSET_PREFIX).unwrap()).is_some(),
                    "{p}"
                );
            }
        }
        if let Some(r) = m.rim("idle.png", true) {
            assert!(bytes(r.strip_prefix(ASSET_PREFIX).unwrap()).is_some());
        }
    }

    #[test]
    fn the_manifest_reader_is_lenient() {
        let m = Manifest::parse(
            r#"{"frame_w": 128, "frame_h": 128, "states": {
                "idle": {"file": "i.png", "frames": 6, "fps": 10},
                "needs_you": "n.png",
                "done": {"file": "d.png", "frames": 4, "loop": false, "overlay": "check.png"}}}"#,
        )
        .unwrap();
        assert_eq!((m.frame_w, m.frame_h, m.baseline), (128., 128., 128.));
        assert_eq!(m.strip(State::Idle).frames, 6);
        assert_eq!(m.strip(State::NeedsYou).file, "n.png");
        assert!(!m.strip(State::Done).looped);
        assert_eq!(m.strip(State::Done).overlay.as_deref(), Some("check.png"));
        // A state the art does not have plays idle.
        assert_eq!(m.strip(State::Sleeping).file, "i.png");
        let m2 = Manifest::parse(
            r#"{"frame_size": {"w": 64, "h": 80}, "variants": {"study": {"suffix": "_study"}},
                "states": {"idle": {"file": "idle.png"}}}"#,
        )
        .unwrap();
        assert_eq!((m2.frame_w, m2.frame_h), (64., 80.));
        assert_eq!(m2.variants[1], ("study".into(), "_study".into()));
        assert!(Manifest::parse("{}").is_none());
        assert!(Manifest::parse("not json").is_none());
    }

    #[test]
    fn strips_play_loop_and_hold() {
        let m = Manifest::parse(
            r#"{"frame": [10, 10], "states": {
                "idle": {"file": "i.png", "frames": 4, "fps": 10},
                "done": {"file": "d.png", "frames": 3, "fps": 10, "loop": false}}}"#,
        )
        .unwrap();
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        let mut a = Animator::new(t0);
        assert_eq!(a.frame(&m, ms(0), false).index, 0);
        assert_eq!(a.frame(&m, ms(250), false).index, 2);
        assert_eq!(a.frame(&m, ms(450), false).index, 0, "loops");
        a.set(State::Done, ms(1000));
        assert_eq!(a.frame(&m, ms(1150), false).index, 1);
        assert_eq!(
            a.frame(&m, ms(5000), false).index,
            2,
            "holds the last frame"
        );
    }

    #[test]
    fn states_crossfade_on_a_spring_with_a_jiggle() {
        let m = Manifest::embedded();
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        let mut a = Animator::new(t0);
        a.frame(&m, ms(2000), false);
        a.set(State::Searching, ms(2000));
        let f = a.frame(&m, ms(2000), false);
        assert_eq!(f.from.map(|x| x.0), Some(State::Idle));
        assert!(f.mix < 0.05);
        let mid = a.frame(&m, ms(2080), false);
        assert!(mid.mix > 0.3 && mid.mix < 1.0, "{}", mid.mix);
        assert!(mid.squash > 0.01, "jiggles on the change");
        assert_eq!(a.next_change(&m, false), Some(Duration::from_millis(16)));
        let end = a.frame(&m, ms(3500), false);
        assert_eq!(end.from, None);
        assert!(end.mix > 0.99);
        // Reduced motion: no fade, no bob.
        a.set(State::NeedsYou, ms(4000));
        let r = a.frame(&m, ms(4000), true);
        assert_eq!(
            (r.state, r.mix, r.lift, r.squash),
            (State::NeedsYou, 1., 0., 0.)
        );
        assert_eq!(a.next_change(&m, true), None);
    }

    #[test]
    fn the_bubble_says_what_scout_is_doing() {
        assert_eq!(bubble(State::Idle, 0, None), None);
        assert_eq!(
            bubble(State::Thinking, 0, None).as_deref(),
            Some("thinking…")
        );
        assert_eq!(
            bubble(State::Reading, 3, None).as_deref(),
            Some("reading 3 sources")
        );
        assert_eq!(
            bubble(State::Reading, 1, None).as_deref(),
            Some("reading 1 source")
        );
        assert_eq!(
            bubble(State::NeedsYou, 0, None).as_deref(),
            Some("needs you")
        );
        let s = bubble(
            State::Searching,
            0,
            Some("how do agentic rag systems decide when to retrieve"),
        )
        .unwrap();
        assert!(s.ends_with("…”") && s.chars().count() < 45, "{s}");
    }

    #[test]
    fn the_final_art_format_reads_with_timing_and_overlays() {
        let m = Manifest::parse(
            r#"{"version": 2, "frame_w": 256, "frame_h": 256, "anchor_baseline_y": 244,
                "states": {"idle": {"file": "strips/scout_idle.png", "frames": 3, "fps": 4,
                   "durations_ms": [100, 200, 300]},
                 "needs_you": {"file": "n.png", "frames": 4, "fps": 6, "loop_start": 2,
                   "overlay": {"file": "overlays/needs_you.png", "bbox": [1, 2, 3, 4]}}}}"#,
        )
        .unwrap();
        assert_eq!(m.baseline, 244.);
        let idle = m.strip(State::Idle);
        assert_eq!(idle.durations, vec![100, 200, 300]);
        assert_eq!(idle.index_at(0), 0);
        assert_eq!(idle.index_at(150), 1);
        assert_eq!(idle.index_at(350), 2);
        assert_eq!(idle.index_at(650), 0, "loops");
        let n = m.strip(State::NeedsYou);
        assert_eq!(n.overlay.as_deref(), Some("overlays/needs_you.png"));
        // 166 ms a frame: the turn (0, 1) plays once, then 2 and 3 loop.
        assert_eq!(n.index_at(170), 1);
        assert_eq!(n.index_at(4 * 166 + 10), 2);
        assert_eq!(n.index_at(5 * 166 + 10), 3);
        assert_eq!(n.index_at(6 * 166 + 10), 2);
    }

    #[test]
    fn ctrl_or_cmd_dot_toggles_scout() {
        assert!(is_toggle_key(".", true, false, false, false));
        assert!(is_toggle_key(".", false, true, false, false));
        assert!(!is_toggle_key(".", false, false, false, false));
        assert!(!is_toggle_key(".", true, false, false, true));
        assert!(!is_toggle_key(",", true, false, false, false));
    }

    #[test]
    fn the_pet_stays_in_the_window() {
        assert_eq!(clamp_pos((-50., -50.), (90., 90.), (800., 600.)), (4., 40.));
        assert_eq!(
            clamp_pos((5000., 5000.), (90., 90.), (800., 600.)),
            (706., 506.)
        );
        assert_eq!(
            clamp_pos((100., 200.), (90., 90.), (800., 600.)),
            (100., 200.)
        );
    }

    #[test]
    fn every_mood_has_a_sprite() {
        for m in [
            Mood::NeedsYou,
            Mood::Blocked,
            Mood::Searching,
            Mood::Reading,
            Mood::Verifying,
            Mood::Thinking,
            Mood::Studying,
            Mood::Ready,
            Mood::Idle,
        ] {
            let _ = State::of(m);
        }
        assert_eq!(State::of(Mood::Ready), State::Done);
    }
}
