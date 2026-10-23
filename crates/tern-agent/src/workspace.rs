//! A chat's workspace: a folder (or one file) the user opened for the
//! chat, and the file tools scoped to it.
//!
//! | Tool | Effect | What it does |
//! | --- | --- | --- |
//! | `files_list` | Read | Lists a folder inside the workspace (hidden entries left out) |
//! | `file_read` | Read | Reads a text file inside the workspace |
//! | `file_search` | Read | Finds lines containing words, across text files |
//! | `file_write` | External side effect (asks) | Writes a text file inside the workspace; Act mode only |
//!
//! Every path is resolved against the workspace root and checked after
//! following symlinks, so `..`, absolute paths, and links that point out
//! are refused. A one-file workspace exposes only that file.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use serde_json::{Value, json};
use tern_core::EffectClass;

use crate::tools::{BoxFuture, Tool, ToolError, ToolOutput, ToolRegistry};

/// Longest read returned to the model, in characters.
const READ_CHARS: usize = 40_000;
/// Most entries a listing returns.
const MAX_LIST: usize = 400;
/// Most hits a search returns.
const MAX_HITS: usize = 40;
/// Files larger than this are not searched or read.
const MAX_FILE_BYTES: u64 = 2_000_000;

/// The tool names, so a chat can take them out again.
pub const TOOL_NAMES: [&str; 4] = ["files_list", "file_read", "file_search", "file_write"];

/// The folder or file a chat works in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Workspace {
    /// The folder, canonical. For a one-file workspace, the file's folder.
    root: PathBuf,
    /// Set when the workspace is one file.
    only: Option<PathBuf>,
}

impl Workspace {
    /// Opens `path` (a folder or a file) as a workspace.
    pub fn open(path: &Path) -> Result<Self, String> {
        let canon = path
            .canonicalize()
            .map_err(|e| format!("Cannot open {}: {e}", path.display()))?;
        if canon.is_dir() {
            return Ok(Self {
                root: canon,
                only: None,
            });
        }
        let root = canon
            .parent()
            .map(Path::to_path_buf)
            .ok_or("That file has no folder.")?;
        Ok(Self {
            root,
            only: Some(canon),
        })
    }

    /// What the chip shows: the folder's or file's name.
    pub fn name(&self) -> String {
        let p = self.only.as_deref().unwrap_or(&self.root);
        p.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| p.display().to_string())
    }

    /// The path the user opened.
    pub fn path(&self) -> &Path {
        self.only.as_deref().unwrap_or(&self.root)
    }

    pub fn is_file(&self) -> bool {
        self.only.is_some()
    }

    /// Resolves a path the model gave (relative to the root, `/`
    /// separated) to a path inside the workspace, or says why not.
    pub fn resolve(&self, rel: &str) -> Result<PathBuf, ToolError> {
        let rel = rel.trim();
        let rel = rel.strip_prefix("./").unwrap_or(rel);
        let p = Path::new(rel);
        if p.is_absolute() || p.has_root() {
            return Err(ToolError::Failed(
                "Give a path inside the workspace, relative to its folder.".into(),
            ));
        }
        if p.components().any(|c| matches!(c, Component::ParentDir)) {
            return Err(ToolError::Failed(
                "`..` is not allowed: the workspace is the only folder these tools reach.".into(),
            ));
        }
        let joined = if rel.is_empty() || rel == "." {
            self.root.clone()
        } else {
            self.root.join(p)
        };
        // Follow links for anything that exists; a new file is checked by
        // its folder.
        let real = if joined.exists() {
            joined
                .canonicalize()
                .map_err(|e| ToolError::Failed(e.to_string()))?
        } else {
            let parent = joined
                .parent()
                .ok_or_else(|| ToolError::Failed("No folder for that path.".into()))?;
            let parent = parent
                .canonicalize()
                .map_err(|_| ToolError::Failed("That folder does not exist.".into()))?;
            parent.join(joined.file_name().unwrap_or_default())
        };
        if !real.starts_with(&self.root) {
            return Err(ToolError::Failed(
                "That path leads outside the workspace.".into(),
            ));
        }
        if let Some(only) = &self.only
            && real != *only
            && real != self.root
        {
            return Err(ToolError::Failed(format!(
                "This workspace is one file: {}.",
                self.name()
            )));
        }
        Ok(real)
    }

    fn rel(&self, p: &Path) -> String {
        p.strip_prefix(&self.root)
            .unwrap_or(p)
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/")
    }

    /// The entries of a folder in the workspace: `name/` for folders.
    pub fn list(&self, rel: &str) -> Result<Vec<String>, ToolError> {
        if let Some(only) = &self.only {
            return Ok(vec![self.rel(only)]);
        }
        let dir = self.resolve(rel)?;
        let mut out = Vec::new();
        let rd = std::fs::read_dir(&dir).map_err(|e| ToolError::Failed(e.to_string()))?;
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let path = e.path();
            let shown = self.rel(&path);
            out.push(if path.is_dir() {
                format!("{shown}/")
            } else {
                shown
            });
        }
        out.sort();
        out.truncate(MAX_LIST);
        Ok(out)
    }

    pub fn read(&self, rel: &str) -> Result<String, ToolError> {
        let p = self.resolve(rel)?;
        if p.is_dir() {
            return Err(ToolError::Failed(
                "That is a folder; list it with files_list.".into(),
            ));
        }
        let meta = std::fs::metadata(&p).map_err(|e| ToolError::Failed(e.to_string()))?;
        if meta.len() > MAX_FILE_BYTES {
            return Err(ToolError::Failed("That file is too large to read.".into()));
        }
        let bytes = std::fs::read(&p).map_err(|e| ToolError::Failed(e.to_string()))?;
        let text = String::from_utf8(bytes)
            .map_err(|_| ToolError::Failed("That file is not text.".into()))?;
        Ok(match text.char_indices().nth(READ_CHARS) {
            Some((at, _)) => format!("{}\n[cut at {READ_CHARS} characters]", &text[..at]),
            None => text,
        })
    }

    /// Lines that contain every word of `query` (case-insensitive).
    pub fn search(&self, query: &str) -> Vec<String> {
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        if words.is_empty() {
            return Vec::new();
        }
        let mut hits = Vec::new();
        let mut files = Vec::new();
        match &self.only {
            Some(f) => files.push(f.clone()),
            None => walk(&self.root, &mut files, 0),
        }
        for f in files {
            if hits.len() >= MAX_HITS {
                break;
            }
            let Ok(meta) = std::fs::metadata(&f) else {
                continue;
            };
            if meta.len() > MAX_FILE_BYTES {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&f) else {
                continue;
            };
            for (n, line) in text.lines().enumerate() {
                let l = line.to_lowercase();
                if words.iter().all(|w| l.contains(w)) {
                    let line: String = line.trim().chars().take(200).collect();
                    hits.push(format!("{}:{}: {}", self.rel(&f), n + 1, line));
                    if hits.len() >= MAX_HITS {
                        break;
                    }
                }
            }
        }
        hits
    }

    pub fn write(&self, rel: &str, text: &str) -> Result<String, ToolError> {
        let p = self.resolve(rel)?;
        if p.is_dir() {
            return Err(ToolError::Failed("That is a folder.".into()));
        }
        let tmp = p.with_extension("tern-tmp");
        std::fs::write(&tmp, text).map_err(|e| ToolError::Failed(e.to_string()))?;
        std::fs::rename(&tmp, &p).map_err(|e| ToolError::Failed(e.to_string()))?;
        Ok(self.rel(&p))
    }

    /// Adds the file tools for this workspace to `tools`.
    pub fn register(self: &Arc<Self>, tools: &mut ToolRegistry) {
        tools.register(FilesList(self.clone()));
        tools.register(FileRead(self.clone()));
        tools.register(FileSearch(self.clone()));
        tools.register(FileWrite(self.clone()));
    }

    /// Takes the file tools out.
    pub fn unregister(tools: &mut ToolRegistry) {
        for n in TOOL_NAMES {
            tools.unregister(n);
        }
    }
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    if depth > 8 || out.len() > 5_000 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || name == "node_modules" || name == "target" {
            continue;
        }
        // Links are not followed while walking.
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            walk(&e.path(), out, depth + 1);
        } else {
            out.push(e.path());
        }
    }
}

fn path_arg(args: &Value) -> String {
    args["path"].as_str().unwrap_or_default().to_owned()
}

macro_rules! tool {
    ($ty:ident, $name:literal, $summary:literal, $effect:expr, $schema:expr, |$ws:ident, $args:ident| $body:expr) => {
        #[derive(Debug, Clone)]
        struct $ty(Arc<Workspace>);

        impl Tool for $ty {
            fn name(&self) -> &'static str {
                $name
            }
            fn summary(&self) -> &'static str {
                $summary
            }
            fn effect(&self) -> EffectClass {
                $effect
            }
            fn input_schema(&self) -> Value {
                $schema
            }
            fn target(&self, args: &Value) -> Option<String> {
                Some(format!("{}/{}", self.0.name(), path_arg(args)))
            }
            fn call<'a>(&'a self, $args: Value) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
                let $ws = self.0.clone();
                Box::pin(async move {
                    tokio::task::spawn_blocking(move || $body)
                        .await
                        .map_err(|e| ToolError::Failed(e.to_string()))?
                })
            }
        }
    };
}

tool!(
    FilesList,
    "files_list",
    "List a folder in the chat's workspace (path relative to it; empty for the top).",
    EffectClass::Read,
    json!({"type": "object", "additionalProperties": false,
           "properties": {"path": {"type": "string"}}}),
    |ws, args| {
        let rows = ws.list(&path_arg(&args))?;
        Ok(ToolOutput::text(if rows.is_empty() {
            "The folder is empty.".to_owned()
        } else {
            rows.join("\n")
        }))
    }
);

tool!(
    FileRead,
    "file_read",
    "Read a text file in the chat's workspace by its path (from files_list or file_search).",
    EffectClass::Read,
    json!({"type": "object", "required": ["path"], "additionalProperties": false,
           "properties": {"path": {"type": "string"}}}),
    |ws, args| ws.read(&path_arg(&args)).map(ToolOutput::text)
);

tool!(
    FileSearch,
    "file_search",
    "Find lines containing all the given words across the workspace's text files. Returns path:line: text.",
    EffectClass::Read,
    json!({"type": "object", "required": ["query"], "additionalProperties": false,
           "properties": {"query": {"type": "string"}}}),
    |ws, args| {
        let hits = ws.search(args["query"].as_str().unwrap_or_default());
        Ok(ToolOutput::text(if hits.is_empty() {
            "No lines match.".to_owned()
        } else {
            hits.join("\n")
        }))
    }
);

tool!(
    FileWrite,
    "file_write",
    "Write a text file in the chat's workspace (creates or replaces it). The user approves each write.",
    EffectClass::ExternalSideEffect,
    json!({"type": "object", "required": ["path", "text"], "additionalProperties": false,
           "properties": {"path": {"type": "string"}, "text": {"type": "string"}}}),
    |ws, args| {
        let p = ws.write(&path_arg(&args), args["text"].as_str().unwrap_or_default())?;
        Ok(ToolOutput::text(format!("Wrote {p}.")))
    }
);

/// What a turn may do, from the composer's mode picker.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum TurnMode {
    /// Answers, reading and searching as useful. Tools that change
    /// things are left out.
    #[default]
    Chat,
    /// Writes a plan of the steps and sources first; no tool runs.
    Plan,
    /// May use tools that change things (vault edits, file writes, MCP
    /// tools that act), each still passing the policy gate.
    Act,
}

impl TurnMode {
    /// Whether a tool with `effect` is offered in this mode.
    pub fn allows(self, effect: EffectClass) -> bool {
        match self {
            Self::Plan => false,
            Self::Act => true,
            Self::Chat => matches!(
                effect,
                EffectClass::Read | EffectClass::Analysis | EffectClass::Network
            ),
        }
    }

    /// The line added to the system prompt.
    pub fn instructions(self) -> Option<&'static str> {
        match self {
            Self::Chat => None,
            Self::Plan => Some(
                "Plan mode: do not call any tool and do not answer the question yet. Reply with a short numbered plan: the steps you would take, the sources or files you would check, and what the answer will cover. End with one line asking the user to approve or change the plan.",
            ),
            Self::Act => Some(
                "Act mode: you may use tools that change things (writing notes or files, MCP tools that act). Each such step is shown to the user for approval; say what you will change before you do it.",
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws() -> (tempfile::TempDir, Workspace) {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("src/inner")).unwrap();
        std::fs::write(
            d.path().join("README.md"),
            "# Demo\nPaging maps virtual pages.\n",
        )
        .unwrap();
        std::fs::write(d.path().join("src/lib.rs"), "fn page_table() {}\n").unwrap();
        std::fs::write(d.path().join(".secret"), "hidden").unwrap();
        let w = Workspace::open(d.path()).unwrap();
        (d, w)
    }

    #[test]
    fn lists_reads_and_searches_inside() {
        let (_d, w) = ws();
        assert_eq!(w.list("").unwrap(), ["README.md", "src/"]);
        assert_eq!(w.list("src").unwrap(), ["src/inner/", "src/lib.rs"]);
        assert!(w.read("README.md").unwrap().contains("Paging"));
        let hits = w.search("virtual PAGES");
        assert_eq!(hits, ["README.md:2: Paging maps virtual pages."]);
    }

    #[test]
    fn refuses_paths_that_leave() {
        let (d, w) = ws();
        for bad in ["../x", "/etc/passwd", "src/../../x"] {
            assert!(w.resolve(bad).is_err(), "{bad}");
        }
        #[cfg(unix)]
        {
            let outside = tempfile::tempdir().unwrap();
            std::fs::write(outside.path().join("o.txt"), "out").unwrap();
            std::os::unix::fs::symlink(outside.path().join("o.txt"), d.path().join("link.txt"))
                .unwrap();
            assert!(w.read("link.txt").is_err());
        }
    }

    #[test]
    fn one_file_workspace_reaches_only_that_file() {
        let (d, _) = ws();
        let w = Workspace::open(&d.path().join("README.md")).unwrap();
        assert!(w.is_file());
        assert_eq!(w.name(), "README.md");
        assert_eq!(w.list("").unwrap(), ["README.md"]);
        assert!(w.read("README.md").is_ok());
        assert!(w.read("src/lib.rs").is_err());
    }

    #[test]
    fn writes_inside_and_modes_gate_effects() {
        let (_d, w) = ws();
        assert_eq!(w.write("notes.txt", "hi").unwrap(), "notes.txt");
        assert_eq!(w.read("notes.txt").unwrap(), "hi");
        assert!(w.write("../escape.txt", "x").is_err());
        assert!(TurnMode::Chat.allows(EffectClass::Read));
        assert!(TurnMode::Chat.allows(EffectClass::Network));
        assert!(!TurnMode::Chat.allows(EffectClass::Write));
        assert!(!TurnMode::Chat.allows(EffectClass::ExternalSideEffect));
        assert!(TurnMode::Act.allows(EffectClass::ExternalSideEffect));
        assert!(!TurnMode::Plan.allows(EffectClass::Read));
        assert!(
            TurnMode::Plan
                .instructions()
                .unwrap()
                .contains("do not call any tool")
        );
    }

    #[test]
    fn registry_mask_follows_the_mode() {
        let (_d, w) = ws();
        let mut tools = ToolRegistry::new();
        Arc::new(w).register(&mut tools);
        tools.set_mode(TurnMode::Chat);
        assert!(tools.get("file_read").is_some());
        assert!(tools.get("file_write").is_none());
        assert!(!tools.describe().contains("file_write"));
        tools.set_mode(TurnMode::Act);
        assert!(tools.get("file_write").is_some());
        tools.set_mode(TurnMode::Plan);
        assert!(tools.is_empty());
        assert!(tools.describe().is_empty());
        Workspace::unregister(&mut tools);
        tools.set_mode(TurnMode::Act);
        assert!(tools.is_empty());
    }
}
