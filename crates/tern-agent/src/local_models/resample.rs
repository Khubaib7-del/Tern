//! A small band-limited resampler (windowed sinc) for speech: good enough
//! for 8–48 kHz in, 16 kHz out, with no extra dependency.

/// The sample rate the local speech models take.
pub const RATE: u32 = 16_000;

/// Resamples mono `x` from `from` Hz to `to` Hz. Going down, the sinc is
/// widened so nothing above the new Nyquist frequency folds back.
pub fn resample(x: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || x.is_empty() || from == 0 || to == 0 {
        return x.to_vec();
    }
    let ratio = f64::from(to) / f64::from(from);
    let n_out = (x.len() as f64 * ratio).floor() as usize;
    let cutoff = ratio.min(1.0) * 0.95;
    let half = (12.0 / cutoff).ceil() as isize;
    let mut out = Vec::with_capacity(n_out);
    for i in 0..n_out {
        let t = i as f64 / ratio;
        let c = t.floor() as isize;
        let (mut acc, mut wsum) = (0.0f64, 0.0f64);
        for k in (c - half + 1).max(0)..=(c + half).min(x.len() as isize - 1) {
            let d = t - k as f64;
            if d.abs() >= half as f64 {
                continue;
            }
            let window = 0.5 + 0.5 * (std::f64::consts::PI * d / half as f64).cos();
            let a = std::f64::consts::PI * cutoff * d;
            let sinc = if a.abs() < 1e-9 { 1.0 } else { a.sin() / a };
            let w = sinc * window;
            acc += f64::from(x[k as usize]) * w;
            wsum += w;
        }
        out.push(if wsum.abs() > 1e-9 {
            (acc / wsum) as f32
        } else {
            0.0
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(hz: f32, rate: u32, secs: f32) -> Vec<f32> {
        (0..(rate as f32 * secs) as usize)
            .map(|i| (i as f32 * hz * std::f32::consts::TAU / rate as f32).sin())
            .collect()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|s| s * s).sum::<f32>() / x.len() as f32).sqrt()
    }

    #[test]
    fn speech_band_survives_and_high_tones_are_removed() {
        // 1 kHz passes from 48 kHz to 16 kHz...
        let y = resample(&tone(1000.0, 48_000, 1.0), 48_000, 16_000);
        assert_eq!(y.len(), 16_000);
        assert!((rms(&y[200..15_800]) - 0.707).abs() < 0.02);
        // ...a 12 kHz tone (above the new 8 kHz Nyquist) does not fold back.
        let y = resample(&tone(12_000.0, 48_000, 1.0), 48_000, 16_000);
        assert!(rms(&y[200..15_800]) < 0.05, "{}", rms(&y));
        // Up from 8 kHz keeps the length ratio and the level.
        let y = resample(&tone(500.0, 8_000, 0.5), 8_000, 16_000);
        assert_eq!(y.len(), 8_000);
        assert!((rms(&y[200..7_800]) - 0.707).abs() < 0.02);
        assert_eq!(resample(&[0.1, 0.2], 16_000, 16_000), vec![0.1, 0.2]);
    }
}
