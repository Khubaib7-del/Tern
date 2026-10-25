//! Settings › Local models: what the page shows and the downloads in
//! flight. GPUI-free so it is unit tested; `view::localmodels_view` draws
//! it and `bridge` runs the downloads (`tern_agent::local_models`).

use std::collections::HashMap;

use tern_agent::local_models::{
    CancelDownload, ModelRole, ModelStatus, State, Task, can_run, human_size,
};

/// Each Default's Alternative and each Alternative's Default, by id.
fn alternatives(statuses: &[ModelStatus]) -> HashMap<String, (String, String, bool)> {
    let mut out = HashMap::new();
    for s in statuses {
        let want = match s.entry.role {
            ModelRole::Default => ModelRole::Alternative,
            ModelRole::Alternative => ModelRole::Default,
            ModelRole::Addon => continue,
        };
        if let Some(a) = statuses
            .iter()
            .find(|o| o.entry.task == s.entry.task && o.entry.role == want)
        {
            out.insert(
                s.entry.id.clone(),
                (a.entry.id.clone(), a.entry.name.clone(), can_run(&a.entry)),
            );
        }
    }
    out
}

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
    /// A Delete clicked once: the next click on the same model deletes.
    pub confirm_delete: Option<String>,
    /// Each model's backup in its task: its id, name, and whether this
    /// build can run it (filled on load).
    pub alternative: HashMap<String, (String, String, bool)>,
}

impl LocalModelsPage {
    pub fn loaded(&mut self, statuses: Vec<ModelStatus>, root: String, warning: Option<String>) {
        self.alternative = alternatives(&statuses);
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

    /// The first Delete click on a model arms it; the second returns
    /// `true`. Another model's Delete re-arms on that one.
    pub fn arm_delete(&mut self, id: &str) -> bool {
        if self.confirm_delete.as_deref() == Some(id) {
            self.confirm_delete = None;
            true
        } else {
            self.confirm_delete = Some(id.to_owned());
            false
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

    /// The Download button's label: Add for an optional part.
    pub fn download_label(s: &ModelStatus) -> &'static str {
        match s.state {
            State::Partial(_) => "Resume",
            _ if s.entry.role == ModelRole::Addon => "Add",
            _ => "Download",
        }
    }

    /// A failure running a model (from the chat), shown on its row.
    pub fn run_failed(&mut self, id: &str, error: String) {
        self.errors.insert(id.to_owned(), error);
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
        assert_eq!(p.for_task(Task::TextToSpeech).len(), 2);
        assert_eq!(p.for_task(Task::DocParse).len(), 3);
    }

    #[test]
    fn a_download_tracks_progress_cancel_and_errors() {
        let mut p = page();
        let id = "whisper-small-q5_1";
        let cancel = p.start(id).unwrap();
        assert!(p.start(id).is_none(), "one download per model");
        let total = p.downloads[id].total;
        assert!(total > 100_000_000);
        p.run_failed("x", "boom".into());
        assert_eq!(p.errors["x"], "boom");
        p.errors.clear();
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
    fn delete_asks_twice() {
        let mut p = page();
        assert!(!p.arm_delete("a"));
        assert!(!p.arm_delete("b"));
        assert!(p.arm_delete("b"));
        assert!(p.confirm_delete.is_none());
    }

    #[test]
    fn details_and_logos() {
        let p = page();
        let s = p.find("whisper-small-q5_1").unwrap();
        assert_eq!(LocalModelsPage::detail(s), "190 MB · MIT · whisper.cpp");
        assert_eq!(
            p.alternative["whisper-small-q5_1"].0,
            "whisper-large-v3-turbo-q5_0"
        );
        assert_eq!(
            p.alternative["whisper-large-v3-turbo-q5_0"].0,
            "whisper-small-q5_1"
        );
        assert!(!p.alternative.contains_key("pp-formulanet-plus-l"));
        let math = p.find("pp-formulanet-plus-l").unwrap();
        assert_eq!(LocalModelsPage::download_label(math), "Add");
        assert_eq!(LocalModelsPage::download_label(s), "Download");
        assert_eq!(logo_key(s), "whisper");
        // Every source row shows its maker's logo, never a bare initial.
        for s in p.statuses.iter().flatten() {
            assert!(
                matches!(
                    crate::brands::logo(&logo_key(s)),
                    crate::brands::Logo::Mark(..)
                ),
                "{} has no logo",
                s.entry.id
            );
        }
        let mut s = s.clone();
        s.state = State::Partial(10);
        assert_eq!(LocalModelsPage::download_label(&s), "Resume");
    }
}
