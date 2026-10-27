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
//!
//! Two paces (#68): [`Pace::STREAMING`] for the fast English model
//! (Moonshine), which resends the open window as it grows, and
//! [`Pace::CHUNKED`] for Whisper (Urdu and every other language): short
//! 2–5 s windows cut at pauses, each committed once, so text still
//! streams in while the user talks. Every window's text goes through
//! [`tern_agent::speech_guard::clean`] (loops and silence phrases), and a
//! window with no speech is quiet: it is counted, not reported.

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
/// A window whose loudest 100 ms is under this is quiet (no speech).
pub const QUIET_RMS: f32 = 0.01;

/// How windows are cut and sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pace {
    /// The open window is first sent at this length.
    pub min: usize,
    /// A window is closed by force at this length.
    pub max: usize,
    /// Interim resends after this much new audio; `None` sends each
    /// window once, when it closes.
    pub resend: Option<usize>,
}

impl Pace {
    /// Moonshine: interim text every 3 s, windows up to 12 s.
    pub const STREAMING: Pace = Pace {
        min: MIN_WINDOW,
        max: MAX_WINDOW,
        resend: Some(RESEND_STEP),
    };
    /// Whisper: each 2–5 s window once (Whisper small takes about as long
    /// as the audio on a laptop CPU, so resending would fall behind).
    pub const CHUNKED: Pace = Pace {
        min: MIN_WINDOW,
        max: RATE * 5,
        resend: None,
    };
}

impl Default for Pace {
    fn default() -> Self {
        Pace::STREAMING
    }
}

/// A window to transcribe: samples `start..end`. `last` windows are
/// committed when their text comes back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Request {
    pub seq: u64,
    pub start: usize,
    pub end: usize,
    pub last: bool,
    /// The window held almost no sound.
    pub quiet: bool,
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
    pace: Pace,
    /// Everything committed so far (for the repeat guard).
    said: String,
    /// Windows that came back empty, and whether any brought text: a
    /// recording with sound and no text at all gets one note at the end.
    pub empty_windows: u32,
    pub heard_any: bool,
    /// The last failure, for a toast.
    pub error: Option<String>,
}

impl Dictation {
    pub fn new() -> Self {
        Self::default()
    }

    /// Dictation cut at `pace`.
    pub fn with_pace(pace: Pace) -> Self {
        Self {
            pace,
            ..Self::default()
        }
    }

    /// Starts the recording over from its first sample at `pace`, every
    /// text so far dropped: Auto heard a language the fast model cannot
    /// write, so Whisper takes the whole recording (#70). An answer still
    /// on its way is ignored.
    pub fn restart(&mut self, pace: Pace) {
        self.committed.clear();
        self.interim.clear();
        self.said.clear();
        self.start = 0;
        self.sent_to = 0;
        self.in_flight = None;
        self.tries = 0;
        self.pace = pace;
    }

    /// Drops the open window's text and any answer on its way, and goes
    /// on at `pace` from the same place (the language changed mid
    /// recording, #70). Committed text stays.
    pub fn redo_open_window(&mut self, pace: Pace) {
        self.interim.clear();
        self.in_flight = None;
        self.sent_to = self.start;
        self.tries = 0;
        self.pace = pace;
    }

    /// Where the open window starts (sample index).
    pub fn open_start(&self) -> usize {
        self.start
    }

    /// Whether the mic was stopped.
    pub fn stopped(&self) -> bool {
        self.stopped
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
        let pace = self.pace;
        let req = if self.stopped {
            (open >= MIN_FINAL).then_some((len, true))
        } else if open < pace.min {
            None
        } else if open >= pace.max {
            // Cut at the quietest spot in the last two seconds, so a word
            // is less likely to be split.
            let from = len
                .saturating_sub(RATE * 2)
                .max(self.start + pace.min.min(pace.max / 2));
            Some((quietest(samples, from, len).unwrap_or(len), true))
        } else if paused(&samples[self.start..]) {
            Some((len - PAUSE / 2, true))
        } else if let Some(step) = pace.resend {
            // The window's first pass, or enough new speech since the last.
            (self.sent_to <= self.start || len >= self.sent_to + step).then_some((len, false))
        } else {
            None
        };
        let (end, last) = req?;
        self.seq += 1;
        if !last {
            self.sent_to = end;
        }
        let window = &samples[self.start.min(end)..end];
        let r = Request {
            seq: self.seq,
            start: self.start,
            end,
            last,
            quiet: loudest(window) < QUIET_RMS,
        };
        self.in_flight = Some(r);
        Some(r)
    }

    /// A window's text came back (or why not). Stale answers are ignored.
    /// Returns whether the window brought words (it held sound and its
    /// text survived the guard), which is when Auto may trust the
    /// language Whisper heard in it.
    pub fn on_result(&mut self, seq: u64, result: Result<String, String>) -> bool {
        let Some(req) = self.in_flight.filter(|r| r.seq == seq) else {
            return false;
        };
        self.in_flight = None;
        // "Heard no speech" is a pause, not a failure.
        let result = match result {
            Err(e) if e.contains(tern_agent::media::NO_SPEECH) => Ok(String::new()),
            r => r,
        };
        let mut spoke = false;
        match result {
            Ok(text) => {
                self.tries = 0;
                let text = tern_agent::speech_guard::clean(&text, &self.said, req.quiet);
                spoke = !req.quiet && !text.trim().is_empty();
                if req.last {
                    // The window's own text wins over its interim guess.
                    let text = text.trim();
                    if text.is_empty() {
                        self.empty_windows += 1;
                    } else {
                        self.heard_any = true;
                        self.committed.push(text.to_owned());
                        self.said.push(' ');
                        self.said.push_str(text);
                        // The guard only looks at the last few sentences.
                        if self.said.len() > 2_000 {
                            let cut = self.said.len() - 1_000;
                            let cut = (cut..self.said.len())
                                .find(|i| self.said.is_char_boundary(*i))
                                .unwrap_or(0);
                            self.said = self.said[cut..].to_owned();
                        }
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
        spoke
    }
}

/// Auto's language check waits for this much audio after a phrase
/// starts (#70)...
pub const CHECK_MIN: usize = RATE * 3;
/// ...and listens to at most this much.
pub const CHECK_MAX: usize = RATE * 4;

/// The stretch of `samples` from `from` Auto's language check should
/// listen to: [`CHECK_MIN`]–[`CHECK_MAX`] of audio with speech in it, or
/// `None` while there is not enough yet (or it is quiet). `stopped`
/// takes whatever there is, from a second.
pub fn language_window(samples: &[f32], from: usize, stopped: bool) -> Option<(usize, usize)> {
    let len = samples.len();
    let need = if stopped { RATE } else { CHECK_MIN };
    if len < from.saturating_add(need) {
        return None;
    }
    let end = (from + CHECK_MAX).min(len);
    (loudest(&samples[from..end]) >= QUIET_RMS * 2.0).then_some((from, end))
}

/// Root mean square of a slice.
fn rms(s: &[f32]) -> f32 {
    if s.is_empty() {
        return 0.0;
    }
    (s.iter().map(|x| x * x).sum::<f32>() / s.len() as f32).sqrt()
}

/// The loudest 100 ms of a window (RMS).
pub fn loudest(window: &[f32]) -> f32 {
    window.chunks(RATE / 10).map(rms).fold(0.0_f32, f32::max)
}

/// The note shown once when a whole recording with sound brought no
/// text (#68: "heard no speech" toasts kept popping for each pause).
pub fn nothing_heard_note(d: &Dictation, samples: &[f32]) -> Option<String> {
    (!d.heard_any && d.empty_windows > 0 && samples.len() >= RATE * 2 && loudest(samples) >= QUIET_RMS)
        .then(|| {
            "Tern heard no words in that recording. Check the microphone, or pick your language in Settings › Speech › Dictation language.".to_owned()
        })
}

/// Where dictated text goes in the composer (#68): at the caret, and the
/// caret follows each phrase, so the next one goes after it. A caret at
/// the very start of text that was already there is the text field's
/// default after its value was set, not a place the user chose: the first
/// phrase then goes at the end.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Caret {
    /// Where the caret was left (byte offset) after the last insert, or
    /// at the start of the recording.
    left_at: Option<usize>,
    inserted: bool,
}

impl Caret {
    /// A recording started with the caret at `cursor`.
    pub fn start(cursor: usize) -> Self {
        Self {
            left_at: Some(cursor),
            inserted: false,
        }
    }

    /// Puts `phrase` into `text` (caret at `cursor`) and returns the new
    /// text and where the caret goes (the end of the phrase). Each phrase
    /// goes at the caret: where the user clicked before or during the
    /// recording, else right after the last phrase. Only a first phrase
    /// with the caret still at the start of existing text goes at the end.
    pub fn place(&mut self, text: &str, cursor: usize, phrase: &str) -> (String, usize) {
        let cursor = cursor.min(text.len());
        let moved = self.left_at != Some(cursor);
        let at = if !self.inserted && !moved && cursor == 0 {
            text.len()
        } else {
            cursor
        };
        let at = (0..=at)
            .rev()
            .find(|i| text.is_char_boundary(*i))
            .unwrap_or(0);
        let (head, tail) = text.split_at(at);
        let mut piece = String::new();
        if head.chars().last().is_some_and(|c| !c.is_whitespace()) {
            piece.push(' ');
        }
        piece.push_str(phrase.trim());
        let end = head.len() + piece.len();
        if tail.chars().next().is_some_and(|c| !c.is_whitespace()) {
            piece.push(' ');
        }
        let out = format!("{head}{piece}{tail}");
        self.left_at = Some(end);
        self.inserted = true;
        (out, end)
    }
}

/// Which model takes the next window (#68).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    /// The fast English model (Moonshine), streaming.
    Fast,
    /// Whisper (on this computer or a provider) in this language, or
    /// detecting it on every window (`None`), in short chunks.
    Whisper(Option<String>),
    /// Auto: the fast model streams live text at once while a language
    /// check (Whisper's language detection only, no transcription) listens
    /// to the first seconds; English then goes on with the fast model and
    /// its text is kept, anything else starts over on Whisper in that
    /// language (#70).
    Detect,
}

impl Route {
    /// The route for the saved language (`None` = Auto) and the models
    /// there are, with a note when the choice cannot be honoured.
    pub fn pick(
        language: Option<&str>,
        fast: bool,
        whisper: bool,
    ) -> Option<(Route, Option<String>)> {
        match language {
            Some("en") if fast => Some((Route::Fast, None)),
            Some(l) if whisper => Some((Route::Whisper(Some(l.to_owned())), None)),
            Some(l) if fast => Some((
                Route::Fast,
                Some(format!(
                    "Dictation in {} needs Whisper: install Whisper small in Settings › Local models. The English model is listening for now.",
                    tern_agent::local_models::speech::language_name(Some(l))
                )),
            )),
            None if fast && whisper => Some((Route::Detect, None)),
            None if fast => Some((Route::Fast, None)),
            _ if whisper => Some((Route::Whisper(None), None)),
            _ => None,
        }
    }

    /// The pace for this route.
    pub fn pace(&self) -> Pace {
        match self {
            Route::Fast | Route::Detect => Pace::STREAMING,
            Route::Whisper(_) => Pace::CHUNKED,
        }
    }

    /// What Auto does once Whisper said which language it heard: English
    /// with the fast model there goes fast; anything else stays on Whisper
    /// in that language.
    pub fn heard(&self, language: Option<&str>, fast: bool) -> Route {
        match (self, language) {
            (Route::Detect, Some("en")) if fast => Route::Fast,
            (Route::Detect, l) => Route::Whisper(l.map(str::to_owned)),
            (r, _) => r.clone(),
        }
    }

    /// Where Auto goes once the language check said `language`: English
    /// with the fast model there stays fast; anything else goes to
    /// Whisper in that language. Hindi from the check is often Urdu
    /// heard by a small model, so Whisper then hears each window itself.
    pub fn checked(language: &str, fast: bool) -> Route {
        match language {
            "en" if fast => Route::Fast,
            "hi" | "?" | "" => Route::Whisper(None),
            l => Route::Whisper(Some(l.to_owned())),
        }
    }

    /// The route after a window came back. Auto stays on Whisper
    /// (detecting) until a window that brought words says which language
    /// it is: a quiet or empty window, a failed one, or one without a
    /// language decides nothing (#69: the first recording after launch
    /// went to Moonshine, because the first window, often the breath
    /// before speaking or one Whisper was still loading for, "sounded"
    /// English to Whisper's detector).
    pub fn after_window(&self, language: Option<&str>, fast: bool, spoke: bool) -> Route {
        match self {
            Route::Detect if spoke && language.is_some() => self.heard(language, fast),
            r => r.clone(),
        }
    }
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
    fn whisper_windows_are_sent_once_and_cut_short() {
        let mut d = Dictation::with_pace(Pace::CHUNKED);
        // No interim pass: a 3 s window without a pause waits.
        assert_eq!(d.next_request(&tone(RATE * 3)), None);
        // At 5 s it is cut and committed.
        let s = tone(RATE * 5);
        let r = d.next_request(&s).unwrap();
        assert!(r.last && !r.quiet);
        assert!(r.end <= s.len() && r.end > RATE);
        d.on_result(r.seq, Ok("آپ کیسے ہیں".into()));
        assert_eq!(d.take_committed().as_deref(), Some("آپ کیسے ہیں"));
        assert_eq!(d.interim_line(), None);
    }

    #[test]
    fn no_speech_is_quiet_and_loops_are_dropped() {
        let mut d = Dictation::new();
        d.stop();
        let silence = vec![0.0_f32; RATE];
        let r = d.next_request(&silence).unwrap();
        assert!(r.quiet);
        d.on_result(
            r.seq,
            Err(
                "moonshine-streaming-small (on this computer) heard no speech in the recording."
                    .into(),
            ),
        );
        assert_eq!(d.error, None, "no toast for a pause");
        assert_eq!(d.empty_windows, 1);
        assert!(
            nothing_heard_note(&d, &silence).is_none(),
            "silence: no note"
        );
        // Whisper's "Thank you." on silence is dropped.
        let mut d = Dictation::new();
        d.stop();
        let r = d.next_request(&silence).unwrap();
        d.on_result(r.seq, Ok("Thank you.".into()));
        assert_eq!(d.take_committed(), None);
        // A loop is cut to one copy.
        let mut d = Dictation::new();
        d.stop();
        let s = tone(RATE);
        let r = d.next_request(&s).unwrap();
        d.on_result(
            r.seq,
            Ok("You will not be able to do anything. You will not be able to do anything.".into()),
        );
        assert_eq!(
            d.take_committed().as_deref(),
            Some("You will not be able to do anything.")
        );
    }

    /// Auto, first recording after launch (#69): a quiet first window
    /// that Whisper calls English keeps Whisper; the first window with
    /// words picks the route (Urdu stays on Whisper in Urdu).
    #[test]
    fn auto_waits_for_words_before_picking_a_model() {
        let mut route = Route::pick(None, true, true).unwrap().0;
        assert_eq!(route, Route::Detect);
        let mut d = Dictation::with_pace(route.pace());
        let mut samples = vec![0.0_f32; RATE * 5];
        let r = d.next_request(&samples).expect("a closed quiet window");
        assert!(r.quiet);
        let spoke = d.on_result(r.seq, Ok("you".into()));
        assert!(!spoke);
        route = route.after_window(Some("en"), true, spoke);
        assert_eq!(route, Route::Detect, "a quiet window decides nothing");
        assert_eq!(route.pace(), Pace::STREAMING);
        samples.extend(tone(RATE * 5));
        let r = d.next_request(&samples).expect("a window with speech");
        assert!(!r.quiet);
        let spoke = d.on_result(r.seq, Ok("سلام پاکستان".into()));
        assert!(spoke);
        route = route.after_window(Some("ur"), true, spoke);
        assert_eq!(route, Route::Whisper(Some("ur".into())));
    }

    #[test]
    fn the_language_picks_the_model() {
        assert_eq!(Route::pick(Some("en"), true, true).unwrap().0, Route::Fast);
        assert_eq!(
            Route::pick(Some("ur"), true, true).unwrap().0,
            Route::Whisper(Some("ur".into()))
        );
        let (r, note) = Route::pick(Some("ur"), true, false).unwrap();
        assert_eq!(r, Route::Fast);
        assert!(note.unwrap().contains("Urdu"));
        assert_eq!(Route::pick(None, true, true).unwrap().0, Route::Detect);
        assert_eq!(Route::pick(None, true, false).unwrap().0, Route::Fast);
        assert_eq!(
            Route::pick(None, false, true).unwrap().0,
            Route::Whisper(None)
        );
        assert!(Route::pick(None, false, false).is_none());
        // Auto hears the first window.
        assert_eq!(Route::Detect.heard(Some("en"), true), Route::Fast);
        // ...but only a window that brought words decides (#69).
        assert_eq!(
            Route::Detect.after_window(Some("en"), true, false),
            Route::Detect
        );
        assert_eq!(Route::Detect.after_window(None, true, true), Route::Detect);
        assert_eq!(
            Route::Detect.after_window(Some("en"), true, true),
            Route::Fast
        );
        assert_eq!(
            Route::Whisper(Some("ur".into())).after_window(Some("en"), true, true),
            Route::Whisper(Some("ur".into()))
        );
        assert_eq!(
            Route::Detect.heard(Some("ur"), true),
            Route::Whisper(Some("ur".into()))
        );
        // Auto streams with the fast model while the language is checked.
        assert_eq!(Route::Detect.pace(), Pace::STREAMING);
        assert_eq!(Route::Fast.pace(), Pace::STREAMING);
    }

    #[test]
    fn the_caret_follows_dictated_text() {
        // Empty composer: each phrase goes after the last, caret at the end.
        let mut c = Caret::start(0);
        let (t, at) = c.place("", 0, "hello");
        assert_eq!((t.as_str(), at), ("hello", 5));
        let (t, at) = c.place(&t, at, "world");
        assert_eq!((t.as_str(), at), ("hello world", 11));
        // Text already typed, caret left at the start (set_value): the
        // first phrase still goes at the end.
        let mut c = Caret::start(0);
        let (t, at) = c.place("draft", 0, "more");
        assert_eq!((t.as_str(), at), ("draft more", 10));
        // The user clicks between words during dictation: text goes there.
        let (t, at) = c.place(&t, 5, "inserted");
        assert_eq!((t.as_str(), at), ("draft inserted more", 14));
        // The caret put between two words before the mic started: the
        // phrases go in there, one after the other.
        let mut c = Caret::start(5);
        let (t, at) = c.place("hello world", 5, "compare");
        assert_eq!((t.as_str(), at), ("hello compare world", 13));
        let (t, at) = c.place(&t, at, "retrieval");
        assert_eq!((t.as_str(), at), ("hello compare retrieval world", 23));
        // Urdu, multi-byte.
        let mut c = Caret::start(0);
        let (t, at) = c.place("", 0, "آپ");
        let (t, at) = c.place(&t, at, "کیسے ہیں");
        assert_eq!(t, "آپ کیسے ہیں");
        assert_eq!(at, t.len());
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

    #[test]
    fn auto_checks_the_language_once_from_the_first_seconds() {
        // Nothing to check before 3 s, or in quiet.
        assert_eq!(language_window(&tone(RATE * 2), 0, false), None);
        assert_eq!(language_window(&vec![0.0; RATE * 5], 0, false), None);
        assert_eq!(
            language_window(&tone(RATE * 5), 0, false),
            Some((0, RATE * 4))
        );
        // Stopped early: whatever there is, from a second.
        assert_eq!(
            language_window(&tone(RATE * 2), 0, true),
            Some((0, RATE * 2))
        );
        assert_eq!(Route::checked("en", true), Route::Fast);
        assert_eq!(
            Route::checked("ur", true),
            Route::Whisper(Some("ur".into()))
        );
        assert_eq!(Route::checked("hi", true), Route::Whisper(None));
        assert_eq!(
            Route::checked("en", false),
            Route::Whisper(Some("en".into()))
        );
    }

    #[test]
    fn urdu_heard_starts_the_recording_over_on_whisper() {
        let mut d = Dictation::with_pace(Route::Detect.pace());
        let s = tone(RATE * 3);
        let r = d.next_request(&s).unwrap();
        assert!(!r.last);
        d.on_result(r.seq, Ok("I'll give this guy my 7th answer".into()));
        assert!(d.interim_line().is_some());
        d.restart(Route::Whisper(Some("ur".into())).pace());
        assert_eq!(d.interim_line(), None);
        assert_eq!(d.take_committed(), None);
        assert_eq!(d.open_start(), 0);
        // The old answer is stale now.
        assert!(!d.on_result(r.seq, Ok("garbage".into())));
        // Whisper windows from the start of the recording.
        let mut long = tone(RATE * 5);
        long.extend(vec![0.0; RATE]);
        let w = d.next_request(&long).unwrap();
        assert_eq!(w.start, 0);
    }
}
