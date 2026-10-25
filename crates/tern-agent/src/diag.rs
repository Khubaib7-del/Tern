//! Diagnostic lines for the app's log (`<data>/logs/tern.log`): one line
//! per chat turn (provider, model, tools offered, tool calls made) and per
//! transcription (model, how long each stage took). Front ends set the
//! sink at startup; without one the lines go nowhere. Lines never carry
//! keys, message text, or file contents, only names and counts.

use std::sync::OnceLock;

static SINK: OnceLock<fn(&str)> = OnceLock::new();

/// Sends every later [`note`] to `sink` (the first call wins).
pub fn set_sink(sink: fn(&str)) {
    let _ = SINK.set(sink);
}

/// Writes one line to the log, when a sink is set.
pub fn note(line: &str) {
    if let Some(sink) = SINK.get() {
        sink(line);
    }
}
