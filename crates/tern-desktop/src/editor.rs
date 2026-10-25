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
    /// A change to a note waiting for Accept or Reject (v2 s13's agent
    /// diff): the note changed on disk under the editor, or an answer was
    /// saved onto a note that already exists.
    pub proposal: Option<Proposal>,
}

/// A change to a note, shown as a diff until accepted or rejected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proposal {
    pub path: String,
    /// The text the user has (the note as the editor last saved it).
    pub before: String,
    /// The text the change would leave.
    pub after: String,
    /// Where it came from: "Changed outside the editor", "Answer saved
    /// onto this note".
    pub source: String,
    /// The change is already on disk (written by the agent or another app);
    /// rejecting writes `before` back.
    pub on_disk: bool,
}

/// One line of a diff.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Line<'a> {
    Same(&'a str),
    Added(&'a str),
    Removed(&'a str),
}

/// A line diff of `a` → `b` (longest common subsequence). Very long notes
/// fall back to "everything removed, everything added" past 4,000 lines.
pub fn diff<'a>(a: &'a str, b: &'a str) -> Vec<Line<'a>> {
    let x: Vec<&str> = a.lines().collect();
    let y: Vec<&str> = b.lines().collect();
    // Common head and tail first: most edits are local.
    let head = x.iter().zip(&y).take_while(|(p, q)| p == q).count();
    let tail = x[head..]
        .iter()
        .rev()
        .zip(y[head..].iter().rev())
        .take_while(|(p, q)| p == q)
        .count();
    let (xm, ym) = (&x[head..x.len() - tail], &y[head..y.len() - tail]);
    let mut out: Vec<Line<'a>> = x[..head].iter().map(|l| Line::Same(l)).collect();
    if xm.len() * ym.len() > 4_000 * 4_000 / 4 {
        out.extend(xm.iter().map(|l| Line::Removed(l)));
        out.extend(ym.iter().map(|l| Line::Added(l)));
    } else {
        let (n, m) = (xm.len(), ym.len());
        let mut t = vec![vec![0u32; m + 1]; n + 1];
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                t[i][j] = if xm[i] == ym[j] {
                    t[i + 1][j + 1] + 1
                } else {
                    t[i + 1][j].max(t[i][j + 1])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < n && j < m {
            if xm[i] == ym[j] {
                out.push(Line::Same(xm[i]));
                i += 1;
                j += 1;
            } else if t[i + 1][j] >= t[i][j + 1] {
                out.push(Line::Removed(xm[i]));
                i += 1;
            } else {
                out.push(Line::Added(ym[j]));
                j += 1;
            }
        }
        out.extend(xm[i..].iter().map(|l| Line::Removed(l)));
        out.extend(ym[j..].iter().map(|l| Line::Added(l)));
    }
    out.extend(x[x.len() - tail..].iter().map(|l| Line::Same(l)));
    out
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

    /// Whether the open note changed on disk since the editor last read or
    /// saved it (the agent's vault tools or another app wrote it). The
    /// change becomes a proposal to accept or reject. Returns whether one
    /// was made.
    pub fn check_disk(&mut self) -> bool {
        let Some(path) = self.open.clone() else {
            return false;
        };
        if self.proposal.as_ref().is_some_and(|p| p.path == path) {
            return false;
        }
        let Ok(disk) = self.vault().and_then(|v| v.read(&path)) else {
            return false;
        };
        if disk == self.saved {
            return false;
        }
        self.proposal = Some(Proposal {
            path,
            before: self.saved.clone(),
            after: disk,
            source: "Changed outside the editor".into(),
            on_disk: true,
        });
        true
    }

    /// Takes the proposal: writes its text and returns (path, text) to
    /// show when that note is open.
    pub fn accept(&mut self) -> Result<Option<(String, String)>, String> {
        let Some(p) = self.proposal.take() else {
            return Ok(None);
        };
        if !p.on_disk {
            self.vault()?.write(&p.path, &p.after)?;
        }
        if self.open.as_deref() == Some(p.path.as_str()) {
            self.saved = p.after.clone();
            self.dirty = false;
            self.relink(&p.after);
        }
        self.refresh();
        Ok(Some((p.path, p.after)))
    }

    /// Drops the proposal; a change already on disk is undone by writing
    /// the user's text back.
    pub fn reject(&mut self) -> Result<(), String> {
        let Some(p) = self.proposal.take() else {
            return Ok(());
        };
        if p.on_disk {
            self.vault()?.write(&p.path, &p.before)?;
        }
        Ok(())
    }

    /// Saves the open note when it has changes. A note changed on disk
    /// meanwhile is not overwritten: the change waits as a proposal.
    pub fn save(&mut self, text: &str) -> Result<(), String> {
        let Some(path) = self.open.clone() else {
            return Ok(());
        };
        if text == self.saved {
            self.dirty = false;
            return Ok(());
        }
        if self.proposal.as_ref().is_some_and(|p| p.path == path) || self.check_disk() {
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

    /// Saves an answer as a new note. Returns its path. When a note of
    /// that title exists, the answer is proposed as an addition to it
    /// instead (accept or reject in the Editor), and `Err` says so.
    pub fn save_answer(
        &mut self,
        question: &str,
        answer: &str,
        sources: &[(String, String)],
    ) -> Result<String, String> {
        let (title, text) = vault::answer_note(question, answer, sources);
        let v = self.vault()?.clone();
        if let Some(note) = v.find(&title)
            && let Ok(before) = v.read(&note.path)
        {
            let after = format!("{}\n\n{}", before.trim_end(), text.trim());
            self.proposal = Some(Proposal {
                path: note.path.clone(),
                before,
                after,
                source: "Answer saved onto this note".into(),
                on_disk: false,
            });
            return Err(format!(
                "{} exists: review the addition in the Editor",
                note.path
            ));
        }
        let path = v.create(&title, &text)?;
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
    fn diff_marks_added_and_removed_lines() {
        let d = diff("a\nb\nc\nd", "a\nB\nc\nd\ne");
        assert_eq!(
            d,
            vec![
                Line::Same("a"),
                Line::Removed("b"),
                Line::Added("B"),
                Line::Same("c"),
                Line::Same("d"),
                Line::Added("e"),
            ]
        );
        assert_eq!(diff("", "x"), vec![Line::Added("x")]);
        assert!(
            diff("same", "same")
                .iter()
                .all(|l| matches!(l, Line::Same(_)))
        );
    }

    #[test]
    fn a_change_on_disk_waits_for_accept_or_reject() {
        let (d, mut e) = editor();
        e.open_note("RAG.md").unwrap();
        std::fs::write(d.path().join("RAG.md"), "agent wrote this").unwrap();
        // Saving the user's edit does not overwrite the agent's change.
        e.save("mine").unwrap();
        let p = e.proposal.clone().unwrap();
        assert!(p.on_disk && p.after == "agent wrote this");
        e.reject().unwrap();
        assert_eq!(
            std::fs::read_to_string(d.path().join("RAG.md")).unwrap(),
            p.before
        );
        std::fs::write(d.path().join("RAG.md"), "again").unwrap();
        assert!(e.check_disk());
        assert_eq!(
            e.accept().unwrap(),
            Some(("RAG.md".to_owned(), "again".to_owned()))
        );
        assert!(!e.check_disk());
    }

    #[test]
    fn an_answer_onto_an_existing_note_is_proposed() {
        let (d, mut e) = editor();
        let first = e.save_answer("What is RAG?", "Retrieval.", &[]).unwrap();
        assert!(e.save_answer("What is RAG?", "More.", &[]).is_err());
        let p = e.proposal.clone().unwrap();
        assert_eq!(p.path, first);
        assert!(!p.on_disk && p.after.contains("More."));
        e.accept().unwrap();
        assert!(
            std::fs::read_to_string(d.path().join(&first))
                .unwrap()
                .contains("More.")
        );
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
