//! The Decision model settings page's state: mode, provider, keys, and the
//! last test. GPUI-free, so it is tested like the rest of the model.

use tern_agent::decide::{DecisionPrefs, DecisionRow, Mode};

/// A row's last action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowNote {
    Working(String),
    Ok(String),
    Failed(String),
}

/// The page's state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DecisionPage {
    pub loaded: bool,
    pub prefs: DecisionPrefs,
    pub rows: Vec<RowView>,
    pub keychain_error: Option<String>,
    /// The row whose key is being typed.
    pub editing: Option<&'static str>,
    pub notes: Vec<(&'static str, RowNote)>,
    /// The "Test" button's last result for the decider in use.
    pub test: Option<RowNote>,
}

/// One remote provider row, as the page shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowView {
    pub id: &'static str,
    pub name: &'static str,
    pub product: &'static str,
    pub icon: &'static str,
    pub key_saved: bool,
    pub key_url: &'static str,
    pub default_model: &'static str,
    pub price: String,
    pub scope: &'static str,
}

impl From<DecisionRow> for RowView {
    fn from(r: DecisionRow) -> Self {
        Self {
            id: r.id,
            name: r.name,
            product: r.product,
            icon: r.icon,
            key_saved: r.key_saved,
            key_url: r.key_url,
            default_model: r.default_model,
            price: r.price,
            scope: r.scope,
        }
    }
}

impl DecisionPage {
    pub fn load(&mut self, rows: Vec<DecisionRow>, prefs: DecisionPrefs, err: Option<String>) {
        self.loaded = true;
        self.rows = rows.into_iter().map(RowView::from).collect();
        self.prefs = prefs;
        self.keychain_error = err;
    }

    pub fn mode(&self) -> Mode {
        self.prefs.mode()
    }

    /// Sets the mode. Remote with no provider picks the first one whose
    /// key is saved, if any.
    pub fn set_mode(&mut self, mode: Mode) {
        self.prefs.mode = Some(mode.label().to_lowercase());
        if mode == Mode::Remote
            && self.prefs.provider.is_none()
            && let Some(r) = self.rows.iter().find(|r| r.key_saved)
        {
            self.prefs.provider = Some(r.id.to_owned());
        }
        self.test = None;
    }

    /// Makes `id` the remote provider (and switches to remote).
    pub fn use_provider(&mut self, id: &'static str) {
        if self.prefs.provider.as_deref() != Some(id) {
            self.prefs.model = None;
        }
        self.prefs.provider = Some(id.to_owned());
        self.prefs.mode = Some("remote".into());
        self.test = None;
    }

    pub fn is_chosen(&self, id: &str) -> bool {
        self.mode() == Mode::Remote && self.prefs.provider.as_deref() == Some(id)
    }

    pub fn note(&self, id: &str) -> Option<&RowNote> {
        self.notes
            .iter()
            .rev()
            .find(|(i, _)| *i == id)
            .map(|(_, n)| n)
    }

    pub fn set_note(&mut self, id: &'static str, note: RowNote) {
        self.notes.retain(|(i, _)| *i != id);
        self.notes.push((id, note));
    }

    pub fn is_working(&self, id: &str) -> bool {
        matches!(self.note(id), Some(RowNote::Working(_)))
    }

    /// A key test or save finished.
    pub fn checked(&mut self, id: &'static str, result: Result<String, String>) {
        let ok = result.is_ok();
        self.set_note(
            id,
            match result {
                Ok(t) => RowNote::Ok(t),
                Err(e) => RowNote::Failed(e),
            },
        );
        if ok {
            if let Some(r) = self.rows.iter_mut().find(|r| r.id == id) {
                r.key_saved = true;
            }
            if self.editing == Some(id) {
                self.editing = None;
            }
        }
    }

    pub fn removed(&mut self, id: &'static str, result: Result<bool, String>) {
        match result {
            Ok(_) => {
                if let Some(r) = self.rows.iter_mut().find(|r| r.id == id) {
                    r.key_saved = false;
                }
                self.set_note(id, RowNote::Ok("Key removed".into()));
            }
            Err(e) => self.set_note(id, RowNote::Failed(e)),
        }
    }

    /// What the page header says is in use.
    pub fn in_use(&self) -> String {
        let label = self.prefs.label();
        match (self.mode(), self.prefs.spec()) {
            (Mode::Remote, Some(s)) if !self.rows.iter().any(|r| r.id == s.id && r.key_saved) => {
                format!("{label} · no key saved, so decisions are local")
            }
            _ => label,
        }
    }
}

/// One line for a test decision's result.
pub fn test_line(d: &tern_agent::decide::Decision) -> String {
    let mut s = format!(
        "{} chose “{}” at {:.0}% in {} ms",
        d.model,
        d.chosen_id(),
        d.probabilities.get(d.chosen).copied().unwrap_or(0.0) * 100.0,
        d.latency_ms
    );
    if d.cost.input_tokens > 0 {
        s.push_str(&format!(
            ", {} tokens, ${:.6}",
            d.cost.input_tokens, d.cost.usd
        ));
    }
    if let Some(f) = &d.fallback {
        s.push_str(&format!(". Fell back to local: {f}"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page() -> DecisionPage {
        let mut p = DecisionPage::default();
        p.load(
            tern_agent::decide::list(None),
            DecisionPrefs::default(),
            None,
        );
        p
    }

    #[test]
    fn modes_and_providers() {
        let mut p = page();
        assert_eq!(p.mode(), Mode::Local);
        assert_eq!(p.rows.len(), 3);
        assert_eq!(p.in_use(), "Local");
        p.set_mode(Mode::Remote);
        assert_eq!(p.prefs.provider, None, "no saved key to pick");
        p.use_provider("subq");
        assert!(p.is_chosen("subq"));
        assert!(p.in_use().contains("no key saved"));
        p.checked("subq", Ok("Works".into()));
        assert!(p.rows.iter().find(|r| r.id == "subq").unwrap().key_saved);
        assert!(!p.in_use().contains("no key"));
        p.removed("subq", Ok(true));
        assert!(!p.rows.iter().find(|r| r.id == "subq").unwrap().key_saved);
        p.set_mode(Mode::Off);
        assert!(!p.is_chosen("subq"));
    }

    #[test]
    fn notes_replace_each_other() {
        let mut p = page();
        p.set_note("voyage", RowNote::Working("Checking…".into()));
        assert!(p.is_working("voyage"));
        p.checked("voyage", Err("401".into()));
        assert_eq!(p.note("voyage"), Some(&RowNote::Failed("401".into())));
        assert_eq!(p.notes.len(), 1);
    }
}
