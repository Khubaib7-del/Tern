//! Live steering: notes the user sends while a run is going.
//!
//! A front end holds a [`Steering`] next to the run's cancel flag and calls
//! [`Steering::send`] from any thread. The run picks notes up at its next
//! safe point, logs `steer.received`, applies them, and logs `steer.applied`
//! with what it did:
//!
//! | Run | Safe points | What a note can do |
//! | --- | --- | --- |
//! | Quick turn | Before each model call | Guidance the model reads from then on |
//! | Deep research | Before workers start, while they run, after each worker round, before the answer | Guidance for notes and the answer; drop, add, or redirect a worker ([`tern_research::steer`]) |
//!
//! A note that arrives while the answer is being written is too late for
//! this run; it is logged as received and not applied.
//!
//! A chat's steering can also come from another process (`tern steer`):
//! notes appended to `<data>/steer/<chat>.txt`, one per line, are picked up
//! with the rest and the file removed.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tern_platform::AppPaths;

pub use tern_research::steer::{SteerAction, parse_steer};

/// One note from the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SteerNote {
    pub text: String,
}

/// Where steering notes wait for the run. Cheap to clone; every clone is
/// the same inbox.
#[derive(Clone, Debug, Default)]
pub struct Steering {
    queue: Arc<Mutex<VecDeque<SteerNote>>>,
    /// A file other processes append notes to.
    inbox: Option<Arc<PathBuf>>,
}

/// The file `tern steer` writes a chat's notes to.
pub fn inbox_path(paths: &AppPaths, session: &str) -> PathBuf {
    paths.data_dir.join("steer").join(format!("{session}.txt"))
}

/// Appends a note to a chat's steering file, for a run in another process.
pub fn send_to_inbox(path: &Path, text: &str) -> Result<(), String> {
    use std::io::Write;
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() {
        return Err("A steering note needs some text.".into());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| format!("Tern could not open {}: {e}", path.display()))?;
    writeln!(f, "{text}").map_err(|e| e.to_string())
}

/// Sends a note to chat `session`'s run from another process
/// (`tern steer`). Returns the file it was written to.
pub fn send_to_chat(session: &str, text: &str) -> Result<PathBuf, String> {
    let paths = AppPaths::discover().map_err(|e| e.to_string())?;
    let path = inbox_path(&paths, session);
    send_to_inbox(&path, text)?;
    Ok(path)
}

impl Steering {
    pub fn new() -> Self {
        Self::default()
    }

    /// An inbox that also reads notes appended to `path`.
    pub fn with_file(path: PathBuf) -> Self {
        Self {
            queue: Arc::default(),
            inbox: Some(Arc::new(path)),
        }
    }

    /// The file this inbox reads, if any.
    pub fn file(&self) -> Option<&Path> {
        self.inbox.as_deref().map(PathBuf::as_path)
    }

    /// Queues a note. Blank notes are ignored. Returns whether it was queued.
    pub fn send(&self, text: &str) -> bool {
        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if text.is_empty() {
            return false;
        }
        match self.queue.lock() {
            Ok(mut q) => {
                q.push_back(SteerNote { text });
                true
            }
            Err(_) => false,
        }
    }

    /// Takes every waiting note, oldest first (the file's after the
    /// in-process ones).
    pub fn take(&self) -> Vec<SteerNote> {
        let mut out: Vec<SteerNote> = self
            .queue
            .lock()
            .map(|mut q| q.drain(..).collect())
            .unwrap_or_default();
        if let Some(path) = self.inbox.as_deref()
            && let Ok(text) = std::fs::read_to_string(path)
        {
            let _ = std::fs::remove_file(path);
            out.extend(
                text.lines()
                    .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
                    .filter(|l| !l.is_empty())
                    .map(|text| SteerNote { text }),
            );
        }
        out
    }

    /// Notes waiting to be picked up in this process.
    pub fn pending(&self) -> usize {
        self.queue.lock().map(|q| q.len()).unwrap_or(0)
    }

    /// Drops any notes left over from an earlier run.
    pub fn clear(&self) {
        let _ = self.take();
    }
}

/// The block a quick turn's model reads for a steering note.
pub fn guidance_message(text: &str) -> String {
    format!(
        "<steer>\nThe user sent this while you were working. Follow it from here on, keeping to your earlier instructions otherwise:\n{text}\n</steer>"
    )
}

/// Lines added to a worker's notes prompt or the synthesis prompt for the
/// guidance received so far.
pub fn guidance_lines(notes: &[String]) -> String {
    if notes.is_empty() {
        return String::new();
    }
    let mut out =
        String::from("\n\nThe user steered the research while it ran. Follow this guidance:\n");
    for n in notes {
        out.push_str(&format!("- {n}\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_queue_in_order_and_blanks_are_dropped() {
        let s = Steering::new();
        let other = s.clone();
        assert!(s.send("focus on  2024 sources"));
        assert!(!other.send("   "));
        assert!(other.send("drop 3"));
        assert_eq!(s.pending(), 2);
        let got: Vec<String> = s.take().into_iter().map(|n| n.text).collect();
        assert_eq!(got, ["focus on 2024 sources", "drop 3"]);
        assert!(other.take().is_empty());
        assert!(guidance_lines(&["a".into()]).contains("- a"));
        assert!(guidance_lines(&[]).is_empty());
    }

    #[test]
    fn notes_from_another_process_arrive_through_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("steer").join("ses_1.txt");
        let s = Steering::with_file(path.clone());
        s.send("first");
        send_to_inbox(&path, "drop  2").unwrap();
        send_to_inbox(&path, "add: costs").unwrap();
        assert!(send_to_inbox(&path, "  ").is_err());
        let got: Vec<String> = s.take().into_iter().map(|n| n.text).collect();
        assert_eq!(got, ["first", "drop 2", "add: costs"]);
        assert!(!path.exists());
        assert!(s.take().is_empty());
    }
}
