//! The vault: a folder of Markdown notes the user and the agent share
//! (`docs/01-product-spec.md`, editor and vault). Plain files, so Obsidian
//! or any editor can open the same folder.
//!
//! | Rule | Why |
//! | --- | --- |
//! | Only `.md` files, inside the vault folder | The agent cannot reach other files through a note path |
//! | Writes go to a temp file, then rename | A crash never leaves half a note |
//! | `[[Note]]` and `[[Note\|label]]` are links | Same as Obsidian; backlinks come from them |
//! | Hidden folders (`.obsidian`, `.git`) are skipped | Settings are not notes |

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use serde_json::{Value, json};
use tern_core::EffectClass;
use tern_platform::AppPaths;

use crate::tools::{BoxFuture, Tool, ToolError, ToolOutput};

/// Longest note a read returns to the model, in characters.
const READ_CHARS: usize = 40_000;
/// Most hits a search returns.
const MAX_HITS: usize = 20;

/// One note, by its path inside the vault (`/`-separated, with `.md`).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Note {
    pub path: String,
}

impl Note {
    /// The note's name: file name without `.md`.
    pub fn name(&self) -> &str {
        let file = self.path.rsplit('/').next().unwrap_or(&self.path);
        file.strip_suffix(".md").unwrap_or(file)
    }

    /// The folder it sits in, `""` at the top.
    pub fn folder(&self) -> &str {
        self.path.rsplit_once('/').map(|(f, _)| f).unwrap_or("")
    }
}

/// A search hit: the note, the line number (from 1), and the line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub note: Note,
    pub line: usize,
    pub text: String,
}

/// A vault folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Vault {
    root: PathBuf,
}

impl Vault {
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The user's vault, created when missing.
    pub fn open_default() -> Result<Self, String> {
        let v = Self::at(default_root(&crate::Preferences::load())?);
        v.ensure()?;
        Ok(v)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Creates the folder, with a welcome note when it is new.
    pub fn ensure(&self) -> Result<(), String> {
        if self.root.is_dir() {
            return Ok(());
        }
        std::fs::create_dir_all(&self.root).map_err(|e| e.to_string())?;
        self.write(
            "Welcome.md",
            "# Welcome to your vault\n\nNotes here are plain Markdown files. Link notes with [[Note name]].\nTern can read and search them, and asks before it writes one.\n",
        )
    }

    /// Turns a note path into a file path, refusing anything outside the
    /// vault or not ending in `.md`.
    pub fn resolve(&self, note: &str) -> Result<PathBuf, String> {
        let mut rel = note
            .trim()
            .trim_start_matches(['/', '\\'])
            .replace('\\', "/");
        if rel.is_empty() {
            return Err("A note needs a name.".into());
        }
        if !rel.to_ascii_lowercase().ends_with(".md") {
            rel.push_str(".md");
        }
        let path = Path::new(&rel);
        if path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        {
            return Err(format!("{note} is not a path inside the vault."));
        }
        Ok(self.root.join(path))
    }

    /// Every note, sorted by path.
    pub fn list(&self) -> Vec<Note> {
        let mut out = Vec::new();
        walk(&self.root, "", &mut out);
        out.sort();
        out
    }

    pub fn read(&self, note: &str) -> Result<String, String> {
        let path = self.resolve(note)?;
        std::fs::read_to_string(&path).map_err(|e| format!("{note}: {e}"))
    }

    /// Writes a note through a temp file and a rename.
    pub fn write(&self, note: &str, text: &str) -> Result<(), String> {
        let path = self.resolve(note)?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let tmp = path.with_extension("md.tmp");
        std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
    }

    /// A note path that is free: `Untitled.md`, `Untitled 2.md`, …
    pub fn free_name(&self, base: &str) -> String {
        let base = clean_name(base);
        let mut n = 1;
        loop {
            let name = if n == 1 {
                format!("{base}.md")
            } else {
                format!("{base} {n}.md")
            };
            if self.resolve(&name).is_ok_and(|p| !p.exists()) {
                return name;
            }
            n += 1;
        }
    }

    /// Creates a new note with `text` under a free name. Returns its path.
    pub fn create(&self, base: &str, text: &str) -> Result<String, String> {
        let name = self.free_name(base);
        self.write(&name, text)?;
        Ok(name)
    }

    /// Renames a note; links to it are left alone (as Obsidian does with
    /// its update-links setting off).
    pub fn rename(&self, from: &str, to: &str) -> Result<String, String> {
        let (a, b) = (self.resolve(from)?, self.resolve(to)?);
        if b.exists() {
            return Err(format!("{to} already exists."));
        }
        if let Some(dir) = b.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::rename(a, &b).map_err(|e| e.to_string())?;
        Ok(rel(&self.root, &b))
    }

    /// Lines that contain every word of `query` (case-insensitive), and
    /// notes whose name matches, best first.
    pub fn search(&self, query: &str) -> Vec<Hit> {
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        if words.is_empty() {
            return Vec::new();
        }
        let mut hits = Vec::new();
        for note in self.list() {
            let name = note.name().to_lowercase();
            if words.iter().all(|w| name.contains(w)) {
                hits.push(Hit {
                    note: note.clone(),
                    line: 0,
                    text: format!("(note name) {}", note.name()),
                });
            }
            let Ok(text) = self.read(&note.path) else {
                continue;
            };
            for (i, line) in text.lines().enumerate() {
                let l = line.to_lowercase();
                if words.iter().all(|w| l.contains(w)) {
                    hits.push(Hit {
                        note: note.clone(),
                        line: i + 1,
                        text: line.trim().chars().take(200).collect(),
                    });
                }
            }
            if hits.len() >= MAX_HITS {
                break;
            }
        }
        hits.truncate(MAX_HITS);
        hits
    }

    /// The note a `[[link]]` points at: an exact path, else the first note
    /// with that name.
    pub fn find(&self, link: &str) -> Option<Note> {
        let notes = self.list();
        let want = link.trim().trim_end_matches(".md");
        notes
            .iter()
            .find(|n| n.path.trim_end_matches(".md").eq_ignore_ascii_case(want))
            .or_else(|| notes.iter().find(|n| n.name().eq_ignore_ascii_case(want)))
            .cloned()
    }

    /// Notes that link to `note`.
    pub fn backlinks(&self, note: &Note) -> Vec<Note> {
        self.list()
            .into_iter()
            .filter(|n| n != note)
            .filter(|n| {
                self.read(&n.path).is_ok_and(|t| {
                    links(&t).iter().any(|l| {
                        let l = l.trim_end_matches(".md");
                        l.eq_ignore_ascii_case(note.name())
                            || l.eq_ignore_ascii_case(note.path.trim_end_matches(".md"))
                    })
                })
            })
            .collect()
    }
}

/// The vault folder: the one in preferences, else `<data>/vault`.
pub fn default_root(prefs: &crate::Preferences) -> Result<PathBuf, String> {
    match &prefs.vault {
        Some(p) => Ok(PathBuf::from(p)),
        None => Ok(AppPaths::discover()
            .map_err(|e| e.to_string())?
            .data_dir
            .join("vault")),
    }
}

fn walk(dir: &Path, prefix: &str, out: &mut Vec<Note>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let Ok(kind) = e.file_type() else { continue };
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        if kind.is_dir() {
            walk(&e.path(), &path, out);
        } else if kind.is_file() && name.to_ascii_lowercase().ends_with(".md") {
            out.push(Note { path });
        }
    }
}

fn rel(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// A note name safe on every OS: no path or reserved characters.
pub fn clean_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if "\\/:*?\"<>|#^[]".contains(c) {
                ' '
            } else {
                c
            }
        })
        .collect();
    let words: Vec<&str> = cleaned.split_whitespace().collect();
    let mut out: String = words.join(" ").chars().take(80).collect();
    out = out.trim_end_matches('.').trim().to_owned();
    if out.is_empty() {
        "Untitled".into()
    } else {
        out
    }
}

/// The targets of every `[[link]]` in a note, without labels or headings.
pub fn links(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find("[[") {
        rest = &rest[i + 2..];
        let Some(j) = rest.find("]]") else { break };
        let inner = &rest[..j];
        let target = inner.split(['|', '#']).next().unwrap_or("").trim();
        if !target.is_empty() && !target.contains('\n') {
            out.push(target.to_owned());
        }
        rest = &rest[j + 2..];
    }
    out
}

/// An answer saved as a note: a title line, the answer, then its sources.
pub fn answer_note(question: &str, answer: &str, sources: &[(String, String)]) -> (String, String) {
    let title = clean_name(question);
    let mut text = format!("# {title}\n\n{}\n", answer.trim());
    if !sources.is_empty() {
        text.push_str("\n## Sources\n\n");
        for (t, u) in sources {
            text.push_str(&format!("- [{t}]({u})\n"));
        }
    }
    (title, text)
}

/// `vault_search`: find notes by words.
#[derive(Debug, Clone)]
pub struct VaultSearchTool(pub Arc<Vault>);

impl Tool for VaultSearchTool {
    fn name(&self) -> &'static str {
        "vault_search"
    }
    fn summary(&self) -> &'static str {
        "Search the user's notes vault. Returns matching lines with note paths; read a note before quoting it."
    }
    fn effect(&self) -> EffectClass {
        EffectClass::Read
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object", "required": ["query"], "additionalProperties": false,
               "properties": {"query": {"type": "string"}}})
    }
    fn call<'a>(&'a self, args: Value) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let q = args["query"].as_str().unwrap_or_default().to_owned();
            let v = self.0.clone();
            let hits = tokio::task::spawn_blocking(move || v.search(&q))
                .await
                .map_err(|e| ToolError::Failed(e.to_string()))?;
            if hits.is_empty() {
                return Ok(ToolOutput::text("No notes match."));
            }
            let out: Vec<String> = hits
                .iter()
                .map(|h| format!("{}:{}: {}", h.note.path, h.line, h.text))
                .collect();
            Ok(ToolOutput::text(out.join("\n")))
        })
    }
}

/// `vault_read`: read one note.
#[derive(Debug, Clone)]
pub struct VaultReadTool(pub Arc<Vault>);

impl Tool for VaultReadTool {
    fn name(&self) -> &'static str {
        "vault_read"
    }
    fn summary(&self) -> &'static str {
        "Read one note from the user's vault by its path (from vault_search) or name."
    }
    fn effect(&self) -> EffectClass {
        EffectClass::Read
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object", "required": ["note"], "additionalProperties": false,
               "properties": {"note": {"type": "string"}}})
    }
    fn target(&self, args: &Value) -> Option<String> {
        args["note"].as_str().map(str::to_owned)
    }
    fn call<'a>(&'a self, args: Value) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let want = args["note"].as_str().unwrap_or_default();
            let path = self
                .0
                .find(want)
                .map(|n| n.path)
                .unwrap_or_else(|| want.to_owned());
            let text = self.0.read(&path).map_err(ToolError::Failed)?;
            let body: String = text.chars().take(READ_CHARS).collect();
            Ok(ToolOutput {
                meta: json!({ "note": path }),
                ..ToolOutput::text(format!("--- {path} ---\n{body}"))
            })
        })
    }
}

/// `vault_write`: create or replace a note. Needs the user's approval.
#[derive(Debug, Clone)]
pub struct VaultWriteTool(pub Arc<Vault>);

impl Tool for VaultWriteTool {
    fn name(&self) -> &'static str {
        "vault_write"
    }
    fn summary(&self) -> &'static str {
        "Create or replace a Markdown note in the user's vault. The user approves each write."
    }
    fn effect(&self) -> EffectClass {
        EffectClass::Write
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object", "required": ["note", "text"], "additionalProperties": false,
               "properties": {"note": {"type": "string"}, "text": {"type": "string"}}})
    }
    fn target(&self, args: &Value) -> Option<String> {
        args["note"].as_str().map(str::to_owned)
    }
    fn call<'a>(&'a self, args: Value) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let note = args["note"].as_str().unwrap_or_default();
            let text = args["text"].as_str().unwrap_or_default();
            self.0.write(note, text).map_err(ToolError::Failed)?;
            Ok(ToolOutput {
                meta: json!({ "note": note }),
                ..ToolOutput::text(format!("Saved {note}."))
            })
        })
    }
}

/// Adds the three vault tools to a registry.
pub fn register(tools: &mut crate::ToolRegistry, vault: Arc<Vault>) {
    tools.register(VaultSearchTool(vault.clone()));
    tools.register(VaultReadTool(vault.clone()));
    tools.register(VaultWriteTool(vault));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault() -> (tempfile::TempDir, Vault) {
        let dir = tempfile::tempdir().unwrap();
        let v = Vault::at(dir.path().join("vault"));
        v.ensure().unwrap();
        (dir, v)
    }

    #[test]
    fn notes_stay_inside_the_vault() {
        let (_d, v) = vault();
        assert!(v.resolve("../secret").is_err());
        assert!(v.resolve("/etc/passwd").unwrap().starts_with(v.root()));
        assert!(v.resolve("a/../../b").is_err());
        assert!(v.resolve("").is_err());
        assert!(v.resolve("Topics/RAG").unwrap().ends_with("Topics/RAG.md"));
    }

    #[test]
    fn write_list_read_rename() {
        let (_d, v) = vault();
        v.write("Topics/RAG.md", "# RAG\nSee [[Transformers|the paper]].")
            .unwrap();
        v.write("Transformers", "Attention is all you need.")
            .unwrap();
        std::fs::create_dir_all(v.root().join(".obsidian")).unwrap();
        std::fs::write(v.root().join(".obsidian/x.md"), "hidden").unwrap();
        let paths: Vec<_> = v.list().into_iter().map(|n| n.path).collect();
        assert_eq!(paths, ["Topics/RAG.md", "Transformers.md", "Welcome.md"]);
        assert_eq!(
            v.read("Transformers").unwrap(),
            "Attention is all you need."
        );
        let to = v
            .rename("Transformers.md", "Papers/Transformers.md")
            .unwrap();
        assert_eq!(to, "Papers/Transformers.md");
        assert!(v.rename("Topics/RAG.md", "Welcome.md").is_err());
        assert_eq!(v.free_name("Welcome"), "Welcome 2.md");
        assert_eq!(v.create("a/b: c?", "x").unwrap(), "a b c.md");
    }

    #[test]
    fn links_backlinks_and_search() {
        let (_d, v) = vault();
        v.write(
            "RAG.md",
            "Uses [[Transformers#Attention]] and [[Vector DB|stores]].",
        )
        .unwrap();
        v.write("Transformers.md", "Self attention layers.")
            .unwrap();
        assert_eq!(links("[[A]] [[B|b]] [[C#h]] [[]] [[x"), ["A", "B", "C"]);
        let t = v.find("transformers").unwrap();
        assert_eq!(t.path, "Transformers.md");
        let back: Vec<_> = v.backlinks(&t).into_iter().map(|n| n.path).collect();
        assert_eq!(back, ["RAG.md"]);
        let hits = v.search("ATTENTION self");
        assert_eq!(hits.len(), 1);
        assert_eq!(
            (hits[0].note.path.as_str(), hits[0].line),
            ("Transformers.md", 1)
        );
        assert!(v.search("rag").iter().any(|h| h.line == 0));
        assert!(v.search("  ").is_empty());
    }

    #[test]
    fn answers_become_notes() {
        let (title, text) = answer_note(
            "What is RAG?",
            "Retrieval plus generation.",
            &[("Survey".into(), "https://arxiv.org/abs/2312.10997".into())],
        );
        assert_eq!(title, "What is RAG");
        assert!(text.starts_with("# What is RAG\n\nRetrieval plus generation.\n"));
        assert!(text.contains("- [Survey](https://arxiv.org/abs/2312.10997)"));
    }

    #[tokio::test]
    async fn tools_read_search_and_write() {
        let (_d, v) = vault();
        let v = Arc::new(v);
        VaultWriteTool(v.clone())
            .call(json!({"note": "Notes/Idea", "text": "Tern uses a vault."}))
            .await
            .unwrap();
        let hit = VaultSearchTool(v.clone())
            .call(json!({"query": "vault tern"}))
            .await
            .unwrap();
        assert!(hit.content.contains("Notes/Idea.md:1:"));
        let read = VaultReadTool(v.clone())
            .call(json!({"note": "idea"}))
            .await
            .unwrap();
        assert!(read.content.contains("Tern uses a vault."));
        assert_eq!(VaultWriteTool(v).effect(), EffectClass::Write);
    }
}
