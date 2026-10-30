//! What a turn already changed in the folder (#82), so a model that does
//! it again is told instead of asked about it.
//!
//! From the #80 Windows test: codestral wrote counts.md nine times in one
//! turn (an approval card each time), and ministral ran a code-mode script
//! that wrote draft.md and renamed it to hello.md, then made the same two
//! calls directly (two more cards). The turn keeps each path it wrote and
//! each rename it made:
//!
//! - a `file_write` to a path already written this turn, with the text it
//!   holds now or the text last written there, does not run and asks
//!   nothing: the model reads that it is already written;
//! - after [`WRITES_PER_PATH`] `file_write` calls to one path, more are
//!   refused with the same steer: answer now;
//! - a `file_rename` this turn already made (its source gone, its target
//!   there) does not run again.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// `file_write` calls to one path in a turn before more are refused.
pub const WRITES_PER_PATH: u32 = 2;

/// One path written this turn, kept in the checkpoint so "Continue"
/// counts on from it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteMark {
    /// The full path.
    pub path: String,
    /// The path as the model named it.
    pub shown: String,
    /// The model's own `file_write` calls to it that ran (a `file_edit`
    /// or a call inside a script is not counted).
    pub writes: u32,
    /// The text last written, hashed ([`text_hash`]).
    pub hash: u64,
    /// Written by a code-mode script.
    #[serde(default)]
    pub in_script: bool,
}

#[derive(Clone, Debug)]
struct Renamed {
    from: PathBuf,
    to: PathBuf,
    from_shown: String,
    to_shown: String,
    in_script: bool,
}

/// Why a call does not run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skip {
    /// It is already done (the same text, the same rename): the call
    /// counts as done. `false`: too many writes to one path, refused.
    pub done: bool,
    /// What the model reads.
    pub line: String,
    /// The path it is about (the full path), for [`Redo::ignored`].
    pub key: String,
    /// The turn's answer when the model calls it again after the line
    /// (it ignored "answer now"): Tern ends the turn with it.
    pub end: String,
}

/// The turn's changes so far.
#[derive(Clone, Debug, Default)]
pub struct Redo {
    writes: BTreeMap<String, WriteMark>,
    renames: Vec<Renamed>,
    /// Skips in a row for a path since it was last written.
    ignored: BTreeMap<String, u32>,
}

/// A stable hash of a file's text, line endings and trailing space aside
/// (FNV-1a, so a checkpoint reads the same in the next build).
pub fn text_hash(text: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in norm(text).bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

fn norm(text: &str) -> String {
    text.replace("\r\n", "\n").trim_end().to_owned()
}

fn key(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

impl Redo {
    /// From a checkpoint's marks.
    pub fn from_marks(marks: Vec<WriteMark>) -> Self {
        Self {
            writes: marks.into_iter().map(|m| (m.path.clone(), m)).collect(),
            ..Self::default()
        }
    }

    /// The marks, for the checkpoint.
    pub fn marks(&self) -> Vec<WriteMark> {
        self.writes.values().cloned().collect()
    }

    /// Before a write to `full` runs: whether it is skipped. `on_disk` is
    /// the file's text now; `counted` is false for a `file_edit` and a
    /// call inside a script.
    pub fn check_write(
        &self,
        full: &Path,
        shown: &str,
        text: &str,
        on_disk: Option<&str>,
        counted: bool,
    ) -> Option<Skip> {
        let m = self.writes.get(&key(full))?;
        let same = text_hash(text) == m.hash || on_disk.is_some_and(|d| norm(d) == norm(text));
        let line = if same {
            let moved = self.renames.iter().rev().find(|r| r.from == full);
            same_line(shown, m.in_script, moved)
        } else if counted && m.writes >= WRITES_PER_PATH {
            limit_line(shown, m.writes)
        } else {
            return None;
        };
        Some(Skip {
            done: same,
            line,
            key: key(full),
            end: format!(
                "{shown} is already written. The model kept writing it again after Tern told it so, so Tern ended the turn here."
            ),
        })
    }

    /// One more skip for the path of `skip`, in a row since it was last
    /// written: the model was told once, so the second ends the turn.
    pub fn ignored(&mut self, skip: &Skip) -> u32 {
        let n = self.ignored.entry(skip.key.clone()).or_default();
        *n += 1;
        *n
    }

    /// Before a rename runs: skipped when this turn made it already and
    /// the folder still shows it (`from` gone, `to` there).
    pub fn check_rename(&self, from: &Path, to: &Path) -> Option<Skip> {
        let r = self
            .renames
            .iter()
            .rev()
            .find(|r| r.from == from && r.to == to)?;
        (!from.exists() && to.exists()).then(|| Skip {
            done: true,
            line: format!(
                "{} was already renamed to {} this turn{}: {} is gone and {} is there. Tern did not rename it again and asked you nothing. Do not call file_rename for it again; answer the user now.",
                r.from_shown,
                r.to_shown,
                if r.in_script { " (inside your script)" } else { "" },
                r.from_shown,
                r.to_shown,
            ),
            key: format!("rename:{}", key(from)),
            end: format!(
                "{} is already renamed to {}. The model kept renaming it again after Tern told it so, so Tern ended the turn here.",
                r.from_shown, r.to_shown
            ),
        })
    }

    /// A write that ran.
    pub fn wrote(&mut self, full: &Path, shown: &str, text: &str, counted: bool, in_script: bool) {
        self.ignored.remove(&key(full));
        let m = self.writes.entry(key(full)).or_insert_with(|| WriteMark {
            path: key(full),
            ..WriteMark::default()
        });
        m.shown = shown.to_owned();
        m.hash = text_hash(text);
        m.in_script = in_script;
        if counted {
            m.writes += 1;
        }
    }

    /// A rename that ran: the text written to `from` is now at `to`.
    pub fn renamed(
        &mut self,
        from: &Path,
        to: &Path,
        from_shown: &str,
        to_shown: &str,
        in_script: bool,
    ) {
        if let Some(m) = self.writes.get(&key(from)).cloned() {
            let t = self.writes.entry(key(to)).or_insert_with(|| WriteMark {
                path: key(to),
                ..WriteMark::default()
            });
            t.shown = to_shown.to_owned();
            t.hash = m.hash;
            t.in_script = in_script;
        }
        self.renames.push(Renamed {
            from: from.to_path_buf(),
            to: to.to_path_buf(),
            from_shown: from_shown.to_owned(),
            to_shown: to_shown.to_owned(),
            in_script,
        });
    }
}

fn same_line(shown: &str, in_script: bool, moved: Option<&Renamed>) -> String {
    let script = if in_script {
        " (inside your script)"
    } else {
        ""
    };
    match moved {
        Some(r) => format!(
            "{shown} was already written with that content this turn{script} and then renamed to {}. Tern did not write it again and asked you nothing. Do not call file_write for it again; answer the user now.",
            r.to_shown
        ),
        None => format!(
            "{shown} is already written with that content{script}; Tern did not write it again and asked you nothing. Do not call file_write for it again; answer the user now."
        ),
    }
}

fn limit_line(shown: &str, writes: u32) -> String {
    format!(
        "{shown} was already written {writes} times this turn, so Tern did not write it again. Do not call file_write for {shown} again: answer the user now and say what it holds. If nothing matched (an empty folder, no such files), say so plainly, for example \"No matching files here, so {shown} lists none.\""
    )
}

/// The line a code-mode script's result ends with when its calls changed
/// the folder (#82: the model made the same calls again directly).
pub fn script_done_line(actions: &[String]) -> Option<String> {
    (!actions.is_empty()).then(|| {
        format!(
            "Done inside the script: {}. These are finished: do not call the tools again for them.",
            actions.join(", ")
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_write_of_the_same_text_is_done_already() {
        let mut r = Redo::default();
        let p = Path::new("/w/counts.md");
        assert_eq!(r.check_write(p, "counts.md", "a\n", None, true), None);
        r.wrote(p, "counts.md", "a\n", true, false);
        let s = r
            .check_write(p, "counts.md", "a\r\n", Some("a\n"), true)
            .unwrap();
        assert!(s.done);
        assert!(
            s.line
                .starts_with("counts.md is already written with that content;"),
            "{s:?}"
        );
        // Told once; the second skip in a row ends the turn, and a write
        // that runs starts the count again.
        assert_eq!(r.ignored(&s), 1);
        assert_eq!(r.ignored(&s), 2);
        r.wrote(p, "counts.md", "b\n", true, false);
        assert_eq!(r.ignored(&s), 1);
        assert!(s.end.starts_with("counts.md is already written."));
    }

    #[test]
    fn the_third_write_to_a_path_is_refused_but_edits_are_not_counted() {
        let mut r = Redo::default();
        let p = Path::new("/w/counts.md");
        r.wrote(p, "counts.md", "a", true, false);
        assert_eq!(r.check_write(p, "counts.md", "b", Some("a"), true), None);
        r.wrote(p, "counts.md", "b", true, false);
        let s = r.check_write(p, "counts.md", "c", Some("b"), true).unwrap();
        assert!(!s.done);
        assert!(s.line.contains("already written 2 times"), "{s:?}");
        assert_eq!(r.check_write(p, "counts.md", "c", Some("b"), false), None);
    }

    #[test]
    fn a_write_renamed_away_is_still_done_and_its_rename_is_said() {
        let mut r = Redo::default();
        let (a, b) = (Path::new("/w/draft.md"), Path::new("/w/hello.md"));
        r.wrote(a, "draft.md", "hi", true, true);
        r.renamed(a, b, "draft.md", "hello.md", true);
        let s = r.check_write(a, "draft.md", "hi", None, true).unwrap();
        assert!(s.line.contains("renamed to hello.md"), "{s:?}");
        assert!(s.line.contains("inside your script"), "{s:?}");
        // The text moved with it.
        assert!(
            r.check_write(b, "hello.md", "hi", Some("hi"), true)
                .unwrap()
                .done
        );
    }

    #[test]
    fn marks_survive_a_checkpoint() {
        let mut r = Redo::default();
        let p = Path::new("/w/c.md");
        r.wrote(p, "c.md", "x", true, false);
        r.wrote(p, "c.md", "y", true, false);
        let back = Redo::from_marks(r.marks());
        assert!(!back.check_write(p, "c.md", "z", None, true).unwrap().done);
        assert_eq!(text_hash("x\r\n"), text_hash("x"));
    }

    #[test]
    fn the_script_line_lists_what_it_did() {
        assert_eq!(script_done_line(&[]), None);
        assert_eq!(
            script_done_line(&[
                "wrote draft.md".into(),
                "renamed draft.md -> hello.md".into()
            ])
            .unwrap(),
            "Done inside the script: wrote draft.md, renamed draft.md -> hello.md. These are finished: do not call the tools again for them."
        );
    }
}
