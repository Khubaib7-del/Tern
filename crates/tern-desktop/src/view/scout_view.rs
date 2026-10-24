//! Scout's mood clock on the workspace. Scout is drawn as the floating
//! sprite pet (`view/pet_view.rs`, PR #58), which replaced the vector
//! figure that sat in the sidebar corner and beside the flashcards; the
//! rig (`crate::scout`) still decides the mood, so a finished run passes
//! through Done before idle and a flashcard answer still registers.

use std::time::Instant;

use gpui_kit::{Context, Window};

use super::Workspace;
use crate::scout::Mood;

impl Workspace {
    /// What Scout should be doing now: the run's state (the pet adds the
    /// Settings › Scout preview on top, in `pet_snapshot`).
    pub(super) fn scout_mood(&self) -> Mood {
        self.model.scout_mood()
    }

    /// Advances Scout's rig to now. It asks for no frames of its own: the
    /// pet animates itself, and the workspace redraws on real changes.
    pub(super) fn tick_scout(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let now = Instant::now();
        let dt = self
            .scout_at
            .map(|t| now.duration_since(t).as_secs_f32())
            .unwrap_or(0.0);
        self.scout_at = Some(now);
        let mood = self.scout_mood();
        self.scout.set_mood(mood);
        let beat = self.model.working_line().map(|w| w.beat).unwrap_or(0);
        if beat != self.scout_beat {
            self.scout_beat = beat;
            self.scout.kick();
        }
        self.scout.step(if self.reduced_motion { 3.0 } else { dt });
        // Settings › Scout's preview card plays its strip: wake for the
        // next frame (about 10 a second), only while that page shows.
        let previewing = self.model.screen == crate::model::Screen::Settings
            && self.model.settings.section == crate::settings::Section::Scout;
        // The empty state's mark plays while an empty chat shows.
        let empty = self.model.screen == crate::model::Screen::Chat
            && self.model.entries.is_empty()
            && !self.model.is_busy();
        // It plays its loops again on a harness switch.
        if !empty || self.mark_harness != self.model.harness {
            self.mark_since = now;
            self.mark_harness = self.model.harness;
        }
        let wait = if previewing {
            Some(100)
        } else if empty && self.mark_anim.playing(self.mark_since, now) {
            Some(self.mark_anim.frame_ms())
        } else {
            None
        };
        if let Some(wait) = wait
            && !self.reduced_motion
            && self.pet_preview_wake.is_none()
        {
            self.pet_preview_wake = Some(cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(wait))
                    .await;
                let _ = this.update(cx, |this, cx| {
                    this.pet_preview_wake = None;
                    cx.notify();
                });
            }));
        }
    }
}
