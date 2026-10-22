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
    /// A run finished: a small nod, then idle.
    Ready,
    #[default]
    Idle,
}

impl Mood {
    pub const ALL: [Mood; 8] = [
        Mood::Idle,
        Mood::Thinking,
        Mood::Searching,
        Mood::Reading,
        Mood::Verifying,
        Mood::NeedsYou,
        Mood::Blocked,
        Mood::Ready,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Mood::NeedsYou => "Needs you",
            Mood::Blocked => "Blocked",
            Mood::Searching => "Searching",
            Mood::Reading => "Reading",
            Mood::Verifying => "Verifying",
            Mood::Thinking => "Thinking",
            Mood::Ready => "Ready",
            Mood::Idle => "Idle",
        }
    }
}

/// How long the Ready nod lasts before Scout settles to idle.
pub const READY_SECS: f32 = 1.6;

/// A damped spring on one number.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Spring {
    pub value: f32,
    pub velocity: f32,
}

impl Spring {
    const STIFFNESS: f32 = 90.0;
    const DAMPING: f32 = 9.0;

    pub fn step(&mut self, target: f32, dt: f32) {
        // Small fixed steps keep it stable when a frame comes late.
        let mut left = dt.min(0.25);
        while left > 0.0 {
            let h = left.min(1.0 / 120.0);
            let accel = Self::STIFFNESS * (target - self.value) - Self::DAMPING * self.velocity;
            self.velocity += accel * h;
            self.value += self.velocity * h;
            left -= h;
        }
    }

    pub fn settled(&self, target: f32) -> bool {
        (self.value - target).abs() < 0.002 && self.velocity.abs() < 0.01
    }
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
    /// When the next blink starts, and the current one if any.
    next_blink: f32,
    blink_at: Option<f32>,
    /// A sweep or tap from the last event, by its start time.
    kick_at: Option<f32>,
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
            lantern: Spring::default(),
            raise: Spring::default(),
            scroll: Spring::default(),
            look_x: Spring::default(),
            look_y: Spring::default(),
            sit: Spring::default(),
            next_blink: 0.0,
            blink_at: None,
            kick_at: None,
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

    fn targets(&self) -> (f32, f32, f32, f32, f32, f32) {
        // (lantern angle, raise, scroll, look x, look y, sit)
        match self.mood {
            Mood::NeedsYou => (0.0, 1.0, 0.0, 0.0, 0.0, 0.0),
            Mood::Blocked => (0.0, 0.0, 0.0, -0.3, 0.6, 1.0),
            Mood::Searching => (0.0, 0.35, 0.0, 0.6, -0.1, 0.0),
            Mood::Reading => (0.0, 0.0, 1.0, -0.5, 0.2, 0.0),
            Mood::Verifying => (0.0, 0.0, 1.0, -0.5, 0.4, 0.0),
            Mood::Thinking => (0.0, 0.0, 0.0, 0.2, -0.7, 0.0),
            Mood::Ready | Mood::Idle => (0.0, 0.0, 0.0, 0.0, 0.0, 0.0),
        }
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
        // Moving the arm swings the lantern by itself.
        let arm_before = self.raise.value;
        self.raise.step(raise, dt);
        self.lantern.velocity -= (self.raise.value - arm_before) * 6.0;
        self.lantern.step(lantern, dt);
        self.scroll.step(scroll, dt);
        self.look_x.step(lx, dt);
        self.look_y.step(ly, dt);
        self.sit.step(sit, dt);

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
            None if self.mood == Mood::Blocked => 0.35,
            None => 1.0,
        };
        let breath = (self.now * std::f32::consts::TAU / 4.0).sin() * 0.4;
        let nod = if self.mood == Mood::Ready {
            let t = (self.now - self.mood_since) / 0.5;
            if t < 1.0 {
                (t * std::f32::consts::PI).sin() * 2.2
            } else {
                0.0
            }
        } else {
            0.0
        };
        Pose {
            lantern: self.lantern.value,
            raise: self.raise.value.clamp(0.0, 1.2),
            lit: matches!(self.mood, Mood::Searching | Mood::NeedsYou),
            eyes_open,
            look_x: self.look_x.value.clamp(-1.0, 1.0),
            look_y: self.look_y.value.clamp(-1.0, 1.0),
            scroll: self.scroll.value.clamp(0.0, 1.0),
            bob: breath + nod,
            sit: self.sit.value.clamp(0.0, 1.0),
        }
    }

    /// Whether the next frame differs enough from this one to draw it
    /// right away. At rest only breathing moves, which a slower tick
    /// covers.
    pub fn busy(&self) -> bool {
        let (lantern, raise, scroll, _, _, sit) = self.targets();
        self.mood != Mood::Idle
            || self.blink_at.is_some()
            || !self.lantern.settled(lantern)
            || !self.raise.settled(raise)
            || !self.scroll.settled(scroll)
            || !self.sit.settled(sit)
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
        let mut s = Spring::default();
        for _ in 0..240 {
            s.step(1.0, 1.0 / 60.0);
        }
        assert!(s.settled(1.0), "{s:?}");
        // A late frame does not blow it up.
        s.step(-1.0, 5.0);
        assert!(s.value.is_finite() && s.value.abs() < 2.0);
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
}
