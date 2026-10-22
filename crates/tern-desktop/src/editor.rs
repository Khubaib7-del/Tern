//! The Editor screen's state (`docs/02-ui-definition.md`, Editor rail
//! item): the vault's notes, the open note, and whether it has unsaved
//! changes. GPUI-free so it is unit tested; files go through
//! `tern_agent::vault`.

use std::sync::Arc;

use tern_agent::vault::{self, Note, Vault};

/// A `[[link]]` in the open note and the note it reaches, if any.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub target: String,
    pub note: Option<String>,
}

#[derive(Debug, Default)]
pub struct EditorState {
    pub vault: Option<Arc<Vault>>,
    pub notes: Vec<Note>,
    /// Path of the open note.
    pub open: Option<String>,
    /// The text as last saved, to tell when there are changes.
    saved: String,
    pub dirty: bool,
    /// Show rendered Markdown instead of the text field.
    pub preview: bool,
    pub links: Vec<Link>,
    pub backlinks: Vec<Note>,
    pub error: Option<String>,
    /// Bumped on every edit; a pending autosave only runs if it still
    /// matches.
    pub generation: u64,
}

impl EditorState {
    /// Uses `vault` and lists its notes.
    pub fn attach(&mut self, vault: Vault) {
        self.vault = Some(Arc::new(vault));
        self.refresh();
    }

    pub fn refresh(&mut self) {
        if let Some(v) = &self.vault {
            self.notes = v.list();
        }
    }

    fn vault(&self) -> Result<&Arc<Vault>, String> {
        self.vault
            .as_ref()
            .ok_or_else(|| "The vault is not open.".into())
    }

    /// Opens a note. Returns its text for the editor.
    pub fn open_note(&mut self, path: &str) -> Result<String, String> {
        let text = self.vault()?.read(path)?;
        self.open = Some(path.to_owned());
        self.saved = text.clone();
        self.dirty = false;
        self.error = None;
        self.relink(&text);
        Ok(text)
    }

    /// The editor's text changed. Returns the generation to autosave.
    pub fn edited(&mut self, text: &str) -> u64 {
        self.dirty = self.open.is_some() && text != self.saved;
        self.generation += 1;
        self.generation
    }

    /// Saves the open note when it has changes.
    pub fn save(&mut self, text: &str) -> Result<(), String> {
        let Some(path) = self.open.clone() else {
            return Ok(());
        };
        if text == self.saved {
            self.dirty = false;
            return Ok(());
        }
        match self.vault()?.write(&path, text) {
            Ok(()) => {
                self.saved = text.to_owned();
                self.dirty = false;
                self.error = None;
                self.relink(text);
                self.refresh();
                Ok(())
            }
            Err(e) => {
                self.error = Some(e.clone());
                Err(e)
            }
        }
    }

    /// Makes an empty note and opens it. Returns its path and text.
    pub fn new_note(&mut self) -> Result<(String, String), String> {
        let path = self.vault()?.create("Untitled", "# Untitled\n\n")?;
        self.refresh();
        let text = self.open_note(&path)?;
        Ok((path, text))
    }

    /// Saves an answer as a new note. Returns its path.
    pub fn save_answer(
        &mut self,
        question: &str,
        answer: &str,
        sources: &[(String, String)],
    ) -> Result<String, String> {
        let (title, text) = vault::answer_note(question, answer, sources);
        let path = self.vault()?.create(&title, &text)?;
        self.refresh();
        Ok(path)
    }

    fn relink(&mut self, text: &str) {
        let Ok(v) = self.vault() else { return };
        let v = v.clone();
        self.links = vault::links(text)
            .into_iter()
            .map(|target| Link {
                note: v.find(&target).map(|n| n.path),
                target,
            })
            .collect();
        self.links.dedup();
        self.backlinks = match &self.open {
            Some(p) => v.backlinks(&Note { path: p.clone() }),
            None => Vec::new(),
        };
    }

    /// The path a link leads to, creating the note when it does not exist
    /// yet (as Obsidian does on click).
    pub fn follow(&mut self, link: &Link) -> Result<String, String> {
        if let Some(p) = &link.note {
            return Ok(p.clone());
        }
        let v = self.vault()?.clone();
        let path = v.create(
            &link.target,
            &format!("# {}\n\n", vault::clean_name(&link.target)),
        )?;
        self.refresh();
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor() -> (tempfile::TempDir, EditorState) {
        let dir = tempfile::tempdir().unwrap();
        let v = Vault::at(dir.path());
        v.write("RAG.md", "See [[Transformers]] and [[Missing]].")
            .unwrap();
        v.write("Transformers.md", "Attention.").unwrap();
        let mut e = EditorState::default();
        e.attach(v);
        (dir, e)
    }

    #[test]
    fn open_edit_save() {
        let (_d, mut e) = editor();
        assert_eq!(e.notes.len(), 2);
        let text = e.open_note("RAG.md").unwrap();
        assert_eq!(e.links.len(), 2);
        assert_eq!(e.links[0].note.as_deref(), Some("Transformers.md"));
        assert_eq!(e.links[1].note, None);
        let g = e.edited(&text);
        assert!(!e.dirty);
        let g2 = e.edited("changed");
        assert!(e.dirty && g2 > g);
        e.save("changed").unwrap();
        assert!(!e.dirty);
        assert_eq!(e.open_note("RAG.md").unwrap(), "changed");
        assert!(e.links.is_empty());
    }

    #[test]
    fn backlinks_new_notes_answers_and_links() {
        let (_d, mut e) = editor();
        e.open_note("Transformers.md").unwrap();
        assert_eq!(e.backlinks.len(), 1);
        let (path, text) = e.new_note().unwrap();
        assert_eq!(path, "Untitled.md");
        assert!(text.starts_with("# Untitled"));
        assert_eq!(e.open.as_deref(), Some("Untitled.md"));
        let saved = e.save_answer("What is RAG?", "Retrieval.", &[]).unwrap();
        assert_eq!(saved, "What is RAG.md");
        e.open_note("RAG.md").unwrap();
        let missing = e.links[1].clone();
        assert_eq!(e.follow(&missing).unwrap(), "Missing.md");
        assert!(e.notes.iter().any(|n| n.path == "Missing.md"));
    }
}
