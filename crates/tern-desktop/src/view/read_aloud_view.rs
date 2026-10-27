//! Read aloud on answers (#69): the speaker button under an answer reads
//! it with the local voice model (Kokoro, Settings › Speech › Read aloud
//! voice and speed). While it reads, the button becomes Pause/Play and
//! Stop with "Reading 2 of 5"; leaving the chat, a new reading, or Stop
//! ends it. Text Kokoro cannot read (Urdu) gets a plain note; a missing
//! or failing voice model gets the local-model banner with Download / Use
//! the alternative. The polling timer only notifies the view (no
//! animation frames outside a render; see `view::frame`).

use std::time::Duration;

use gpui_kit::component::Sizable as _;
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, SharedString, Window, div, px};
use tern_agent::local_models::Task as ModelTask;
use tern_agent::read_aloud::{Missing, Phase, Status};

use gpui_kit::assets::IconName as AssetIcon;

use super::Workspace;
use crate::theme::Palette;

/// The answer being read.
pub(crate) struct ReadingNow {
    /// The answer's entry in the transcript.
    pub entry: usize,
    /// The chat it is in; reading stops when another chat shows.
    pub session: Option<tern_core::SessionId>,
    /// The voice model, for the failure banner.
    pub model: String,
    /// The reading itself (`None` in the UI tour's mock).
    pub player: Option<tern_agent::read_aloud::ReadAloud>,
    pub status: Status,
}

impl std::fmt::Debug for ReadingNow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReadingNow")
            .field("entry", &self.entry)
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

/// The small label beside the reading buttons.
pub(crate) fn status_label(s: &Status) -> String {
    match &s.phase {
        Phase::Preparing => "Preparing the voice…".to_owned(),
        Phase::Playing => format!("Reading {} of {}", s.sentence + 1, s.sentences.max(1)),
        Phase::Paused => format!("Paused · {} of {}", s.sentence + 1, s.sentences.max(1)),
        Phase::Done => "Done".to_owned(),
        Phase::Failed(_) => "Could not read".to_owned(),
    }
}

impl Workspace {
    /// Starts reading answer `i` aloud, or stops the reading of it.
    pub(super) fn toggle_read_aloud(
        &mut self,
        i: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.read_aloud.as_ref().is_some_and(|r| r.entry == i) {
            self.stop_read_aloud();
            cx.notify();
            return;
        }
        self.stop_read_aloud();
        let Some(text) = self.model.entry_text(i).map(str::to_owned) else {
            return;
        };
        let sentences = match tern_agent::read_aloud::prepare(&text) {
            Ok(s) => s,
            Err(m) => {
                self.model.toast_info(m.message());
                cx.notify();
                return;
            }
        };
        let prefs = self.model.providers.prefs.clone();
        let lm = match tern_agent::local_models::LocalModels::discover() {
            Ok((lm, _)) => lm,
            Err(e) => {
                self.model.toast_error(format!("Read aloud: {e}"), None);
                cx.notify();
                return;
            }
        };
        let model = match tern_agent::read_aloud::voice_model(&lm, &prefs.local_models) {
            Ok(m) => m,
            Err(Missing::Download { id, name }) => {
                let alternative = lm
                    .manifest()
                    .find(&id)
                    .and_then(|e| lm.alternative_to(e))
                    .filter(|a| tern_agent::local_models::can_run(a))
                    .map(|a| (a.id.clone(), a.name.clone(), lm.installed(&a.id).is_some()));
                self.model.local_failed = Some(crate::chatflow::LocalFailure {
                    id,
                    name,
                    task: ModelTask::TextToSpeech,
                    error: "it is not downloaded yet".into(),
                    alternative,
                    missing: true,
                });
                cx.notify();
                return;
            }
            Err(m) => {
                self.model.toast_info(m.message());
                cx.notify();
                return;
            }
        };
        let voice = tern_agent::local_models::kokoro_text::voice_or_default(
            prefs.read_aloud_voice.as_deref(),
        )
        .to_owned();
        let speed = prefs.read_aloud_speed.unwrap_or(100).clamp(50, 200) as f32 / 100.0;
        let total = sentences.len();
        match tern_agent::read_aloud::ReadAloud::start(model.clone(), sentences, voice, speed) {
            Ok(player) => {
                tern_agent::diag::note(&format!(
                    "tts: read aloud answer {i} ({total} sentences) with {model}"
                ));
                self.read_aloud = Some(ReadingNow {
                    entry: i,
                    session: self.current,
                    model,
                    player: Some(player),
                    status: Status {
                        phase: Phase::Preparing,
                        sentence: 0,
                        sentences: total,
                    },
                });
                self.start_read_aloud_timer(window, cx);
            }
            Err(e) => self.model.toast_error(format!("Read aloud: {e}"), None),
        }
        cx.notify();
    }

    /// Pause or play the answer being read.
    pub(super) fn pause_read_aloud(&mut self, cx: &mut Context<Self>) {
        if let Some(r) = self.read_aloud.as_mut() {
            if let Some(p) = &r.player {
                p.toggle_pause();
                r.status = p.status();
            } else {
                // The tour's mock flips by itself.
                r.status.phase = match r.status.phase {
                    Phase::Paused => Phase::Playing,
                    _ => Phase::Paused,
                };
            }
        }
        cx.notify();
    }

    /// Stops reading (Stop, a new reading, another chat).
    pub(super) fn stop_read_aloud(&mut self) {
        if let Some(r) = self.read_aloud.take()
            && let Some(p) = &r.player
        {
            p.stop();
        }
        self.read_aloud_task = None;
    }

    /// Every 250 ms while reading: the state for the buttons; the end, a
    /// failure, or a chat change ends it.
    fn start_read_aloud_timer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.read_aloud_task = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
                let go = this.update_in(cx, |this, _, cx| this.poll_read_aloud(cx));
                if !matches!(go, Ok(true)) {
                    break;
                }
            }
        }));
    }

    fn poll_read_aloud(&mut self, cx: &mut Context<Self>) -> bool {
        let current = self.current;
        let Some(r) = self.read_aloud.as_mut() else {
            return false;
        };
        if r.session != current {
            self.stop_read_aloud();
            cx.notify();
            return false;
        }
        let Some(p) = &r.player else {
            return true;
        };
        let st = p.status();
        let changed = st != r.status;
        r.status = st.clone();
        match st.phase {
            Phase::Done => {
                self.read_aloud = None;
                cx.notify();
                false
            }
            Phase::Failed(e) => {
                let model = r.model.clone();
                self.read_aloud = None;
                self.model.local_failed = self.model.settings.local.failure(&model, &e).or(Some(
                    crate::chatflow::LocalFailure {
                        id: model.clone(),
                        name: model,
                        task: ModelTask::TextToSpeech,
                        error: e,
                        alternative: None,
                        missing: false,
                    },
                ));
                cx.notify();
                false
            }
            _ => {
                if changed {
                    cx.notify();
                }
                true
            }
        }
    }

    /// The read-aloud control under answer `i`: a speaker button, or
    /// Pause/Play · Stop · "Reading 2 of 5" while it is read.
    pub(super) fn read_aloud_action(
        &self,
        i: usize,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let button = move |id: &'static str, icon: AssetIcon, tip: &'static str| {
            div()
                .id((id, i))
                .flex()
                .items_center()
                .gap(px(2.))
                .p(px(3.))
                .rounded_md()
                .cursor_pointer()
                .text_color(p.faint)
                .hover(move |d| d.text_color(p.text).bg(p.hover))
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(tip).build(window, cx)
                })
                .child(gpui_kit::component::Icon::new(icon).xsmall())
        };
        let Some(r) = self.read_aloud.as_ref().filter(|r| r.entry == i) else {
            return button(
                "read-aloud",
                AssetIcon::Volume2,
                "Read aloud (on this computer)",
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.toggle_read_aloud(i, window, cx);
            }))
            .into_any_element();
        };
        let paused = r.status.phase == Phase::Paused;
        let label: SharedString = status_label(&r.status).into();
        div()
            .id(("read-aloud-on", i))
            .flex()
            .items_center()
            .gap(px(2.))
            .px(px(2.))
            .rounded_md()
            .bg(p.hover)
            .child(
                button(
                    "read-aloud-pause",
                    if paused {
                        AssetIcon::Play
                    } else {
                        AssetIcon::Pause
                    },
                    if paused { "Play" } else { "Pause" },
                )
                .text_color(p.text)
                .on_click(cx.listener(move |this, _, _, cx| this.pause_read_aloud(cx))),
            )
            .child(
                button("read-aloud-stop", AssetIcon::Square, "Stop reading")
                    .text_color(p.text)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.stop_read_aloud();
                        cx.notify();
                    })),
            )
            .child(div().text_xs().text_color(p.muted).pr_1().child(label))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_label_says_where_reading_is() {
        let mut s = Status {
            phase: Phase::Preparing,
            sentence: 0,
            sentences: 5,
        };
        assert_eq!(status_label(&s), "Preparing the voice…");
        s.phase = Phase::Playing;
        s.sentence = 1;
        assert_eq!(status_label(&s), "Reading 2 of 5");
        s.phase = Phase::Paused;
        assert_eq!(status_label(&s), "Paused · 2 of 5");
    }
}
