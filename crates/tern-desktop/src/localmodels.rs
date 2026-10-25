//! Settings › Local models: what the page shows and the downloads in
//! flight. GPUI-free so it is unit tested; `view::localmodels_view` draws
//! it and `bridge` runs the downloads (`tern_agent::local_models`).

use std::collections::HashMap;

use tern_agent::local_models::{CancelDownload, ModelStatus, State, Task, human_size};

/// One download in flight.
#[derive(Clone, Debug)]
pub struct Download {
    pub done: u64,
    pub total: u64,
    /// The file being checked, once the bytes are in.
    pub verifying: Option<String>,
    pub cancel: CancelDownload,
}

impl Download {
    pub fn new(total: u64) -> Self {
        Self {
            done: 0,
            total,
            verifying: None,
            cancel: CancelDownload::new(),
        }
    }

    /// 0.0 to 1.0.
    pub fn fraction(&self) -> f32 {
        if self.total == 0 {
            0.
        } else {
            (self.done as f64 / self.total as f64).clamp(0., 1.) as f32
        }
    }

    /// `42% · 62 MB of 148 MB`, or `Checking ggml-base.en.bin…`.
    pub fn line(&self) -> String {
        match &self.verifying {
            Some(f) => format!("Checking {f}…"),
            None => format!(
                "{:.0}% · {} of {}",
                self.fraction() * 100.,
                human_size(self.done),
                human_size(self.total)
            ),
        }
    }
}

/// The page's state.
#[derive(Clone, Debug, Default)]
pub struct LocalModelsPage {
    /// `None` until the first load comes back.
    pub statuses: Option<Vec<ModelStatus>>,
    /// Downloads in flight, by model id.
    pub downloads: HashMap<String, Download>,
    /// The last error per model id (a failed download or delete).
    pub errors: HashMap<String, String>,
    /// A broken `local-models.json`, or a folder that cannot be read.
    pub warning: Option<String>,
    /// Where models are stored, for the page's footer.
    pub root: Option<String>,
}

impl LocalModelsPage {
    pub fn loaded(&mut self, statuses: Vec<ModelStatus>, root: String, warning: Option<String>) {
        self.statuses = Some(statuses);
        self.root = Some(root);
        self.warning = warning;
    }

    /// The models listed for `task`, in manifest order.
    pub fn for_task(&self, task: Task) -> Vec<&ModelStatus> {
        self.statuses
            .iter()
            .flatten()
            .filter(|s| s.entry.task == task)
            .collect()
    }

    pub fn find(&self, id: &str) -> Option<&ModelStatus> {
        self.statuses.iter().flatten().find(|s| s.entry.id == id)
    }

    /// Starts tracking a download; `None` when one is already running.
    pub fn start(&mut self, id: &str) -> Option<CancelDownload> {
        if self.downloads.contains_key(id) {
            return None;
        }
        let total = self.find(id).map_or(0, |s| s.entry.size());
        let d = Download::new(total);
        let cancel = d.cancel.clone();
        self.errors.remove(id);
        self.downloads.insert(id.to_owned(), d);
        Some(cancel)
    }

    pub fn progress(&mut self, id: &str, done: u64, total: u64) {
        if let Some(d) = self.downloads.get_mut(id) {
            d.done = done;
            d.total = total;
            d.verifying = None;
        }
    }

    pub fn verifying(&mut self, id: &str, file: String) {
        if let Some(d) = self.downloads.get_mut(id) {
            d.verifying = Some(file);
        }
    }

    pub fn cancel(&mut self, id: &str) {
        if let Some(d) = self.downloads.get(id) {
            d.cancel.cancel();
        }
    }

    /// A download ended. A cancel is not an error.
    pub fn finished(&mut self, id: &str, result: Result<(), String>, cancelled: bool) {
        self.downloads.remove(id);
        match result {
            Err(e) if !cancelled => {
                self.errors.insert(id.to_owned(), e);
            }
            _ => {
                self.errors.remove(id);
            }
        }
    }

    /// The row's second line: size, licence, runtime, and state.
    pub fn detail(s: &ModelStatus) -> String {
        let state = match s.state {
            State::Installed if !s.enabled => " · installed, off",
            State::Installed => " · installed",
            State::Partial(_) => " · partly downloaded",
            State::NotInstalled => "",
        };
        format!(
            "{} · {} · {}{}",
            human_size(s.entry.size()),
            s.entry.licence,
            s.entry.runtime.label(),
            state
        )
    }

    /// The Download button's label.
    pub fn download_label(s: &ModelStatus) -> &'static str {
        match s.state {
            State::Partial(_) => "Resume",
            _ => "Download",
        }
    }
}

/// The brand key for a model's logo (its manifest `logo`, else the repo
/// owner), for `brands::logo`.
pub fn logo_key(s: &ModelStatus) -> String {
    s.entry
        .logo
        .clone()
        .unwrap_or_else(|| s.entry.hf_repo.split('/').next().unwrap_or("").to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tern_agent::local_models::Manifest;

    fn page() -> LocalModelsPage {
        let mut p = LocalModelsPage::default();
        let st = Manifest::builtin()
            .models
            .into_iter()
            .map(|e| ModelStatus {
                entry: e,
                state: State::NotInstalled,
                enabled: true,
                is_default: false,
            })
            .collect();
        p.loaded(st, "/m".into(), None);
        p
    }

    #[test]
    fn rows_group_by_task() {
        let p = page();
        assert!(!p.for_task(Task::Ocr).is_empty());
        assert!(!p.for_task(Task::SpeechToText).is_empty());
        assert!(p.for_task(Task::TextToSpeech).is_empty());
    }

    #[test]
    fn a_download_tracks_progress_cancel_and_errors() {
        let mut p = page();
        let id = "whisper-base-en";
        let cancel = p.start(id).unwrap();
        assert!(p.start(id).is_none(), "one download per model");
        let total = p.downloads[id].total;
        assert!(total > 100_000_000);
        p.progress(id, total / 2, total);
        assert!((p.downloads[id].fraction() - 0.5).abs() < 0.01);
        assert!(p.downloads[id].line().starts_with("50% · "));
        p.verifying(id, "ggml-base.en.bin".into());
        assert_eq!(p.downloads[id].line(), "Checking ggml-base.en.bin…");
        p.cancel(id);
        assert!(cancel.is_cancelled());
        p.finished(id, Err("cancelled".into()), true);
        assert!(p.downloads.is_empty() && p.errors.is_empty());
        p.start(id);
        p.finished(id, Err("network".into()), false);
        assert_eq!(p.errors[id], "network");
        p.start(id);
        assert!(p.errors.is_empty(), "a new try clears the error");
    }

    #[test]
    fn details_and_logos() {
        let p = page();
        let s = p.find("whisper-base-en").unwrap();
        assert_eq!(LocalModelsPage::detail(s), "148 MB · MIT · whisper.cpp");
        assert_eq!(LocalModelsPage::download_label(s), "Download");
        assert_eq!(logo_key(s), "whisper");
        let mut s = s.clone();
        s.state = State::Partial(10);
        assert_eq!(LocalModelsPage::download_label(&s), "Resume");
    }
}
