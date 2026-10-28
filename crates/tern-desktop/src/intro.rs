//! The first-run intro (#76): plays once, on the first launch, before
//! onboarding, with no skip. A dithered field reveals over the whole
//! window and leans towards the cursor; the tern (the mark rig's own
//! frames, `assets/mark/intro_bird_dark.png`) flies in with real
//! wingbeats, glides, and settles in the middle as the still mark; it
//! turns to metal and then to white (`intro_metal.png`), slides aside, and
//! a wipe reveals "TERN" with *Research · Study* beside it. Then the
//! intro fades and onboarding starts.
//!
//! This file is the timeline and the background field, as plain functions
//! of time so they can be tested; `view/intro_view.rs` draws them.

/// When each part starts and how long it runs, in seconds.
pub const REVEAL: (f32, f32) = (0.0, 1.8);
pub const FLY: (f32, f32) = (0.5, 3.0);
pub const SETTLE: (f32, f32) = (3.5, 1.2);
pub const METAL: (f32, f32) = (4.8, 1.1);
pub const SLIDE: (f32, f32) = (6.0, 0.9);
pub const WORDS: (f32, f32) = (6.25, 0.9);
pub const SUBTITLE: (f32, f32) = (6.9, 0.7);
pub const FADE: (f32, f32) = (8.7, 0.6);
/// The whole intro.
pub const TOTAL: f32 = FADE.0 + FADE.1;

/// The rig's frame rate and its loop.
const FPS: f32 = 30.;
/// Frames 0-35 are the three wingbeats; 36-71 glide and come to rest.
const BEAT_FRAMES: u32 = 36;
const LOOP_FRAMES: u32 = 72;
/// Frames in the metal sheet.
pub const METAL_FRAMES: u32 = 12;

/// Where things are at one moment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    /// How far the background has revealed (0 to 1).
    pub reveal: f32,
    /// How bright the background is (it dims under the lockup).
    pub field: f32,
    /// The bird's centre, as an offset from the window's centre in units
    /// of the bird's height, and its scale (1 = settled size).
    pub bird: (f32, f32),
    pub scale: f32,
    /// The dithered frame to draw (0 to 71), while it is dithered.
    pub frame: Option<u32>,
    /// The metal frame (0 to 11) and how much of it shows over the
    /// dithered bird, once it turns.
    pub metal: Option<(u32, f32)>,
    /// How far the bird has slid towards its lockup place (0 to 1).
    pub slide: f32,
    /// How far the wipe has revealed "TERN" (0 to 1), and the subtitle's
    /// opacity.
    pub wipe: f32,
    pub subtitle: f32,
    /// The whole intro's opacity (it fades out at the end).
    pub alpha: f32,
}

fn clamp01(x: f32) -> f32 {
    x.clamp(0., 1.)
}

/// Progress through a part, 0 before it, 1 after.
pub fn part(t: f32, (start, len): (f32, f32)) -> f32 {
    clamp01((t - start) / len)
}

/// Cubic ease in and out.
pub fn ease(x: f32) -> f32 {
    let x = clamp01(x);
    if x < 0.5 {
        4. * x * x * x
    } else {
        1. - (-2. * x + 2.).powi(3) / 2.
    }
}

/// Ease out (decelerating), for the arrival.
pub fn ease_out(x: f32) -> f32 {
    1. - (1. - clamp01(x)).powi(3)
}

/// The pose at `t` seconds.
pub fn pose(t: f32) -> Pose {
    let reveal = ease_out(part(t, REVEAL));
    // Flight: in from the upper left, along a shallow arc (the mark dives
    // down and to the right), slowing into the middle.
    let f = part(t, FLY);
    let k = ease_out(f);
    let start = (-3.2_f32, -2.1_f32);
    let arc = (k * std::f32::consts::PI).sin() * 0.35;
    let mut bird = (start.0 * (1. - k), start.1 * (1. - k) - arc);
    // A small rise and fall with each beat while it flies.
    let flying = t >= FLY.0 && t < SETTLE.0;
    if flying {
        bird.1 -= 0.03 * ((t - FLY.0) * FPS / 12. * std::f32::consts::TAU).sin() * (1. - k);
    }
    let scale = 0.55 + 0.45 * k;
    let frame = if t < FLY.0 {
        None
    } else if t < SETTLE.0 {
        Some(((t - FLY.0) * FPS) as u32 % BEAT_FRAMES)
    } else if t < SETTLE.0 + SETTLE.1 {
        Some((BEAT_FRAMES + ((t - SETTLE.0) * FPS) as u32).min(LOOP_FRAMES - 1))
    } else {
        Some(0)
    };
    let m = part(t, METAL);
    let metal = (t >= METAL.0).then(|| {
        let i = ((m * METAL_FRAMES as f32) as u32).min(METAL_FRAMES - 1);
        (i, clamp01(m * 4.))
    });
    let slide = ease(part(t, SLIDE));
    let wipe = ease(part(t, WORDS));
    let subtitle = ease(part(t, SUBTITLE));
    let field = 1. - 0.55 * slide;
    let alpha = 1. - ease(part(t, FADE));
    Pose {
        reveal,
        field,
        bird,
        scale,
        frame: if metal.is_some_and(|(_, a)| a >= 1.) {
            None
        } else {
            frame
        },
        metal,
        slide,
        wipe,
        subtitle,
        alpha,
    }
}

/// Whether the intro is still playing at `t`.
pub fn playing(t: f32) -> bool {
    t < TOTAL
}

/// The 4x4 Bayer threshold of a cell, in (0, 1).
pub fn bayer(ix: i32, iy: i32) -> f32 {
    const M: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
    (M[iy.rem_euclid(4) as usize][ix.rem_euclid(4) as usize] as f32 + 0.5) / 16.
}

/// The background's tone at a point (`x`, `y` in 0..1 across the window),
/// 0 to 1, before dithering: a low band of light like a horizon, slow
/// wind lines drifting through it, and a soft lift around the cursor
/// (`cursor` in the same units, `aspect` = width / height).
pub fn field(x: f32, y: f32, t: f32, cursor: Option<(f32, f32)>, aspect: f32) -> f32 {
    let horizon = 0.42 * (-((y - 0.64) / 0.16).powi(2)).exp();
    let sky = 0.10 * (1. - y);
    let wind = 0.5 + 0.5 * (x * 9. - y * 3. + t * 0.35 + (y * 7. + t * 0.2).sin() * 0.9).sin();
    let lines = 0.16 * wind * (-((y - 0.58) / 0.28).powi(2)).exp();
    let near = cursor.map_or(0., |(cx, cy)| {
        let d2 = ((x - cx) * aspect).powi(2) + (y - cy).powi(2);
        0.30 * (-d2 / 0.018).exp()
    });
    (horizon + sky + lines + near).clamp(0., 1.)
}

/// How much of a cell the reveal has reached: a ring that grows from the
/// middle, with a soft dithered edge.
pub fn revealed(x: f32, y: f32, reveal: f32, aspect: f32) -> f32 {
    let d = (((x - 0.5) * aspect).powi(2) + (y - 0.5).powi(2)).sqrt();
    let far = (0.25 * aspect * aspect + 0.25).sqrt();
    clamp01((reveal * (far + 0.15) - d) / 0.15)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bird_flies_in_settles_turns_and_steps_aside() {
        let start = pose(FLY.0 + 0.01);
        assert!(start.bird.0 < -2.5 && start.bird.1 < -1.5, "{start:?}");
        assert!(start.scale < 0.6);
        assert!(start.frame.unwrap() < BEAT_FRAMES);
        // Real wingbeats: the frame moves on while it flies.
        assert_ne!(pose(1.0).frame, pose(1.1).frame);
        let settled = pose(SETTLE.0 + SETTLE.1 + 0.05);
        assert!(settled.bird.0.abs() < 1e-3 && settled.bird.1.abs() < 1e-3);
        assert_eq!(settled.frame, Some(0), "rests on the still mark");
        assert!(settled.metal.is_none());
        let white = pose(METAL.0 + METAL.1);
        assert_eq!(white.metal, Some((METAL_FRAMES - 1, 1.)));
        assert_eq!(white.frame, None);
        assert_eq!(pose(SLIDE.0 + SLIDE.1).slide, 1.);
        assert_eq!(pose(WORDS.0 + WORDS.1).wipe, 1.);
        assert!(pose(TOTAL).alpha < 1e-3);
        assert!(playing(TOTAL - 0.1) && !playing(TOTAL));
    }

    #[test]
    fn the_field_reveals_from_the_middle_and_follows_the_cursor() {
        assert_eq!(revealed(0.5, 0.5, 0., 1.6), 0.);
        assert!(revealed(0.5, 0.5, 0.3, 1.6) > 0.9);
        assert_eq!(revealed(0.0, 0.0, 0.3, 1.6), 0.);
        assert!(revealed(0.0, 0.0, 1.0, 1.6) > 0.99);
        let calm = field(0.2, 0.3, 1., None, 1.6);
        let lit = field(0.2, 0.3, 1., Some((0.2, 0.3)), 1.6);
        assert!(lit > calm + 0.2);
        let mut seen = std::collections::HashSet::new();
        for y in 0..4 {
            for x in 0..4 {
                seen.insert((bayer(x, y) * 16.) as u32);
            }
        }
        assert_eq!(seen.len(), 16);
    }
}
