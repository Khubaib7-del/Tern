//! The microphone, through cpal (WASAPI on Windows, CoreAudio on macOS,
//! ALSA on Linux). A [`Recording`] runs on its own thread, because an
//! audio stream may not move between threads on every platform; stopping
//! it returns a 16 kHz mono WAV ([`crate::audio::speech_wav`]).
//!
//! Nothing is recorded until the user clicks the mic, and nothing is kept:
//! the WAV goes to the speech-to-text model the user configured and is
//! dropped.

use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

/// The longest recording: speech-to-text endpoints cap uploads (Groq
/// 100 MB, OpenAI 25 MB); ten minutes of 16 kHz mono WAV is about 19 MB.
pub const MAX_SECONDS: u64 = 600;

/// A recording in progress.
#[derive(Debug)]
pub struct Recording {
    stop: mpsc::Sender<()>,
    done: JoinHandle<Result<Vec<u8>, String>>,
}

impl Recording {
    /// Opens the default input device and starts recording. Fails with a
    /// plain reason when there is no microphone or it cannot be opened.
    pub fn start() -> Result<Self, String> {
        let (stop, stopped) = mpsc::channel::<()>();
        let (ready, is_ready) = mpsc::channel::<Result<(), String>>();
        let done = std::thread::Builder::new()
            .name("tern-mic".into())
            .spawn(move || record(stopped, ready))
            .map_err(|e| format!("Could not start recording: {e}"))?;
        match is_ready.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => Ok(Self { stop, done }),
            Ok(Err(e)) => {
                let _ = done.join();
                Err(e)
            }
            Err(_) => Err("The microphone did not start.".into()),
        }
    }

    /// Stops and returns the speech as a WAV file.
    pub fn stop(self) -> Result<Vec<u8>, String> {
        let _ = self.stop.send(());
        self.done
            .join()
            .map_err(|_| "The recording thread failed.".to_owned())?
    }

    /// Stops and throws the audio away (Esc).
    pub fn cancel(self) {
        let _ = self.stop.send(());
        let _ = self.done.join();
    }
}

fn record(
    stopped: mpsc::Receiver<()>,
    ready: mpsc::Sender<Result<(), String>>,
) -> Result<Vec<u8>, String> {
    let fail = |e: String| {
        let _ = ready.send(Err(e.clone()));
        Err(e)
    };
    let host = cpal::default_host();
    let Some(device) = host.default_input_device() else {
        return fail("No microphone was found.".into());
    };
    let config = match device.default_input_config() {
        Ok(c) => c,
        Err(e) => return fail(format!("The microphone could not be opened: {e}")),
    };
    let channels = config.channels() as usize;
    let rate: u32 = config.sample_rate();
    let buf: Arc<Mutex<Vec<f32>>> = Arc::default();
    let errors: Arc<Mutex<Option<String>>> = Arc::default();
    let err_slot = errors.clone();
    let on_error = move |e: cpal::Error| {
        if let Ok(mut slot) = err_slot.lock() {
            slot.get_or_insert_with(|| e.to_string());
        }
    };
    let limit = (MAX_SECONDS as usize) * rate as usize * channels;
    macro_rules! stream {
        ($t:ty) => {{
            let buf = buf.clone();
            device.build_input_stream(
                config.clone().into(),
                move |data: &[$t], _: &cpal::InputCallbackInfo| {
                    if let Ok(mut b) = buf.lock()
                        && b.len() < limit
                    {
                        b.extend(data.iter().map(|s| cpal::Sample::to_sample::<f32>(*s)));
                    }
                },
                on_error.clone(),
                None,
            )
        }};
    }
    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => stream!(f32),
        cpal::SampleFormat::I16 => stream!(i16),
        cpal::SampleFormat::U16 => stream!(u16),
        cpal::SampleFormat::I32 => stream!(i32),
        cpal::SampleFormat::I8 => stream!(i8),
        other => {
            return fail(format!(
                "The microphone sends {other} samples, which Tern does not read."
            ));
        }
    };
    let stream = match stream {
        Ok(s) => s,
        Err(e) => return fail(format!("The microphone could not be opened: {e}")),
    };
    if let Err(e) = stream.play() {
        return fail(format!("The microphone could not start: {e}"));
    }
    let _ = ready.send(Ok(()));
    let _ = stopped.recv_timeout(Duration::from_secs(MAX_SECONDS));
    drop(stream);
    if let Some(e) = errors.lock().ok().and_then(|mut e| e.take()) {
        let samples = buf.lock().map(|b| b.len()).unwrap_or(0);
        if samples == 0 {
            return Err(format!("The microphone stopped: {e}"));
        }
    }
    let samples = std::mem::take(&mut *buf.lock().map_err(|_| "The recording was lost.")?);
    if samples.is_empty() {
        return Err("Nothing was recorded.".into());
    }
    Ok(crate::audio::speech_wav(&samples, channels, rate))
}
