//! Audio for the local speech models: decoding a file (WAV, MP3, M4A/AAC,
//! Ogg Vorbis, FLAC) to mono samples with symphonia, and resampling to
//! the 16 kHz the speech models take. Built with the `audio` feature.

use std::io::Cursor;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{CODEC_TYPE_NULL, DecoderOptions};
use symphonia::core::errors::Error;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

pub use super::resample::{RATE, resample};

/// Decodes an audio file to 16 kHz mono samples in -1..1. `name` (the
/// file name) helps guess the format.
pub fn decode_16k_mono(bytes: &[u8], name: &str) -> Result<Vec<f32>, String> {
    let (mono, rate) = decode_mono(bytes, name)?;
    Ok(resample(&mono, rate, RATE))
}

/// How long an audio file plays, in seconds, from its headers only (no
/// decoding). `None` when the container does not say (an MP3 with no
/// frame count) or the format is not one symphonia reads.
pub fn duration_secs(path: &std::path::Path) -> Option<f64> {
    let file = std::fs::File::open(path).ok()?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(&ext.to_ascii_lowercase());
    }
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .ok()?;
    let track = probed
        .format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)?;
    let p = &track.codec_params;
    let frames = p.n_frames?;
    match (p.time_base, p.sample_rate) {
        (Some(tb), _) => {
            let t = tb.calc_time(frames);
            Some(t.seconds as f64 + t.frac)
        }
        (None, Some(rate)) if rate > 0 => Some(frames as f64 / rate as f64),
        _ => None,
    }
}

/// Decodes the first audio track to mono at its own sample rate.
pub fn decode_mono(bytes: &[u8], name: &str) -> Result<(Vec<f32>, u32), String> {
    let mss = MediaSourceStream::new(Box::new(Cursor::new(bytes.to_vec())), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = std::path::Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
    {
        hint.with_extension(&ext.to_ascii_lowercase());
    }
    let unsupported = |e: Error| {
        format!(
            "{name} is not an audio format Tern can decode here ({e}). WAV, MP3, M4A/AAC, Ogg Vorbis, and FLAC work; Opus voice notes (.opus, most .ogg from WhatsApp) do not yet."
        )
    };
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(unsupported)?;
    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or_else(|| format!("{name} has no audio track"))?;
    let track_id = track.id;
    let mut rate = track.codec_params.sample_rate.unwrap_or(0);
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(unsupported)?;
    let mut mono = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(Error::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(Error::ResetRequired) => break,
            Err(e) => return Err(format!("{name} could not be read: {e}")),
        };
        if packet.track_id() != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(buf) => {
                let spec = *buf.spec();
                let ch = spec.channels.count().max(1);
                rate = spec.rate;
                let mut sb = SampleBuffer::<f32>::new(buf.capacity() as u64, spec);
                sb.copy_interleaved_ref(buf);
                mono.extend(
                    sb.samples()
                        .chunks(ch)
                        .map(|f| f.iter().sum::<f32>() / ch as f32),
                );
            }
            // A damaged packet is skipped, as players do.
            Err(Error::DecodeError(_)) => continue,
            Err(e) => return Err(format!("{name} could not be decoded: {e}")),
        }
    }
    if rate == 0 {
        return Err(format!("{name} does not say its sample rate"));
    }
    if mono.is_empty() {
        return Err(format!("{name} holds no sound"));
    }
    Ok((mono, rate))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 16-bit PCM WAV file.
    pub(crate) fn wav(samples: &[f32], rate: u32, channels: u16) -> Vec<u8> {
        let mut out = Vec::new();
        let data_len = (samples.len() * 2 * channels as usize) as u32;
        out.extend(b"RIFF");
        out.extend((36 + data_len).to_le_bytes());
        out.extend(b"WAVEfmt ");
        out.extend(16u32.to_le_bytes());
        out.extend(1u16.to_le_bytes());
        out.extend(channels.to_le_bytes());
        out.extend(rate.to_le_bytes());
        out.extend((rate * 2 * u32::from(channels)).to_le_bytes());
        out.extend((2 * channels).to_le_bytes());
        out.extend(16u16.to_le_bytes());
        out.extend(b"data");
        out.extend(data_len.to_le_bytes());
        for s in samples {
            let v = (s * 32767.0) as i16;
            for _ in 0..channels {
                out.extend(v.to_le_bytes());
            }
        }
        out
    }

    #[test]
    fn a_wav_file_decodes_to_16k_mono() {
        // One second of a 440 Hz tone, stereo, 44.1 kHz.
        let rate = 44_100u32;
        let samples: Vec<f32> = (0..rate)
            .map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin() * 0.5)
            .collect();
        let out = decode_16k_mono(&wav(&samples, rate, 2), "tone.WAV").unwrap();
        assert!((out.len() as i64 - 16_000).abs() <= 2, "{}", out.len());
        let peak = out.iter().fold(0f32, |m, s| m.max(s.abs()));
        assert!((0.45..0.55).contains(&peak), "{peak}");
    }

    #[test]
    fn duration_comes_from_the_headers() {
        let rate = 16_000u32;
        let samples = vec![0.1f32; (rate * 5 / 2) as usize];
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("memo.wav");
        std::fs::write(&p, wav(&samples, rate, 1)).unwrap();
        let d = duration_secs(&p).unwrap();
        assert!((d - 2.5).abs() < 0.01, "{d}");
        std::fs::write(dir.path().join("junk.mp3"), b"nope").unwrap();
        assert_eq!(duration_secs(&dir.path().join("junk.mp3")), None);
    }

    #[test]
    fn junk_is_refused_with_the_formats_that_work() {
        let e = decode_16k_mono(b"not audio at all", "memo.opus").unwrap_err();
        assert!(e.contains("WAV, MP3"), "{e}");
    }
}
