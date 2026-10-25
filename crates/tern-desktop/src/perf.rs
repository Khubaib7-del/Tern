//! What each frame costs, by what is on screen.
//!
//! The window's root (`view::Shell`) marks the start of every frame it
//! draws and a canvas at the end of its tree marks the end of paint, so a
//! frame's time is its whole CPU side: render, layout, prepaint and paint
//! (not the GPU). Canvases split it into phases: one first in the
//! root's tree and one first in the workspace's mark the end of
//! `layout` (render, request_layout and the taffy pass; the cached
//! workspace renders and lays out inside the root's prepaint, so the
//! later mark counts), the root's first canvas paints when `prepaint`
//! ends (with the deferred popovers), and the last canvas's paint ends
//! `paint`. The workspace says when it rendered (a frame drawn with
//! the workspace cached is only Scout) and how long building its elements
//! took, under a tag for the screen (`settings/providers`, `chat`).
//!
//! Every [`WINDOW`] the frames so far are summed up in one line
//! (`perf settings/providers: 61 frames in 2.0 s, 61 workspace renders,
//! frame mean 9.1 ms p95 14.0 ms max 21.3 ms, build mean 2.2 ms`) and
//! written to `tern.log` when a frame was slow or the workspace kept
//! redrawing with nothing moving; with `TERN_PERF=1` every line also goes
//! to stderr, and the UI tour prints them as `PERF` lines. A screen that
//! redraws the whole workspace 60 times a second while idle, or whose
//! frames take far longer than another's, shows up here.
//!
//! GPUI-free so it is unit tested.

use std::time::{Duration, Instant};

/// How long one summary covers.
pub const WINDOW: Duration = Duration::from_secs(2);
/// A frame slower than this is worth a line in `tern.log`.
pub const SLOW_MS: f32 = 24.;

/// One summary of a window of frames.
#[derive(Clone, Debug, PartialEq)]
pub struct Summary {
    pub tag: String,
    pub secs: f32,
    pub frames: usize,
    pub workspace_renders: usize,
    pub frame_mean: f32,
    pub frame_p95: f32,
    pub frame_max: f32,
    pub build_mean: f32,
    /// Mean of each phase, ms: render + layout, prepaint, paint.
    pub layout_mean: f32,
    pub prepaint_mean: f32,
    pub paint_mean: f32,
}

impl Summary {
    /// Whether this window is worth writing down outside a profile: a
    /// slow frame, or the workspace drawn on most frames for a whole
    /// window (something keeps repainting it).
    pub fn notable(&self) -> bool {
        self.frame_max > SLOW_MS
            || (self.secs > 1.5 && self.workspace_renders as f32 > 40. * self.secs)
    }

    pub fn line(&self) -> String {
        format!(
            "perf {}: {} frames in {:.1} s, {} workspace renders, frame mean {:.1} ms p95 {:.1} ms max {:.1} ms, build mean {:.1} ms, layout {:.1} prepaint {:.1} paint {:.1} ms",
            self.tag,
            self.frames,
            self.secs,
            self.workspace_renders,
            self.frame_mean,
            self.frame_p95,
            self.frame_max,
            self.build_mean,
            self.layout_mean,
            self.prepaint_mean,
            self.paint_mean
        )
    }
}

/// The frames since the last summary.
#[derive(Debug, Default)]
pub struct Meter {
    tag: String,
    since: Option<Instant>,
    frame_start: Option<Instant>,
    prepaint_at: Option<Instant>,
    paint_at: Option<Instant>,
    frames: Vec<f32>,
    builds: Vec<f32>,
    /// Per frame: render + layout, prepaint, paint (ms).
    phases: Vec<[f32; 3]>,
}

fn mean(v: &[f32]) -> f32 {
    if v.is_empty() {
        0.
    } else {
        v.iter().sum::<f32>() / v.len() as f32
    }
}

fn p95(v: &[f32]) -> f32 {
    if v.is_empty() {
        return 0.;
    }
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.total_cmp(b));
    let i = ((s.len() as f32 * 0.95).ceil() as usize).clamp(1, s.len()) - 1;
    s[i]
}

impl Meter {
    /// A frame starts drawing at `now`.
    pub fn frame_start(&mut self, now: Instant) {
        self.since.get_or_insert(now);
        self.frame_start = Some(now);
        self.prepaint_at = None;
        self.paint_at = None;
    }

    /// An element marking the end of layout prepaints at `now`. The
    /// window's root marks it, and so does the workspace when it renders:
    /// a cached view renders and lays out inside its parent's prepaint,
    /// so the last mark before paint is where layout ended.
    pub fn prepaint_start(&mut self, now: Instant) {
        if self.frame_start.is_some() && self.paint_at.is_none() {
            self.prepaint_at = Some(now);
        }
    }

    /// The frame's first element started painting at `now`.
    pub fn paint_start(&mut self, now: Instant) {
        if self.frame_start.is_some() {
            self.paint_at.get_or_insert(now);
        }
    }

    /// The workspace rendered this frame, showing `tag`, in `build`.
    /// A new tag ends the window first (returned), so each summary is one
    /// screen.
    pub fn workspace(&mut self, tag: &str, build: Duration, now: Instant) -> Option<Summary> {
        let done = if tag != self.tag && !self.frames.is_empty() {
            self.flush(now)
        } else {
            None
        };
        if tag != self.tag {
            self.tag = tag.to_owned();
        }
        self.builds.push(build.as_secs_f32() * 1000.);
        done
    }

    /// The frame that started last finished painting at `now`. Returns a
    /// summary when the window is full.
    pub fn frame_end(&mut self, now: Instant) -> Option<Summary> {
        let start = self.frame_start.take()?;
        let ms = |a: Instant, b: Instant| b.saturating_duration_since(a).as_secs_f32() * 1000.;
        self.frames.push(ms(start, now));
        // A phase mark missing (a frame drawn without the first canvas)
        // folds that phase into the one before it.
        let pre = self.prepaint_at.take().unwrap_or(now).max(start);
        let paint = self.paint_at.take().unwrap_or(now).max(pre);
        self.phases
            .push([ms(start, pre), ms(pre, paint), ms(paint, now)]);
        let since = *self.since.get_or_insert(start);
        (now.saturating_duration_since(since) >= WINDOW).then(|| self.flush(now))?
    }

    /// Sums up the window so far and starts a new one.
    pub fn flush(&mut self, now: Instant) -> Option<Summary> {
        let since = self.since.take()?;
        let frames = std::mem::take(&mut self.frames);
        let builds = std::mem::take(&mut self.builds);
        let phases = std::mem::take(&mut self.phases);
        if frames.is_empty() {
            return None;
        }
        let phase = |i: usize| mean(&phases.iter().map(|p| p[i]).collect::<Vec<_>>());
        Some(Summary {
            tag: if self.tag.is_empty() {
                "start".to_owned()
            } else {
                self.tag.clone()
            },
            secs: now.saturating_duration_since(since).as_secs_f32(),
            frames: frames.len(),
            workspace_renders: builds.len(),
            frame_mean: mean(&frames),
            frame_p95: p95(&frames),
            frame_max: frames.iter().copied().fold(0., f32::max),
            build_mean: mean(&builds),
            layout_mean: phase(0),
            prepaint_mean: phase(1),
            paint_mean: phase(2),
        })
    }
}

thread_local! {
    static METER: std::cell::RefCell<Meter> = std::cell::RefCell::new(Meter::default());
}

/// Writes a summary down: `tern.log` when notable, stderr under
/// `TERN_PERF`, stdout as a `PERF` line in the UI tour.
fn report(s: Option<Summary>) {
    let Some(s) = s else {
        return;
    };
    let line = s.line();
    if verbose() {
        eprintln!("{line}");
    }
    if crate::view::tour::enabled() {
        use std::io::Write as _;
        println!("PERF {line}");
        let _ = std::io::stdout().flush();
    }
    if s.notable() {
        crate::crashlog::log_line(&line);
    }
}

/// A frame starts (the window's root renders).
pub fn start() {
    METER.with(|m| m.borrow_mut().frame_start(Instant::now()));
}

/// The first element of the frame prepaints (render and layout done).
pub fn prepaint_start() {
    METER.with(|m| m.borrow_mut().prepaint_start(Instant::now()));
}

/// The first element of the frame paints (prepaint done).
pub fn paint_start() {
    METER.with(|m| m.borrow_mut().paint_start(Instant::now()));
}

/// The workspace rendered, showing `tag`, in `build`.
pub fn workspace(tag: &str, build: Duration) {
    report(METER.with(|m| m.borrow_mut().workspace(tag, build, Instant::now())));
}

/// The frame finished painting.
pub fn end() {
    report(METER.with(|m| m.borrow_mut().frame_end(Instant::now())));
}

/// Whether every summary goes to stderr (`TERN_PERF` set, not `0`).
pub fn verbose() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("TERN_PERF").is_some_and(|v| !v.is_empty() && v != "0"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn a_window_of_frames_sums_up_into_one_line() {
        let t0 = Instant::now();
        let mut m = Meter::default();
        let mut out = None;
        for i in 0..130u64 {
            let at = t0 + ms(i * 16);
            m.frame_start(at);
            if i % 2 == 0 {
                assert!(m.workspace("settings/providers", ms(3), at).is_none());
            }
            if let Some(s) = m.frame_end(at + ms(if i == 5 { 30 } else { 8 })) {
                out = Some(s);
                break;
            }
        }
        let s = out.expect("a summary after two seconds");
        assert_eq!(s.tag, "settings/providers");
        assert!((120..=127).contains(&s.frames), "{}", s.frames);
        assert_eq!(s.workspace_renders, s.frames.div_ceil(2));
        assert_eq!(s.frame_max, 30.);
        assert_eq!(s.frame_p95, 8.);
        assert!((s.build_mean - 3.).abs() < 0.01);
        assert!(s.notable(), "a 30 ms frame is worth a line");
        assert!(s.line().starts_with("perf settings/providers: "));
    }

    #[test]
    fn each_frame_splits_into_layout_prepaint_and_paint() {
        let t0 = Instant::now();
        let mut m = Meter::default();
        // Marks before a frame starts are ignored.
        m.prepaint_start(t0);
        m.frame_start(t0);
        m.workspace("settings/providers", ms(2), t0);
        m.prepaint_start(t0 + ms(3));
        m.prepaint_start(t0 + ms(7)); // the last before paint counts
        m.paint_start(t0 + ms(9));
        m.prepaint_start(t0 + ms(10)); // after paint: ignored
        assert!(m.frame_end(t0 + ms(13)).is_none());
        // A frame with no phase marks counts as all layout.
        m.frame_start(t0 + ms(20));
        assert!(m.frame_end(t0 + ms(24)).is_none());
        let s = m.flush(t0 + ms(30)).expect("two frames");
        assert_eq!(s.frames, 2);
        assert!((s.layout_mean - 5.).abs() < 0.01, "{}", s.layout_mean);
        assert!((s.prepaint_mean - 1.5).abs() < 0.01, "{}", s.prepaint_mean);
        assert!((s.paint_mean - 2.).abs() < 0.01, "{}", s.paint_mean);
        assert!(
            s.line().ends_with("layout 5.0 prepaint 1.5 paint 2.0 ms"),
            "{}",
            s.line()
        );
    }

    #[test]
    fn a_new_screen_ends_the_window_and_an_idle_one_is_quiet() {
        let t0 = Instant::now();
        let mut m = Meter::default();
        m.frame_start(t0);
        m.workspace("settings/tools", ms(1), t0);
        assert!(m.frame_end(t0 + ms(5)).is_none());
        m.frame_start(t0 + ms(100));
        let s = m
            .workspace("settings/providers", ms(1), t0 + ms(100))
            .expect("the tools window ends");
        assert_eq!((s.tag.as_str(), s.frames), ("settings/tools", 1));
        assert!(!s.notable(), "one quick frame is nothing to log");
        // Nothing drawn: nothing to sum up.
        assert!(Meter::default().flush(t0).is_none());
        assert!(Meter::default().frame_end(t0).is_none());
    }

    #[test]
    fn a_workspace_redrawn_every_frame_for_a_whole_window_is_notable() {
        let s = Summary {
            tag: "settings/providers".into(),
            secs: 2.,
            frames: 120,
            workspace_renders: 120,
            frame_mean: 4.,
            frame_p95: 5.,
            frame_max: 6.,
            build_mean: 1.,
            layout_mean: 1.,
            prepaint_mean: 1.,
            paint_mean: 2.,
        };
        assert!(s.notable());
        let calm = Summary {
            workspace_renders: 3,
            ..s.clone()
        };
        assert!(!calm.notable());
    }
}
