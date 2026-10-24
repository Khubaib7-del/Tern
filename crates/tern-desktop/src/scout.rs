//! Scout, the mascot (`docs/02-ui-definition.md` §7): a small hooded
//! scholar with a lantern and a scroll. Not a set of pictures but a rig:
//! the lantern hangs on a spring, the eyes blink on their own clock and
//! follow the work, the scroll opens to read. Every movement comes from a
//! real event; at rest Scout only breathes and blinks.
//!
//! GPUI-free so the motion is unit tested. The view samples [`Rig::pose`]
//! each frame and draws it.

/// What Scout is doing, in priority order (the first that applies wins).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mood {
    /// Plan approval, a permission gate, a question: faces you, lantern up.
    NeedsYou,
    /// An error or a budget limit: sits, scroll closed.
    Blocked,
    /// Search events: the lantern sweeps once per query.
    Searching,
    /// Fetching and parsing: scroll open, eyes run down the page.
    Reading,
    /// Claim checks: taps the scroll once per verdict.
    Verifying,
    /// Thinking or writing: still, eyes up.
    Thinking,
    /// Flashcard review: holds up a card (arm raised, scroll open) and
    /// reacts to each answer ([`Rig::react`]).
    Studying,
    /// A run finished: a small nod, then idle.
    Ready,
    #[default]
    Idle,
}

impl Mood {
    pub fn label(self) -> &'static str {
        match self {
            Mood::NeedsYou => "Needs you",
            Mood::Blocked => "Blocked",
            Mood::Searching => "Searching",
            Mood::Reading => "Reading",
            Mood::Verifying => "Verifying",
            Mood::Thinking => "Thinking",
            Mood::Studying => "Studying",
            Mood::Ready => "Ready",
            Mood::Idle => "Idle",
        }
    }
}

/// Scout's size in the sidebar corner, in pixels. Matched to v2 by the
/// figure, not the box: v2 draws a 56 px SVG whose figure (hood tip to
/// hem) spans 58 of its 64 units, about 51 px tall; Tern's rig spans 34
/// of its 48 units, so a 72 px box gives the same 51 px figure (it was
/// 64, a 45 px figure).
pub const CORNER: f32 = 72.;

/// The word beside Scout for its state, as v2's states are named
/// (searching, reading, verifying, thinking, needs you, done, study).
/// Idle shows nothing.
pub fn state_line(mood: Mood) -> Option<&'static str> {
    match mood {
        Mood::Idle => None,
        Mood::NeedsYou => Some("needs you"),
        Mood::Blocked => Some("blocked"),
        Mood::Searching => Some("searching"),
        Mood::Reading => Some("reading"),
        Mood::Verifying => Some("verifying"),
        Mood::Thinking => Some("thinking"),
        Mood::Studying => Some("studying"),
        Mood::Ready => Some("done"),
    }
}

/// How long the Ready nod lasts before Scout settles to idle.
pub const READY_SECS: f32 = 1.6;
/// How long Scout reacts to a flashcard answer.
pub const REACT_SECS: f32 = 0.9;

/// Scout's joints ride the shared closed-form spring
/// ([`crate::motion::Spring`] on [`SpringSpec::SCOUT`]): exact for any
/// frame gap, and a mood change mid-move keeps each joint's velocity, so
/// Scout flows from one pose into the next instead of snapping.
pub type Spring = crate::motion::Spring;
use crate::motion::SpringSpec;

fn joint() -> Spring {
    Spring::new(0.0, SpringSpec::SCOUT)
}

/// Everything the drawing needs for one frame. Units: the rig is drawn in
/// a 48×48 box; angles in radians; amounts 0 to 1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    /// Lantern swing about its hook, 0 hanging straight down.
    pub lantern: f32,
    /// How high the lantern arm is raised, 0 at the side, 1 held up.
    pub raise: f32,
    /// Whether the lantern's flame is lit (searching, needs you).
    pub lit: bool,
    /// How bright the flame is, 0 to 1: it fades up and down on a spring
    /// rather than switching.
    pub glow: f32,
    /// How far Scout has turned to face you, 0 to 1 (needs you): the face
    /// comes to the centre of the hood and the eyes look straight out.
    pub front: f32,
    /// 1 eyes open, 0 shut.
    pub eyes_open: f32,
    /// Where the eyes look, each -1 to 1.
    pub look_x: f32,
    pub look_y: f32,
    /// 0 rolled up, 1 open.
    pub scroll: f32,
    /// Breathing and the nod, as a vertical offset in px.
    pub bob: f32,
    /// Sitting lowers the whole figure.
    pub sit: f32,
}

/// Scout's state over time.
#[derive(Clone, Debug)]
pub struct Rig {
    mood: Mood,
    /// Seconds since the rig started.
    now: f32,
    mood_since: f32,
    lantern: Spring,
    raise: Spring,
    scroll: Spring,
    look_x: Spring,
    look_y: Spring,
    sit: Spring,
    /// Lid height between moods (Blocked droops), the flame's glow, and
    /// the turn to face you.
    lid: Spring,
    glow: Spring,
    front: Spring,
    /// Where the pointer is from Scout, in px, while it is near.
    pointer: Option<(f32, f32)>,
    /// When the next blink starts, and the current one if any.
    next_blink: f32,
    blink_at: Option<f32>,
    /// A sweep or tap from the last event, by its start time.
    kick_at: Option<f32>,
    /// A reaction to a flashcard answer: right or wrong, and when.
    reaction: Option<(bool, f32)>,
    seed: u32,
}

const BLINK_SECS: f32 = 0.16;

impl Default for Rig {
    fn default() -> Self {
        Self::new(7)
    }
}

impl Rig {
    pub fn new(seed: u32) -> Self {
        let mut rig = Self {
            mood: Mood::Idle,
            now: 0.0,
            mood_since: 0.0,
            lantern: joint(),
            raise: joint(),
            scroll: joint(),
            look_x: joint(),
            look_y: joint(),
            sit: joint(),
            lid: Spring::new(1.0, SpringSpec::SCOUT),
            glow: joint(),
            front: joint(),
            pointer: None,
            next_blink: 0.0,
            blink_at: None,
            kick_at: None,
            reaction: None,
            seed: seed.max(1),
        };
        rig.next_blink = rig.blink_gap();
        rig
    }

    pub fn mood(&self) -> Mood {
        self.mood
    }

    /// A pseudo-random gap of 2.5 to 6 seconds between blinks.
    fn blink_gap(&mut self) -> f32 {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 17;
        self.seed ^= self.seed << 5;
        2.5 + (self.seed % 1000) as f32 / 1000.0 * 3.5
    }

    /// Moves to `mood`. A run that ends (anything busy going to idle)
    /// passes through Ready for its nod.
    pub fn set_mood(&mut self, mood: Mood) {
        let mood = if mood == Mood::Idle
            && !matches!(self.mood, Mood::Idle | Mood::Ready | Mood::Blocked)
        {
            Mood::Ready
        } else {
            mood
        };
        if mood != self.mood {
            self.mood = mood;
            self.mood_since = self.now;
            self.kick_at = None;
        }
    }

    /// A real event happened (a query, a page, a verdict): sweep or tap.
    pub fn kick(&mut self) {
        if matches!(self.mood, Mood::Searching | Mood::Reading | Mood::Verifying) {
            self.kick_at = Some(self.now);
        }
    }

    /// A flashcard was answered: a nod for right, a small head shake for
    /// wrong.
    pub fn react(&mut self, right: bool) {
        self.reaction = Some((right, self.now));
    }

    /// The reaction in progress: right or wrong, and how far through (0
    /// to 1).
    fn reacting(&self) -> Option<(bool, f32)> {
        let (right, at) = self.reaction?;
        let t = (self.now - at) / REACT_SECS;
        (t < 1.0).then_some((right, t.max(0.0)))
    }

    fn targets(&self) -> (f32, f32, f32, f32, f32, f32) {
        // (lantern angle, raise, scroll, look x, look y, sit)
        match self.mood {
            Mood::NeedsYou => (0.0, 1.0, 0.0, 0.0, 0.0, 0.0),
            Mood::Blocked => (0.0, 0.0, 0.0, -0.3, 0.6, 1.0),
            Mood::Searching => (0.0, 0.35, 0.0, 0.6, -0.1, 0.0),
            Mood::Reading => (0.0, 0.0, 1.0, -0.5, 0.2, 0.0),
            Mood::Verifying => (0.0, 0.0, 1.0, -0.5, 0.4, 0.0),
            Mood::Thinking => (0.0, 0.0, 0.0, 0.2, -0.7, 0.0),
            Mood::Studying => (0.0, 0.7, 1.0, 0.3, -0.3, 0.0),
            Mood::Ready | Mood::Idle => (0.0, 0.0, 0.0, 0.0, 0.0, 0.0),
        }
    }

    /// (lid height, flame glow, turn to face you) for the mood.
    fn extras(&self) -> (f32, f32, f32) {
        match self.mood {
            Mood::NeedsYou => (1.0, 1.0, 1.0),
            Mood::Blocked => (0.35, 0.0, 0.0),
            Mood::Searching => (1.0, 1.0, 0.0),
            Mood::Ready => (1.0, 0.0, 0.5),
            _ => (1.0, 0.0, 0.0),
        }
    }

    /// Where the pointer is relative to Scout's face, in px; `None` when
    /// it is outside the window.
    pub fn set_pointer(&mut self, at: Option<(f32, f32)>) {
        self.pointer = at;
    }

    /// Whether the pointer is near enough to move the eyes.
    pub fn watching(&self) -> bool {
        self.pointer
            .is_some_and(|(dx, dy)| (dx * dx + dy * dy).sqrt() < 380.0)
    }

    /// Advances the rig by `dt` seconds.
    pub fn step(&mut self, dt: f32) {
        self.now += dt;
        if self.mood == Mood::Ready && self.now - self.mood_since > READY_SECS {
            self.mood = Mood::Idle;
            self.mood_since = self.now;
        }
        let (mut lantern, raise, scroll, mut lx, mut ly, sit) = self.targets();
        let t = self.now - self.mood_since;
        match self.mood {
            // The eyes run down the page and back to the top.
            Mood::Reading => {
                ly = -0.4 + ((t * 0.9) % 1.0) * 1.0;
                lx = -0.5 + ((t * 2.7).sin() * 0.25);
            }
            // While searching the lantern drifts, and swings on each query.
            Mood::Searching => lantern = (t * 1.3).sin() * 0.12,
            _ => {}
        }
        if let Some(k) = self.kick_at {
            let since = self.now - k;
            if since < 0.6 {
                match self.mood {
                    Mood::Searching => lantern += 0.5 * (1.0 - since / 0.6),
                    Mood::Verifying => ly += 0.5 * (1.0 - since / 0.6),
                    _ => {}
                }
            } else {
                self.kick_at = None;
            }
        }
        match self.reacting() {
            // Wrong: the eyes shake side to side, fading out.
            Some((false, t)) => lx += (t * 18.0).sin() * 0.8 * (1.0 - t),
            // Right: eyes up, toward you.
            Some((true, _)) => ly -= 0.4,
            None => self.reaction = None,
        }
        // The eyes follow the pointer when it comes near: fully when Scout
        // is idle or facing you, a little while it works (v2's corner).
        if let Some((dx, dy)) = self.pointer {
            let dist = (dx * dx + dy * dy).sqrt();
            let near = (1.0 - (dist - 120.0) / 260.0).clamp(0.0, 1.0);
            let w = match self.mood {
                Mood::Idle | Mood::Ready | Mood::NeedsYou => near,
                _ => near * 0.35,
            };
            let (px_, py_) = ((dx / 140.0).clamp(-1.0, 1.0), (dy / 140.0).clamp(-1.0, 1.0));
            lx += (px_ - lx) * w;
            ly += (py_ - ly) * w;
        }
        // Moving the arm swings the lantern by itself.
        let arm_before = self.raise.value();
        self.raise.advance(raise, dt);
        self.lantern.kick(-(self.raise.value() - arm_before) * 6.0);
        self.lantern.advance(lantern, dt);
        self.scroll.advance(scroll, dt);
        self.look_x.advance(lx, dt);
        self.look_y.advance(ly, dt);
        self.sit.advance(sit, dt);
        let (lid, glow, front) = self.extras();
        self.lid.advance(lid, dt);
        self.glow.advance(glow, dt);
        self.front.advance(front, dt);

        if let Some(b) = self.blink_at {
            if self.now - b > BLINK_SECS {
                self.blink_at = None;
            }
        } else if self.now >= self.next_blink && self.mood != Mood::Blocked {
            self.blink_at = Some(self.now);
            self.next_blink = self.now + self.blink_gap();
        }
    }

    pub fn pose(&self) -> Pose {
        let eyes_open = match self.blink_at {
            Some(b) => {
                let x = ((self.now - b) / BLINK_SECS).clamp(0.0, 1.0);
                (2.0 * x - 1.0).abs()
            }
            None => 1.0,
        } * self.lid.value().clamp(0.2, 1.0);
        let breath = (self.now * std::f32::consts::TAU / 4.0).sin() * 0.4;
        let nod = if self.mood == Mood::Ready {
            let t = (self.now - self.mood_since) / 0.5;
            if t < 1.0 {
                (t * std::f32::consts::PI).sin() * 2.2
            } else {
                0.0
            }
        } else if let Some((true, t)) = self.reacting() {
            // Two quick nods.
            (t * std::f32::consts::TAU).sin().abs() * 2.0 * (1.0 - t)
        } else {
            0.0
        };
        Pose {
            lantern: self.lantern.value(),
            raise: self.raise.value().clamp(0.0, 1.2),
            lit: self.glow.value() > 0.05,
            glow: self.glow.value().clamp(0.0, 1.0),
            front: self.front.value().clamp(0.0, 1.0),
            eyes_open,
            look_x: self.look_x.value().clamp(-1.0, 1.0),
            look_y: self.look_y.value().clamp(-1.0, 1.0),
            scroll: self.scroll.value().clamp(0.0, 1.0),
            bob: breath + nod,
            sit: self.sit.value().clamp(0.0, 1.0),
        }
    }

    /// Whether the next frame differs enough from this one to draw it
    /// right away. At rest only breathing moves, which a slower tick
    /// covers.
    ///
    /// Only change counts: a spring still moving, a blink, a nod, a kick,
    /// or a reaction. A mood that holds still (Thinking, Blocked, NeedsYou)
    /// is not busy once settled, so a long answer or an error does not
    /// redraw the window every frame.
    pub fn busy(&self) -> bool {
        let (lantern, raise, scroll, lx, ly, sit) = self.targets();
        let ambient = self.ambient();
        let (lid, glow, front) = self.extras();
        self.reacting().is_some()
            || !self.lid.settled(lid)
            || !self.glow.settled(glow)
            || !self.front.settled(front)
            || self.blink_at.is_some()
            || self.kick_at.is_some()
            || self.mood == Mood::Ready
            || !self.raise.settled(raise)
            || !self.scroll.settled(scroll)
            || !self.sit.settled(sit)
            || (!ambient
                && (!self.lantern.settled(lantern)
                    || !self.look_x.settled(lx)
                    || !self.look_y.settled(ly)))
    }

    /// Moods that keep drifting (the eyes reading down a page, the lantern
    /// swaying while searching). They are drawn on a slow tick, not every
    /// frame.
    pub fn ambient(&self) -> bool {
        matches!(self.mood, Mood::Reading | Mood::Searching)
    }

    /// Seconds until the next blink, so an idle Scout can sleep until then.
    pub fn until_blink(&self) -> f32 {
        (self.next_blink - self.now).max(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(rig: &mut Rig, secs: f32) {
        for _ in 0..(secs * 60.0) as usize {
            rig.step(1.0 / 60.0);
        }
    }

    #[test]
    fn a_spring_settles_on_its_target() {
        let mut s = joint();
        for _ in 0..240 {
            s.advance(1.0, 1.0 / 60.0);
        }
        assert!(s.settled(1.0), "{s:?}");
        // A late frame does not blow it up.
        s.advance(-1.0, 5.0);
        assert!(s.value().is_finite() && s.value().abs() < 2.0);
    }

    #[test]
    fn moods_blend_with_no_snapping() {
        // Every pair of moods: no joint jumps between two 60 fps frames.
        let all = [
            Mood::Idle,
            Mood::Thinking,
            Mood::Searching,
            Mood::Reading,
            Mood::Verifying,
            Mood::NeedsYou,
            Mood::Blocked,
            Mood::Studying,
        ];
        for from in all {
            for to in all {
                let mut rig = Rig::new(11);
                rig.set_mood(from);
                run(&mut rig, 2.0);
                let mut last = rig.pose();
                rig.set_mood(to);
                for _ in 0..90 {
                    rig.step(1.0 / 60.0);
                    let p = rig.pose();
                    for (a, b, what) in [
                        (last.raise, p.raise, "raise"),
                        (last.scroll, p.scroll, "scroll"),
                        (last.sit, p.sit, "sit"),
                        (last.glow, p.glow, "glow"),
                        (last.front, p.front, "front"),
                    ] {
                        assert!((a - b).abs() < 0.2, "{from:?}->{to:?} {what} {a}->{b}");
                    }
                    last = p;
                }
            }
        }
    }

    #[test]
    fn needing_you_turns_scout_to_face_you_with_the_lantern_up() {
        let mut rig = Rig::new(4);
        rig.set_mood(Mood::NeedsYou);
        run(&mut rig, 2.0);
        let p = rig.pose();
        assert!(p.front > 0.95 && p.glow > 0.95 && p.raise > 0.9, "{p:?}");
        assert!(
            p.look_x.abs() < 0.1 && p.look_y.abs() < 0.1,
            "looks straight out"
        );
        rig.set_mood(Mood::Reading);
        run(&mut rig, 2.0);
        assert!(rig.pose().front < 0.05 && rig.pose().glow < 0.05);
    }

    #[test]
    fn idle_eyes_follow_a_near_pointer() {
        let mut rig = Rig::new(8);
        rig.set_pointer(Some((90.0, 0.0)));
        run(&mut rig, 1.5);
        assert!(rig.pose().look_x > 0.4, "{:?}", rig.pose());
        rig.set_pointer(Some((-2000.0, 0.0)));
        run(&mut rig, 1.5);
        assert!(rig.pose().look_x.abs() < 0.05, "far away: back to rest");
    }

    #[test]
    fn scout_blinks_on_its_own_and_rests_between() {
        let mut rig = Rig::new(42);
        let mut blinks = 0;
        let mut was_open = true;
        for _ in 0..(20 * 60) {
            rig.step(1.0 / 60.0);
            let open = rig.pose().eyes_open > 0.5;
            if was_open && !open {
                blinks += 1;
            }
            was_open = open;
        }
        assert!((3..=8).contains(&blinks), "{blinks} blinks in 20 s");
        assert!(rig.until_blink() <= 6.0);
    }

    #[test]
    fn moods_move_the_rig() {
        let mut rig = Rig::new(3);
        rig.set_mood(Mood::NeedsYou);
        run(&mut rig, 2.0);
        let p = rig.pose();
        assert!(p.raise > 0.9 && p.lit);

        rig.set_mood(Mood::Reading);
        run(&mut rig, 2.0);
        assert!(rig.pose().scroll > 0.9 && !rig.pose().lit);

        rig.set_mood(Mood::Searching);
        run(&mut rig, 1.0);
        let before = rig.pose().lantern;
        rig.kick();
        run(&mut rig, 0.15);
        assert!(
            rig.pose().lantern > before + 0.1,
            "a query swings the lantern"
        );

        // The run ends: a nod, then idle and still.
        rig.set_mood(Mood::Idle);
        assert_eq!(rig.mood(), Mood::Ready);
        run(&mut rig, READY_SECS + 2.5);
        assert_eq!(rig.mood(), Mood::Idle);
        let p = rig.pose();
        assert!(p.scroll < 0.05 && p.raise < 0.05 && p.lantern.abs() < 0.05);

        rig.set_mood(Mood::Blocked);
        run(&mut rig, 2.0);
        assert!(rig.pose().sit > 0.9 && rig.pose().eyes_open < 0.5);
        // Leaving Blocked does not nod.
        rig.set_mood(Mood::Idle);
        assert_eq!(rig.mood(), Mood::Idle);
    }

    #[test]
    fn still_moods_stop_asking_for_frames() {
        let mut rig = Rig::new(9);
        rig.set_mood(Mood::Thinking);
        assert!(rig.busy(), "moving into the pose");
        // Settle, staying clear of a blink.
        for _ in 0..40 {
            run(&mut rig, 0.25);
            if rig.blink_at.is_none() && !rig.busy() {
                break;
            }
        }
        assert!(
            !rig.busy(),
            "a long Thinking turn does not redraw every frame"
        );
        assert!(!rig.ambient());
        rig.set_mood(Mood::Reading);
        assert!(rig.ambient(), "reading drifts on a slow tick");
    }

    #[test]
    fn studying_holds_up_a_card_and_reacts() {
        let mut rig = Rig::new(5);
        rig.set_mood(Mood::Studying);
        run(&mut rig, 2.0);
        let p = rig.pose();
        assert!(p.raise > 0.6 && p.scroll > 0.9, "{p:?}");
        let rest = rig.pose().bob;
        rig.react(true);
        run(&mut rig, 0.25);
        assert!(rig.pose().bob > rest + 0.5, "a nod");
        assert!(rig.busy());
        run(&mut rig, 1.0);
        rig.react(false);
        let mut xs = Vec::new();
        for _ in 0..30 {
            rig.step(1.0 / 60.0);
            xs.push(rig.pose().look_x);
        }
        let spread = xs.iter().cloned().fold(f32::MIN, f32::max)
            - xs.iter().cloned().fold(f32::MAX, f32::min);
        assert!(spread > 0.1, "a head shake: {spread}");
        run(&mut rig, 2.0);
        assert!(rig.reacting().is_none());
    }

    #[test]
    fn the_corner_is_bigger_and_names_each_state() {
        const { assert!(CORNER >= 70. && CORNER <= 74.) };
        assert_eq!(state_line(Mood::Idle), None);
        assert_eq!(state_line(Mood::Ready), Some("done"));
        assert_eq!(state_line(Mood::NeedsYou), Some("needs you"));
        for m in [
            Mood::Searching,
            Mood::Reading,
            Mood::Verifying,
            Mood::Thinking,
            Mood::Studying,
            Mood::Blocked,
        ] {
            assert!(state_line(m).is_some(), "{m:?}");
        }
    }
}
