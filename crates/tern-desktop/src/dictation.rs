//! Streaming voice input: speech is cut into windows and each window is
//! transcribed while the user is still talking, the way Claude's web mic
//! works. The open window is sent again as it grows (its text is shown as
//! interim, grey, with a trailing "…"); when the speaker pauses, or the
//! window gets long, the window is closed and its last transcription is
//! committed. Stopping the mic sends the open window one last time and
//! commits it, replacing the interim text.
//!
//! No I/O here: the window asks for [`Request`]s, sends each window's
//! samples to the speech-to-text model, and hands the text back with
//! [`Dictation::on_result`]. Samples are 16 kHz mono.

/// Samples per second (16 kHz mono speech).
pub const RATE: usize = 16_000;
/// The open window is first sent once it holds this much speech.
pub const MIN_WINDOW: usize = RATE * 2;
/// A window is closed by force at this length, even without a pause.
pub const MAX_WINDOW: usize = RATE * 12;
/// An interim resend waits for at least this much new audio. Three
/// seconds keeps a minute of talking near 20 requests, inside the free
/// speech-to-text limits (Groq allows 20 a minute).
pub const RESEND_STEP: usize = RATE * 3;
/// Quiet this long ends a phrase and closes the window.
pub const PAUSE: usize = RATE / 2;
/// The last bit of audio after stop is sent if it is at least this long.
pub const MIN_FINAL: usize = RATE / 4;
/// A window that failed this many times is dropped, not retried forever.
pub const MAX_TRIES: u8 = 2;

/// A window to transcribe: samples `start..end`. `last` windows are
/// committed when their text comes back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Request {
    pub seq: u64,
    pub start: usize,
    pub end: usize,
    pub last: bool,
}

/// Streaming dictation state for one recording.
#[derive(Debug, Default)]
pub struct Dictation {
    /// Text from closed windows, not yet handed to the composer.
    committed: Vec<String>,
    /// The open window's latest text.
    interim: String,
    /// Where the open window starts.
    start: usize,
    /// The end of the last interim request (to pace resends).
    sent_to: usize,
    in_flight: Option<Request>,
    seq: u64,
    tries: u8,
    stopped: bool,
    /// The last failure, for a toast.
    pub error: Option<String>,
}

impl Dictation {
    pub fn new() -> Self {
        Self::default()
    }

    /// The mic was stopped: the open window gets one last pass.
    pub fn stop(&mut self) {
        self.stopped = true;
    }

    /// The open window's text as the composer shows it, grey: the latest
    /// interim text with a trailing ellipsis, or nothing.
    pub fn interim_line(&self) -> Option<String> {
        let t = self.interim.trim();
        (!t.is_empty()).then(|| format!("{t}…"))
    }

    /// Committed text not yet put in the composer, joined; empties the
    /// queue.
    pub fn take_committed(&mut self) -> Option<String> {
        if self.committed.is_empty() {
            return None;
        }
        let t = self
            .committed
            .drain(..)
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        (!t.is_empty()).then_some(t)
    }

    /// Stopped, nothing in flight, and nothing left to send: the
    /// recording is finished.
    pub fn done(&self, len: usize) -> bool {
        self.stopped && self.in_flight.is_none() && len.saturating_sub(self.start) < MIN_FINAL
    }

    /// The next window to send, given every sample so far, or `None` when
    /// it is not time yet (or one is already in flight).
    pub fn next_request(&mut self, samples: &[f32]) -> Option<Request> {
        if self.in_flight.is_some() {
            return None;
        }
        let len = samples.len();
        let open = len.saturating_sub(self.start);
        let req = if self.stopped {
            (open >= MIN_FINAL).then_some((len, true))
        } else if open < MIN_WINDOW {
            None
        } else if open >= MAX_WINDOW {
            // Cut at the quietest spot in the last two seconds, so a word
            // is less likely to be split.
            let from = len.saturating_sub(RATE * 2).max(self.start + MIN_WINDOW);
            Some((quietest(samples, from, len).unwrap_or(len), true))
        } else if paused(&samples[self.start..]) {
            Some((len - PAUSE / 2, true))
        } else if self.sent_to <= self.start || len >= self.sent_to + RESEND_STEP {
            // The window's first pass, or enough new speech since the last.
            Some((len, false))
        } else {
            None
        };
        let (end, last) = req?;
        self.seq += 1;
        if !last {
            self.sent_to = end;
        }
        let r = Request {
            seq: self.seq,
            start: self.start,
            end,
            last,
        };
        self.in_flight = Some(r);
        Some(r)
    }

    /// A window's text came back (or why not). Stale answers are ignored.
    pub fn on_result(&mut self, seq: u64, result: Result<String, String>) {
        let Some(req) = self.in_flight.filter(|r| r.seq == seq) else {
            return;
        };
        self.in_flight = None;
        match result {
            Ok(text) => {
                self.tries = 0;
                if req.last {
                    // The window's own text wins over its interim guess.
                    let text = text.trim();
                    if !text.is_empty() {
                        self.committed.push(text.to_owned());
                    }
                    self.interim.clear();
                    self.start = req.end;
                    self.sent_to = req.end;
                } else {
                    self.interim = text.trim().to_owned();
                }
            }
            Err(e) => {
                self.error = Some(e);
                self.tries += 1;
                if req.last && self.tries >= MAX_TRIES {
                    // Keep what was heard so far rather than lose it.
                    if !self.interim.trim().is_empty() {
                        self.committed.push(std::mem::take(&mut self.interim));
                    }
                    self.start = req.end;
                    self.sent_to = req.end;
                    self.tries = 0;
                } else if !req.last && self.tries >= MAX_TRIES {
                    // Interim passes stop until the window closes.
                    self.sent_to = usize::MAX / 2;
                }
            }
        }
    }
}

/// Root mean square of a slice.
fn rms(s: &[f32]) -> f32 {
    if s.is_empty() {
        return 0.0;
    }
    (s.iter().map(|x| x * x).sum::<f32>() / s.len() as f32).sqrt()
}

/// Whether the window ends in a pause: its last [`PAUSE`] is much quieter
/// than the speech before it (and the window holds speech at all).
pub fn paused(window: &[f32]) -> bool {
    if window.len() < MIN_WINDOW {
        return false;
    }
    let (speech, tail) = window.split_at(window.len() - PAUSE);
    let loud = speech.chunks(RATE / 10).map(rms).fold(0.0_f32, f32::max);
    let quiet = tail.chunks(RATE / 10).map(rms).fold(0.0_f32, f32::max);
    loud > 0.01 && quiet < loud * 0.15
}

/// The start of the quietest 100 ms between `from` and `to`.
fn quietest(samples: &[f32], from: usize, to: usize) -> Option<usize> {
    let step = RATE / 10;
    let to = to.min(samples.len());
    (from..to.saturating_sub(step))
        .step_by(step)
        .map(|i| (i, rms(&samples[i..i + step])))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i + step / 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(n: usize) -> Vec<f32> {
        (0..n).map(|i| ((i as f32) * 0.05).sin() * 0.3).collect()
    }

    #[test]
    fn nothing_is_sent_before_two_seconds() {
        let mut d = Dictation::new();
        assert_eq!(d.next_request(&tone(RATE)), None);
        let r = d.next_request(&tone(MIN_WINDOW)).unwrap();
        assert!(!r.last);
        assert_eq!((r.start, r.end), (0, MIN_WINDOW));
        // One at a time.
        assert_eq!(d.next_request(&tone(MIN_WINDOW * 2)), None);
    }

    #[test]
    fn interim_text_shows_grey_then_a_pause_commits_it() {
        let mut d = Dictation::new();
        let r = d.next_request(&tone(MIN_WINDOW)).unwrap();
        d.on_result(r.seq, Ok("hello wor".into()));
        assert_eq!(d.interim_line().as_deref(), Some("hello wor…"));
        assert_eq!(d.take_committed(), None);
        let mut s = tone(RATE * 2);
        s.extend(std::iter::repeat_n(0.0, PAUSE));
        let r = d.next_request(&s).unwrap();
        assert!(r.last, "a pause closes the window");
        d.on_result(r.seq, Ok("hello world".into()));
        assert_eq!(d.interim_line(), None);
        assert_eq!(d.take_committed().as_deref(), Some("hello world"));
        // The next window starts where that one ended.
        let more = [s.clone(), tone(MIN_WINDOW)].concat();
        let r = d.next_request(&more).unwrap();
        assert_eq!(r.start, s.len() - PAUSE / 2);
    }

    #[test]
    fn stop_sends_the_rest_once_and_finishes() {
        let mut d = Dictation::new();
        let s = tone(RATE);
        d.stop();
        let r = d.next_request(&s).unwrap();
        assert!(r.last && r.end == s.len());
        assert!(!d.done(s.len()));
        d.on_result(r.seq, Ok("final words".into()));
        assert!(d.done(s.len()));
        assert_eq!(d.take_committed().as_deref(), Some("final words"));
    }

    #[test]
    fn a_long_window_is_cut_without_a_pause() {
        let mut d = Dictation::new();
        let s = tone(MAX_WINDOW + RATE);
        let r = d.next_request(&s).unwrap();
        assert!(r.last);
        assert!(r.end > MIN_WINDOW && r.end <= s.len());
    }

    #[test]
    fn stale_and_failed_results_do_not_lose_text() {
        let mut d = Dictation::new();
        let r = d.next_request(&tone(MIN_WINDOW)).unwrap();
        d.on_result(r.seq + 7, Ok("ignored".into()));
        assert!(
            d.in_flight.is_some(),
            "a stale answer leaves the window waiting"
        );
        d.on_result(r.seq, Ok("partial".into()));
        d.stop();
        let s = tone(RATE * 2);
        for _ in 0..MAX_TRIES {
            let r = d.next_request(&s).unwrap();
            d.on_result(r.seq, Err("network".into()));
        }
        assert_eq!(d.take_committed().as_deref(), Some("partial"));
        assert!(d.done(s.len()));
        assert_eq!(d.error.as_deref(), Some("network"));
    }
}
