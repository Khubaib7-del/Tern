//! The speakers, through cpal (WASAPI, CoreAudio, ALSA), for read aloud
//! (#69). A [`Player`] owns an output stream on its own thread (a stream
//! may not move between threads on every platform) and plays mono
//! samples pushed to it, at any rate: they are resampled to the device's
//! rate and copied to every channel. Pause keeps the queue; stop drops
//! it. Nothing is recorded or kept.

use std::collections::VecDeque;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

#[derive(Debug, Default)]
struct Shared {
    queue: VecDeque<f32>,
    paused: bool,
    /// Frames played so far (device rate).
    played: u64,
    /// Frames pushed so far (device rate).
    pushed: u64,
    error: Option<String>,
}

/// Audio playing on the default output device.
#[derive(Debug)]
pub struct Player {
    shared: Arc<Mutex<Shared>>,
    stop: mpsc::Sender<()>,
    rate: u32,
}

impl Player {
    /// Opens the default output device. Fails with a plain reason when
    /// there is none or it cannot be opened.
    pub fn start() -> Result<Self, String> {
        let shared: Arc<Mutex<Shared>> = Arc::default();
        let (stop, stopped) = mpsc::channel::<()>();
        let (ready, is_ready) = mpsc::channel::<Result<u32, String>>();
        let s = shared.clone();
        std::thread::Builder::new()
            .name("tern-speaker".into())
            .spawn(move || play(stopped, ready, s))
            .map_err(|e| format!("Could not start playback: {e}"))?;
        match is_ready.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(rate)) => Ok(Self { shared, stop, rate }),
            Ok(Err(e)) => Err(e),
            Err(_) => Err("The speakers did not start.".into()),
        }
    }

    /// The device's rate (frames per second).
    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// Queues mono `samples` at `rate`; returns the frame (device rate)
    /// at which they start playing.
    pub fn push(&self, samples: &[f32], rate: u32) -> u64 {
        let resampled = crate::audio::resample(samples, rate, self.rate);
        let Ok(mut s) = self.shared.lock() else {
            return 0;
        };
        let at = s.pushed;
        s.pushed += resampled.len() as u64;
        s.queue.extend(resampled);
        at
    }

    /// Pauses (true) or resumes (false); the queue is kept.
    pub fn set_paused(&self, paused: bool) {
        if let Ok(mut s) = self.shared.lock() {
            s.paused = paused;
        }
    }

    pub fn is_paused(&self) -> bool {
        self.shared.lock().is_ok_and(|s| s.paused)
    }

    /// Frames played so far (device rate).
    pub fn played(&self) -> u64 {
        self.shared.lock().map_or(0, |s| s.played)
    }

    /// Whether everything pushed has played.
    pub fn drained(&self) -> bool {
        self.shared
            .lock()
            .map(|s| s.queue.is_empty())
            .unwrap_or(true)
    }

    /// The stream's error, if it failed while playing.
    pub fn error(&self) -> Option<String> {
        self.shared.lock().ok().and_then(|s| s.error.clone())
    }

    /// Stops at once and drops what is queued.
    pub fn stop(&self) {
        if let Ok(mut s) = self.shared.lock() {
            s.queue.clear();
        }
        let _ = self.stop.send(());
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        let _ = self.stop.send(());
    }
}

/// The longest a player thread lives (an hour of reading).
const MAX_PLAY: Duration = Duration::from_secs(3600);

fn play(
    stopped: mpsc::Receiver<()>,
    ready: mpsc::Sender<Result<u32, String>>,
    shared: Arc<Mutex<Shared>>,
) {
    let fail = |e: String| {
        let _ = ready.send(Err(e));
    };
    let host = cpal::default_host();
    let Some(device) = host.default_output_device() else {
        return fail("No speakers or headphones were found.".into());
    };
    let config = match device.default_output_config() {
        Ok(c) => c,
        Err(e) => return fail(format!("The speakers could not be opened: {e}")),
    };
    let channels = (config.channels() as usize).max(1);
    let rate: u32 = config.sample_rate();
    let err_slot = shared.clone();
    let on_error = move |e: cpal::Error| {
        if let Ok(mut s) = err_slot.lock() {
            s.error.get_or_insert_with(|| e.to_string());
        }
    };
    macro_rules! stream {
        ($t:ty) => {{
            let shared = shared.clone();
            device.build_output_stream(
                config.clone().into(),
                move |data: &mut [$t], _: &cpal::OutputCallbackInfo| {
                    let mut s = match shared.lock() {
                        Ok(s) => s,
                        Err(_) => {
                            data.fill(cpal::Sample::from_sample::<f32>(0.0));
                            return;
                        }
                    };
                    for frame in data.chunks_mut(channels) {
                        let v = if s.paused {
                            0.0
                        } else {
                            match s.queue.pop_front() {
                                Some(v) => {
                                    s.played += 1;
                                    v.clamp(-1.0, 1.0)
                                }
                                None => 0.0,
                            }
                        };
                        for out in frame.iter_mut() {
                            *out = cpal::Sample::from_sample::<f32>(v);
                        }
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
        other => {
            return fail(format!(
                "The speakers take {other} samples, which Tern does not write."
            ));
        }
    };
    let stream = match stream {
        Ok(s) => s,
        Err(e) => return fail(format!("The speakers could not be opened: {e}")),
    };
    if let Err(e) = stream.play() {
        return fail(format!("The speakers could not start: {e}"));
    }
    let _ = ready.send(Ok(rate));
    let _ = stopped.recv_timeout(MAX_PLAY);
    drop(stream);
}
