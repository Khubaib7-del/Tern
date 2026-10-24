//! Speech for the mic: a recording made into the 16 kHz mono 16-bit WAV
//! that speech-to-text endpoints take. The conversion is plain code, tested
//! without a microphone; the capture itself is in [`crate::mic`] behind the
//! `mic` feature.

/// The rate transcription models expect.
pub const SPEECH_RATE: u32 = 16_000;

/// Averages interleaved `channels` into one.
pub fn downmix(samples: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return samples.to_vec();
    }
    samples
        .chunks(channels)
        .map(|frame| frame.iter().sum::<f32>() / frame.len() as f32)
        .collect()
}

/// Resamples mono audio from `from` Hz to `to` Hz by linear interpolation:
/// plenty for speech recognition.
pub fn resample(samples: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || samples.is_empty() || from == 0 {
        return samples.to_vec();
    }
    let ratio = from as f64 / to as f64;
    let n = ((samples.len() as f64) / ratio).floor() as usize;
    (0..n)
        .map(|i| {
            let pos = i as f64 * ratio;
            let j = pos.floor() as usize;
            let frac = (pos - j as f64) as f32;
            let a = samples[j.min(samples.len() - 1)];
            let b = samples[(j + 1).min(samples.len() - 1)];
            a + (b - a) * frac
        })
        .collect()
}

/// A 16-bit PCM WAV file of mono `samples` at `rate`.
pub fn wav(samples: &[f32], rate: u32) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 2).to_le_bytes()); // byte rate
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        let v = (s.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

/// Interleaved samples at `rate` with `channels`, made into speech WAV.
pub fn speech_wav(samples: &[f32], channels: usize, rate: u32) -> Vec<u8> {
    wav(
        &resample(&downmix(samples, channels), rate, SPEECH_RATE),
        SPEECH_RATE,
    )
}

/// Seconds of audio in a speech WAV made by [`speech_wav`].
pub fn wav_seconds(wav: &[u8]) -> f32 {
    wav.len().saturating_sub(44) as f32 / (SPEECH_RATE as f32 * 2.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stereo_48k_becomes_16k_mono_wav() {
        // One second of a 440 Hz tone, stereo, 48 kHz.
        let frames = 48_000;
        let mut s = Vec::with_capacity(frames * 2);
        for i in 0..frames {
            let v = (i as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin() * 0.5;
            s.push(v);
            s.push(v);
        }
        let w = speech_wav(&s, 2, 48_000);
        assert_eq!(&w[..4], b"RIFF");
        assert_eq!(&w[8..16], b"WAVEfmt ");
        assert_eq!(u16::from_le_bytes([w[22], w[23]]), 1, "mono");
        assert_eq!(u32::from_le_bytes([w[24], w[25], w[26], w[27]]), 16_000);
        assert_eq!(u16::from_le_bytes([w[34], w[35]]), 16, "16-bit");
        assert_eq!(w.len(), 44 + 16_000 * 2);
        assert!((wav_seconds(&w) - 1.0).abs() < 0.01);
    }

    #[test]
    fn clipping_and_empty_input() {
        let w = wav(&[2.0, -2.0], 16_000);
        assert_eq!(i16::from_le_bytes([w[44], w[45]]), i16::MAX);
        assert_eq!(i16::from_le_bytes([w[46], w[47]]), -i16::MAX);
        assert_eq!(speech_wav(&[], 1, 44_100).len(), 44);
        assert_eq!(downmix(&[1.0, 0.0, 0.5, 0.5], 2), [0.5, 0.5]);
    }
}
