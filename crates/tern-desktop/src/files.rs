//! The Files tab (#74): what a chat made (Outputs) and the tree of its
//! folder. GPUI-free, so it is unit tested; `view/files_view.rs` draws it.
//!
//! - Outputs are the chat's saves (block exports, Save as…) and its
//!   `file_write` calls (`tern_agent::chat::outputs_in`), newest first,
//!   with their size and time read from disk when shown.
//! - The tree lists one folder level at a time: a folder's entries are
//!   read when it is expanded, folders first then by name (without case).
//!   `.git`, `node_modules` and `target` (and a few other build and cache
//!   folders) are left out unless "Show hidden folders" is on. A folder
//!   with more than [`MAX_ENTRIES`] entries lists the first ones and says
//!   how many it left out.
//! - [`FolderTree::signature`] is the modification times of the folders on
//!   screen; the view polls it while the tab shows and reloads on change.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Folders the tree leaves out by default.
pub const IGNORED: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "__pycache__",
    ".venv",
    ".mypy_cache",
    ".pytest_cache",
    ".next",
    ".gradle",
];

/// Entries one folder lists at most.
pub const MAX_ENTRIES: usize = 2_000;

/// One entry of a folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub dir: bool,
    pub bytes: Option<u64>,
}

/// A folder as read: its entries and how many were left out.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Listing {
    pub entries: Vec<Entry>,
    /// Entries past [`MAX_ENTRIES`], not listed.
    pub more: usize,
    /// Hidden by the ignore list.
    pub ignored: usize,
    /// The folder could not be read.
    pub error: Option<String>,
}

/// One row of the tree as drawn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    Entry {
        depth: usize,
        entry: Entry,
        open: bool,
    },
    /// "N more not listed" under a folder.
    More { depth: usize, n: usize },
    /// The folder could not be read.
    Error { depth: usize, text: String },
}

/// Reads `dir`: folders first, then files, each by name without case.
pub fn read_dir(dir: &Path, show_ignored: bool) -> Listing {
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) => {
            return Listing {
                error: Some(e.to_string()),
                ..Listing::default()
            };
        }
    };
    let mut entries = Vec::new();
    let mut ignored = 0;
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let meta = e.metadata().ok();
        let dir = meta.as_ref().is_some_and(|m| m.is_dir());
        if dir && !show_ignored && IGNORED.contains(&name.as_str()) {
            ignored += 1;
            continue;
        }
        entries.push(Entry {
            bytes: meta.filter(|m| m.is_file()).map(|m| m.len()),
            path: e.path(),
            name,
            dir,
        });
    }
    entries.sort_by(|a, b| {
        b.dir
            .cmp(&a.dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
    let more = entries.len().saturating_sub(MAX_ENTRIES);
    entries.truncate(MAX_ENTRIES);
    Listing {
        entries,
        more,
        ignored,
        error: None,
    }
}

/// The folder tree of a chat's workspace.
#[derive(Clone, Debug, Default)]
pub struct FolderTree {
    pub root: Option<PathBuf>,
    /// Expanded folders (the root always is).
    pub open: BTreeSet<PathBuf>,
    pub show_ignored: bool,
    /// Folders read so far (read while drawing, hence the cell).
    listings: RefCell<BTreeMap<PathBuf, Listing>>,
    /// [`Self::signature`] when they were read.
    seen: RefCell<Vec<(PathBuf, Option<SystemTime>)>>,
}

impl FolderTree {
    /// Points the tree at `root` (a chat's folder); a new root starts
    /// collapsed, the same root keeps what was open.
    pub fn set_root(&mut self, root: Option<PathBuf>) {
        let root = root.filter(|r| r.is_dir());
        if root != self.root {
            self.root = root;
            self.open.clear();
            self.reload();
        }
    }

    /// Expands or collapses a folder.
    pub fn toggle(&mut self, dir: &Path) {
        if !self.open.remove(dir) {
            self.open.insert(dir.to_path_buf());
        }
    }

    pub fn set_show_ignored(&mut self, on: bool) {
        if self.show_ignored != on {
            self.show_ignored = on;
            self.reload();
        }
    }

    /// Reads every folder on screen again (after a save, or when
    /// [`Self::changed`] says so).
    pub fn reload(&self) {
        self.listings.borrow_mut().clear();
        self.seen.borrow_mut().clear();
    }

    /// The folders whose entries are on screen: the root and each open
    /// folder under an open parent.
    fn shown_dirs(&self) -> Vec<PathBuf> {
        let Some(root) = &self.root else {
            return Vec::new();
        };
        let mut out = vec![root.clone()];
        for d in &self.open {
            if d != root
                && d.starts_with(root)
                && d.ancestors()
                    .skip(1)
                    .take_while(|a| *a != root.as_path())
                    .all(|a| self.open.contains(a))
            {
                out.push(d.clone());
            }
        }
        out
    }

    /// The modification time of each folder on screen.
    pub fn signature(&self) -> Vec<(PathBuf, Option<SystemTime>)> {
        self.shown_dirs()
            .into_iter()
            .map(|d| {
                let t = std::fs::metadata(&d).and_then(|m| m.modified()).ok();
                (d, t)
            })
            .collect()
    }

    /// Whether a folder on screen changed since it was read; reloads the
    /// tree when so.
    pub fn changed(&self) -> bool {
        if self.root.is_none() {
            return false;
        }
        let now = self.signature();
        let seen = self.seen.borrow().clone();
        if !seen.is_empty() && now != seen {
            self.reload();
            return true;
        }
        false
    }

    /// The rows to draw, reading folders not read yet.
    pub fn rows(&self) -> Vec<Row> {
        let Some(root) = self.root.clone() else {
            return Vec::new();
        };
        if self.seen.borrow().is_empty() {
            *self.seen.borrow_mut() = self.signature();
        }
        let mut out = Vec::new();
        self.push_dir(&root, 0, &mut out);
        out
    }

    fn push_dir(&self, dir: &Path, depth: usize, out: &mut Vec<Row>) {
        if depth > 32 {
            return;
        }
        let show = self.show_ignored;
        let listing = self
            .listings
            .borrow_mut()
            .entry(dir.to_path_buf())
            .or_insert_with(|| read_dir(dir, show))
            .clone();
        if let Some(e) = listing.error {
            out.push(Row::Error { depth, text: e });
            return;
        }
        for e in listing.entries {
            let open = e.dir && self.open.contains(&e.path);
            let path = e.path.clone();
            out.push(Row::Entry {
                depth,
                entry: e,
                open,
            });
            if open {
                self.push_dir(&path, depth + 1, out);
            }
        }
        if listing.more > 0 {
            out.push(Row::More {
                depth,
                n: listing.more,
            });
        }
    }

    /// How many folders the root's listing left out by the ignore list.
    pub fn ignored_at_root(&self) -> usize {
        self.root
            .as_ref()
            .and_then(|r| self.listings.borrow().get(r).cloned())
            .map_or(0, |l| l.ignored)
    }
}

/// A file size as the list shows it: `812 B`, `14 KB`, `3.2 MB`.
pub fn size_label(bytes: u64) -> String {
    const K: f64 = 1024.;
    let b = bytes as f64;
    if b < K {
        format!("{bytes} B")
    } else if b < K * K {
        format!("{:.0} KB", (b / K).max(1.))
    } else if b < K * K * K {
        format!("{:.1} MB", b / K / K)
    } else {
        format!("{:.1} GB", b / K / K / K)
    }
}

/// An age in seconds, short: `now`, `5m`, `3h`, `2d`, else in weeks.
pub fn ago(secs: i64) -> String {
    let s = secs.max(0);
    match s {
        0..=59 => "now".into(),
        60..=3599 => format!("{}m", s / 60),
        3600..=86_399 => format!("{}h", s / 3600),
        86_400..=2_591_999 => format!("{}d", s / 86_400),
        _ => format!("{}w", s / 604_800),
    }
}

/// A path shown short: inside `root` relative to it, else the full path.
pub fn short_path(path: &Path, root: Option<&Path>) -> String {
    match root.and_then(|r| path.strip_prefix(r).ok()) {
        Some(rel) if !rel.as_os_str().is_empty() => rel.display().to_string(),
        _ => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> (PathBuf, FolderTree) {
        let base = std::env::temp_dir().join(format!(
            "tern files test {}",
            std::process::id() as u64 * 1000
                + SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap()
                    .subsec_nanos() as u64
                    % 1000
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("src/inner")).unwrap();
        std::fs::create_dir_all(base.join("node_modules/x")).unwrap();
        std::fs::create_dir_all(base.join(".git")).unwrap();
        std::fs::write(base.join("b.txt"), "hello").unwrap();
        std::fs::write(base.join("A.md"), "x").unwrap();
        std::fs::write(base.join("src/main.rs"), "fn main(){}").unwrap();
        let mut t = FolderTree::default();
        t.set_root(Some(base.clone()));
        (base, t)
    }

    fn names(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|r| match r {
                Row::Entry { depth, entry, open } => format!(
                    "{}{}{}",
                    "  ".repeat(*depth),
                    entry.name,
                    if entry.dir {
                        if *open { "/-" } else { "/+" }
                    } else {
                        ""
                    }
                ),
                Row::More { n, .. } => format!("+{n}"),
                Row::Error { text, .. } => format!("!{text}"),
            })
            .collect()
    }

    #[test]
    fn folders_first_ignored_left_out_and_expanded_lazily() {
        let (base, mut t) = tree();
        assert_eq!(names(&t.rows()), ["src/+", "A.md", "b.txt"]);
        assert_eq!(t.ignored_at_root(), 2);
        t.toggle(&base.join("src"));
        assert_eq!(
            names(&t.rows()),
            ["src/-", "  inner/+", "  main.rs", "A.md", "b.txt"]
        );
        t.set_show_ignored(true);
        assert_eq!(
            names(&t.rows()),
            [
                ".git/+",
                "node_modules/+",
                "src/-",
                "  inner/+",
                "  main.rs",
                "A.md",
                "b.txt"
            ]
        );
        // A collapsed parent hides an open child.
        t.toggle(&base.join("src/inner"));
        t.toggle(&base.join("src"));
        assert_eq!(t.shown_dirs(), vec![base.clone()]);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_new_file_is_seen_on_the_next_check() {
        let (base, t) = tree();
        let before = t.rows().len();
        assert!(!t.changed());
        // Folder times have whole-second resolution on some systems.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        std::fs::write(base.join("new.csv"), "a,b").unwrap();
        assert!(t.changed());
        assert_eq!(t.rows().len(), before + 1);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn labels() {
        assert_eq!(size_label(812), "812 B");
        assert_eq!(size_label(14 * 1024 + 100), "14 KB");
        assert_eq!(size_label(3_355_443), "3.2 MB");
        assert_eq!(ago(0), "now");
        assert_eq!(ago(-5), "now");
        assert_eq!(ago(300), "5m");
        assert_eq!(ago(3 * 3600), "3h");
        assert_eq!(ago(3 * 86_400), "3d");
        assert_eq!(
            short_path(Path::new("/w/notes/a.md"), Some(Path::new("/w"))),
            "notes/a.md"
        );
        assert_eq!(
            short_path(Path::new("/elsewhere/t.csv"), Some(Path::new("/w"))),
            "/elsewhere/t.csv"
        );
    }
}
