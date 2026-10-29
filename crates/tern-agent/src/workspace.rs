//! A chat's workspace: a folder (or one file) the user opened for the
//! chat, and the file tools scoped to it.
//!
//! | Tool | Effect | What it does |
//! | --- | --- | --- |
//! | `files_list` | Read | Lists a folder inside the workspace (hidden entries left out) |
//! | `file_read` | Read | Reads a text file inside the workspace, or by a full path the user typed (asks for a file outside it) |
//! | `file_search` | Read | Finds lines containing words, across text files |
//! | `file_write` | External side effect (asks) | Writes a text file inside the workspace; Act mode only |
//! | `file_edit` | External side effect (asks) | Replaces one exact passage or adds lines at the end; shown and approved as the `file_write` of the new text |
//!
//! Every relative path is resolved against the workspace root and checked
//! after following symlinks, so `..` and links that point out are refused.
//! A one-file workspace exposes only that file. `file_read` also takes a
//! full path (`C:\Users\me\a.txt`, `/home/me/a.md`, `~/a.md`, quoted or
//! not): inside the workspace it reads as usual, outside it the user
//! approves the file first. With no workspace open, [`LocalFileRead`]
//! reads full paths only, each approved.

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
pub const TOOL_NAMES: [&str; 7] = [
    "files_list",
    "file_read",
    "file_search",
    "file_write",
    "file_edit",
    "file_rename",
    "run_command",
];

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

    /// The folder: the workspace itself, or a one-file workspace's folder.
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn is_file(&self) -> bool {
        self.only.is_some()
    }

    /// Resolves a path the model gave (relative to the root, `/`
    /// separated) to a path inside the workspace, or says why not.
    pub fn resolve(&self, rel: &str) -> Result<PathBuf, ToolError> {
        let rel = self.unroot(rel);
        let rel = rel.as_str();
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
        // the nearest folder above it that exists (#79: `file_write`
        // creates the folders in between).
        let real = if joined.exists() {
            joined
                .canonicalize()
                .map_err(|e| ToolError::Failed(e.to_string()))?
        } else {
            let mut base = joined.as_path();
            let mut tail = Vec::new();
            loop {
                tail.push(base.file_name().unwrap_or_default().to_owned());
                base = base
                    .parent()
                    .ok_or_else(|| ToolError::Failed("No folder for that path.".into()))?;
                if base.exists() {
                    break;
                }
            }
            let mut real = base
                .canonicalize()
                .map_err(|_| ToolError::Failed("That folder does not exist.".into()))?;
            for t in tail.iter().rev() {
                real.push(t);
            }
            real
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

    /// `rel` without a first segment that repeats the folder's own name,
    /// when the folder has nothing by that name (#79: in a folder named
    /// `New` the model listed `New` and wrote `New/review.md`, which would
    /// have made `New/New`). See [`unroot_path`].
    pub fn unroot(&self, rel: &str) -> String {
        if self.only.is_some() {
            let rel = rel.trim();
            return rel.strip_prefix("./").unwrap_or(rel).to_owned();
        }
        unroot_path(&self.root, rel)
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

    /// Reads a text file: a path relative to the workspace, or a full
    /// path the user typed (`C:\\Users\\me\\a.txt`, `/home/me/a.md`,
    /// `~/a.md`, quoted or not). A full path outside the workspace is read
    /// too; the turn loop asks the user first ([`Workspace::outside`]).
    pub fn read(&self, rel: &str) -> Result<String, ToolError> {
        if crate::paths::looks_absolute(rel) {
            return read_text_file(&crate::paths::user_path(rel));
        }
        read_text_file(&self.resolve(rel)?)
    }

    /// The full path `raw` names when it is one and lies outside the
    /// workspace (or is not its one file), so reading it needs approval.
    pub fn outside(&self, raw: &str) -> Option<PathBuf> {
        if !crate::paths::looks_absolute(raw) {
            return None;
        }
        let p = crate::paths::user_path(raw);
        let real = p.canonicalize().unwrap_or_else(|_| p.clone());
        let inside = match &self.only {
            Some(only) => real == *only,
            None => real.starts_with(&self.root),
        };
        (!inside).then_some(p)
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
        // Missing folders in the path are made (#79: the model tried
        // `run_command mkdir` first).
        if let Some(dir) = p.parent()
            && !dir.is_dir()
        {
            std::fs::create_dir_all(dir).map_err(|e| ToolError::Failed(e.to_string()))?;
        }
        let tmp = p.with_extension("tern-tmp");
        std::fs::write(&tmp, text).map_err(|e| ToolError::Failed(e.to_string()))?;
        std::fs::rename(&tmp, &p).map_err(|e| ToolError::Failed(e.to_string()))?;
        Ok(self.rel(&p))
    }

    /// The whole text of a file after a `file_edit` (#78): `find` (exact,
    /// once) replaced by `replace`, or `append` added at the end on its
    /// own line. A missing file can only be appended to (it is created).
    pub fn edited(&self, rel: &str, args: &Value) -> Result<String, String> {
        let p = self.resolve(rel).map_err(|e| match e {
            ToolError::Failed(m) | ToolError::BadInput(m) => m,
            ToolError::TimedOut => "timed out".into(),
        })?;
        if p.is_dir() {
            return Err("that is a folder; give a file inside it".into());
        }
        let old = if p.exists() {
            if std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0) > MAX_FILE_BYTES {
                return Err("that file is too large to edit".into());
            }
            std::fs::read_to_string(&p).map_err(|_| "that file is not text".to_owned())?
        } else {
            String::new()
        };
        let s = |k: &str| args.get(k).and_then(Value::as_str);
        if let Some(add) = s("append").filter(|a| !a.trim().is_empty()) {
            let mut text = old.clone();
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            text.push_str(add);
            if (old.is_empty() || old.ends_with('\n')) && !add.ends_with('\n') {
                text.push('\n');
            }
            return Ok(text);
        }
        let Some(find) = s("find").filter(|f| !f.is_empty()) else {
            return Err("give `append` (the lines to add at the end) or `find` and `replace` (an exact passage of the file and its new text)".into());
        };
        if !p.exists() {
            return Err(format!(
                "there is no file at {rel}; create it with `file_write`"
            ));
        }
        let replace = s("replace").unwrap_or_default();
        // Windows line endings: a `find` written with \n still matches.
        let (find, replace) = if old.contains("\r\n") && !find.contains('\r') {
            (find.replace('\n', "\r\n"), replace.replace('\n', "\r\n"))
        } else {
            (find.to_owned(), replace.to_owned())
        };
        match old.matches(find.as_str()).count() {
            1 => Ok(old.replacen(find.as_str(), &replace, 1)),
            0 => Err(format!(
                "`find` is not in {rel}; read it with `file_read` and copy the passage exactly"
            )),
            n => Err(format!(
                "`find` is in {rel} {n} times; give a longer passage that is there once"
            )),
        }
    }

    /// Renames `from` to `to`, both inside the workspace. `to` must not
    /// exist (nothing is overwritten); missing folders on its path are
    /// made.
    pub fn rename(&self, from: &str, to: &str) -> Result<(String, String), ToolError> {
        let a = self.resolve(from)?;
        if !a.exists() {
            return Err(ToolError::Failed(format!("There is no file at {from}.")));
        }
        if a == self.root {
            return Err(ToolError::Failed(
                "The workspace folder itself cannot be renamed.".into(),
            ));
        }
        let b = self.resolve(to)?;
        if b.exists() {
            return Err(ToolError::Failed(format!(
                "{to} already exists; nothing was renamed."
            )));
        }
        if let Some(dir) = b.parent()
            && !dir.is_dir()
        {
            std::fs::create_dir_all(dir).map_err(|e| ToolError::Failed(e.to_string()))?;
        }
        std::fs::rename(&a, &b).map_err(|e| ToolError::Failed(e.to_string()))?;
        Ok((self.rel(&a), self.rel(&b)))
    }

    /// Adds the file tools for this workspace to `tools`.
    pub fn register(self: &Arc<Self>, tools: &mut ToolRegistry) {
        tools.register(FilesList(self.clone()));
        tools.register(FileRead(self.clone()));
        tools.register(FileSearch(self.clone()));
        tools.register(FileWrite(self.clone()));
        tools.register(FileEdit(self.clone()));
        tools.register(FileRename(self.clone()));
        crate::workbench::register(self, tools);
    }

    /// Takes the file tools out.
    pub fn unregister(tools: &mut ToolRegistry) {
        for n in TOOL_NAMES {
            tools.unregister(n);
        }
    }
}

/// A path the model gave, relative to the folder `root`, without a first
/// segment that repeats the folder's own name when `root` has nothing by
/// that name (#79): in a folder named `New`, `New` is the folder itself and
/// `New/review.md` is `review.md`. A leading `./` is dropped too. Names
/// match without case on Windows and macOS.
pub fn unroot_path(root: &Path, rel: &str) -> String {
    let t = rel.trim();
    let t = t.strip_prefix("./").unwrap_or(t);
    let Some(name) = root.file_name().map(|n| n.to_string_lossy().into_owned()) else {
        return t.to_owned();
    };
    let (first, rest) = match t.find(['/', '\\']) {
        Some(i) => (&t[..i], &t[i + 1..]),
        None => (t, ""),
    };
    let same = if cfg!(windows) || cfg!(target_os = "macos") {
        first.eq_ignore_ascii_case(&name)
    } else {
        first == name
    };
    if first.is_empty() || !same || root.join(first).exists() {
        return t.to_owned();
    }
    rest.trim_start_matches(['/', '\\']).to_owned()
}

/// Reads a text file at a full path, with the same limits as a
/// workspace read: not a folder, at most 2 MB, UTF-8, cut at 40,000
/// characters.
pub fn read_text_file(p: &Path) -> Result<String, ToolError> {
    if p.as_os_str().is_empty() {
        return Err(ToolError::Failed("Give the file's path.".into()));
    }
    if p.is_dir() {
        return Err(ToolError::Failed(
            "That is a folder; give a file inside it.".into(),
        ));
    }
    let meta = std::fs::metadata(p).map_err(|e| {
        ToolError::Failed(match e.kind() {
            std::io::ErrorKind::NotFound => format!("There is no file at {}.", p.display()),
            std::io::ErrorKind::PermissionDenied => {
                format!("This computer does not let Tern read {}.", p.display())
            }
            _ => format!("Cannot read {}: {e}", p.display()),
        })
    })?;
    if meta.len() > MAX_FILE_BYTES {
        return Err(ToolError::Failed("That file is too large to read.".into()));
    }
    let bytes = std::fs::read(p).map_err(|e| ToolError::Failed(e.to_string()))?;
    let text =
        String::from_utf8(bytes).map_err(|_| ToolError::Failed("That file is not text.".into()))?;
    Ok(match text.char_indices().nth(READ_CHARS) {
        Some((at, _)) => format!("{}\n[cut at {READ_CHARS} characters]", &text[..at]),
        None => text,
    })
}

/// `file_read` for a chat with no workspace: reads a text file by the
/// full path the user typed. Each file is approved by the user first.
#[derive(Debug, Clone, Default)]
pub struct LocalFileRead;

impl Tool for LocalFileRead {
    fn name(&self) -> &'static str {
        "file_read"
    }
    fn summary(&self) -> &'static str {
        "Read a text file on this computer by the full path the user gave (C:\\Users\\..., /home/..., ~/...). The user approves each file."
    }
    fn effect(&self) -> EffectClass {
        EffectClass::Read
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object", "required": ["path"], "additionalProperties": false,
               "properties": {"path": {"type": "string"}}})
    }
    fn target(&self, args: &Value) -> Option<String> {
        Some(
            crate::paths::user_path(&path_arg(args))
                .display()
                .to_string(),
        )
    }
    fn asks_because(&self, args: &Value) -> Option<String> {
        Some(format!(
            "it reads {} from this computer",
            crate::paths::user_path(&path_arg(args)).display()
        ))
    }
    fn call<'a>(&'a self, args: Value) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let raw = path_arg(&args);
            if !crate::paths::looks_absolute(&raw) {
                return Err(ToolError::Failed(
                    "Give the file's full path (C:\\Users\\..., /home/..., or ~/...); no folder is open as this chat's workspace.".into(),
                ));
            }
            tokio::task::spawn_blocking(move || read_text_file(&crate::paths::user_path(&raw)))
                .await
                .map_err(|e| ToolError::Failed(e.to_string()))?
                .map(ToolOutput::text)
        })
    }
}

/// Adds [`LocalFileRead`] (a chat with no workspace).
pub fn register_local_read(tools: &mut ToolRegistry) {
    tools.register(LocalFileRead);
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
                Some(format!(
                    "{}/{}",
                    self.0.name(),
                    self.0.unroot(&path_arg(args))
                ))
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

#[derive(Debug, Clone)]
struct FilesList(Arc<Workspace>);

impl Tool for FilesList {
    fn name(&self) -> &'static str {
        "files_list"
    }
    fn summary(&self) -> &'static str {
        "List a folder in the chat's workspace (path relative to it; empty for the top), or any folder by the full path the user gave (the user approves folders outside the workspace)."
    }
    fn effect(&self) -> EffectClass {
        EffectClass::Read
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object", "additionalProperties": false,
               "properties": {"path": {"type": "string"}}})
    }
    fn target(&self, args: &Value) -> Option<String> {
        let raw = path_arg(args);
        Some(match self.0.outside(&raw) {
            Some(p) => p.display().to_string(),
            None => format!("{}/{}", self.0.name(), self.0.unroot(&raw)),
        })
    }
    fn asks_because(&self, args: &Value) -> Option<String> {
        self.0
            .outside(&path_arg(args))
            .map(|p| format!("{} is outside the workspace", p.display()))
    }
    fn rule_paths(&self, args: &Value) -> Vec<PathBuf> {
        self.0.outside(&path_arg(args)).into_iter().collect()
    }
    fn call<'a>(&'a self, args: Value) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        let ws = self.0.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let raw = path_arg(&args);
                let rows = match ws.outside(&raw) {
                    Some(p) => list_outside(&p)?,
                    None => ws.list(&raw)?,
                };
                Ok(ToolOutput::text(if rows.is_empty() {
                    "The folder is empty.".to_owned()
                } else {
                    rows.join("\n")
                }))
            })
            .await
            .map_err(|e| ToolError::Failed(e.to_string()))?
        })
    }
}

/// A folder outside the workspace the user approved: its entries by
/// full path, folders ending in `/`. Protected folders (keys, browser
/// profiles, Tern's own data) are refused.
fn list_outside(dir: &Path) -> Result<Vec<String>, ToolError> {
    if let Some(why) = crate::sandbox::protected(dir) {
        return Err(ToolError::Failed(why));
    }
    if !dir.is_dir() {
        return Err(ToolError::Failed(format!(
            "There is no folder at {}.",
            dir.display()
        )));
    }
    let rd = std::fs::read_dir(dir).map_err(|e| ToolError::Failed(e.to_string()))?;
    let mut out: Vec<String> = rd
        .flatten()
        .map(|e| {
            let p = e.path();
            let shown = p.display().to_string();
            if p.is_dir() {
                format!("{shown}/")
            } else {
                shown
            }
        })
        .collect();
    out.sort();
    out.truncate(MAX_LIST);
    Ok(out)
}

/// `file_rename` (#76): moves or renames a file inside the workspace.
/// The user sees old -> new on the approval card first.
#[derive(Debug, Clone)]
struct FileRename(Arc<Workspace>);

impl Tool for FileRename {
    fn name(&self) -> &'static str {
        "file_rename"
    }
    fn summary(&self) -> &'static str {
        "Rename or move a file inside the chat's workspace (from -> to, both relative to its folder). The user approves it first on a card showing the old and new path. Use this to rename; never write a new copy instead."
    }
    fn effect(&self) -> EffectClass {
        EffectClass::ExternalSideEffect
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object", "required": ["from", "to"], "additionalProperties": false,
               "properties": {"from": {"type": "string"}, "to": {"type": "string"}}})
    }
    fn target(&self, args: &Value) -> Option<String> {
        Some(format!(
            "{}/{} -> {}",
            self.0.name(),
            self.0.unroot(args["from"].as_str().unwrap_or_default()),
            self.0.unroot(args["to"].as_str().unwrap_or_default())
        ))
    }
    fn confined(&self, args: &Value) -> bool {
        self.0
            .resolve(args["from"].as_str().unwrap_or_default())
            .is_ok()
            && self
                .0
                .resolve(args["to"].as_str().unwrap_or_default())
                .is_ok()
    }
    fn rule_paths(&self, args: &Value) -> Vec<PathBuf> {
        ["from", "to"]
            .iter()
            .filter_map(|k| self.0.resolve(args[*k].as_str().unwrap_or_default()).ok())
            .collect()
    }
    fn call<'a>(&'a self, args: Value) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        let ws = self.0.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let from = args["from"].as_str().unwrap_or_default();
                let (a, b) = ws.rename(from, args["to"].as_str().unwrap_or_default())?;
                // The Files tab's preview of the old name follows it (#79).
                if !crate::live::rename_preview(&ws.unroot(from), &b) {
                    crate::live::rename_preview(&a, &b);
                }
                Ok(ToolOutput::text(format!("Renamed {a} to {b}.")))
            })
            .await
            .map_err(|e| ToolError::Failed(e.to_string()))?
        })
    }
}

#[derive(Debug, Clone)]
struct FileRead(Arc<Workspace>);

impl Tool for FileRead {
    fn name(&self) -> &'static str {
        "file_read"
    }
    fn summary(&self) -> &'static str {
        "Read a text file in the chat's workspace by its path (from files_list or file_search), or any file by the full path the user gave (the user approves files outside the workspace)."
    }
    fn effect(&self) -> EffectClass {
        EffectClass::Read
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object", "required": ["path"], "additionalProperties": false,
               "properties": {"path": {"type": "string"}}})
    }
    fn target(&self, args: &Value) -> Option<String> {
        let raw = path_arg(args);
        Some(match self.0.outside(&raw) {
            Some(p) => p.display().to_string(),
            None => format!("{}/{}", self.0.name(), self.0.unroot(&raw)),
        })
    }
    fn asks_because(&self, args: &Value) -> Option<String> {
        self.0
            .outside(&path_arg(args))
            .map(|p| format!("{} is outside the workspace", p.display()))
    }
    fn rule_paths(&self, args: &Value) -> Vec<PathBuf> {
        self.0.outside(&path_arg(args)).into_iter().collect()
    }
    fn call<'a>(&'a self, args: Value) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        let ws = self.0.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || ws.read(&path_arg(&args)).map(ToolOutput::text))
                .await
                .map_err(|e| ToolError::Failed(e.to_string()))?
        })
    }
}

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

#[derive(Debug, Clone)]
struct FileWrite(Arc<Workspace>);

impl Tool for FileWrite {
    fn name(&self) -> &'static str {
        "file_write"
    }
    fn summary(&self) -> &'static str {
        "Write a text file in the chat's workspace (creates or replaces it; missing folders in the path are created, so no mkdir is needed). The user sees the text as it is written and a diff before it is saved, and approves the write unless the approval mode allows it."
    }
    fn effect(&self) -> EffectClass {
        EffectClass::ExternalSideEffect
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object", "required": ["path", "text"], "additionalProperties": false,
               "properties": {"path": {"type": "string"}, "text": {"type": "string"}}})
    }
    fn target(&self, args: &Value) -> Option<String> {
        Some(format!(
            "{}/{}",
            self.0.name(),
            self.0.unroot(&path_arg(args))
        ))
    }
    fn confined(&self, args: &Value) -> bool {
        self.0.resolve(&path_arg(args)).is_ok()
    }
    fn rule_paths(&self, args: &Value) -> Vec<PathBuf> {
        self.0.resolve(&path_arg(args)).into_iter().collect()
    }
    fn old_text(&self, args: &Value) -> Option<String> {
        let p = self.0.resolve(&path_arg(args)).ok()?;
        if std::fs::metadata(&p).ok()?.len() > MAX_FILE_BYTES {
            return None;
        }
        std::fs::read_to_string(&p).ok()
    }
    fn check(&self, args: &Value) -> Option<String> {
        // A new file with no text (#78: a task graph wrote an empty
        // counts.md and the card showed +0 -0): sent back, never asked.
        let empty = args["text"].as_str().is_none_or(|t| t.trim().is_empty());
        let new = self
            .0
            .resolve(&path_arg(args))
            .is_ok_and(|p| std::fs::metadata(&p).ok().is_none_or(|m| m.len() == 0));
        (empty && new).then(|| {
            format!(
                "`text` is empty, so {} was not created. Send the file's whole content in `text`; to add lines to a file that has some, use `file_edit` with `append`",
                path_arg(args)
            )
        })
    }
    fn call<'a>(&'a self, args: Value) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        let ws = self.0.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let p = ws.write(&path_arg(&args), args["text"].as_str().unwrap_or_default())?;
                Ok(ToolOutput::text(format!("Wrote {p}.")))
            })
            .await
            .map_err(|e| ToolError::Failed(e.to_string()))?
        })
    }
}

/// `file_edit` (#78): a small change to a file without sending all of
/// it: one exact passage replaced, or lines added at the end. The turn
/// runs it as the `file_write` of the whole new text ([`Tool::expand`]),
/// so the user approves it on the same card with a +N -M diff and the
/// same permission rules apply.
#[derive(Debug, Clone)]
struct FileEdit(Arc<Workspace>);

impl Tool for FileEdit {
    fn name(&self) -> &'static str {
        "file_edit"
    }
    fn summary(&self) -> &'static str {
        "Change part of a text file in the chat's workspace: `append` adds lines at the end; `find` (an exact passage that is in the file once) is replaced by `replace`. Use it to add a line or fix a passage instead of rewriting the file. The user approves the diff."
    }
    fn effect(&self) -> EffectClass {
        EffectClass::ExternalSideEffect
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object", "required": ["path"], "additionalProperties": false,
               "properties": {"path": {"type": "string"}, "append": {"type": "string"},
                              "find": {"type": "string"}, "replace": {"type": "string"}}})
    }
    fn target(&self, args: &Value) -> Option<String> {
        Some(format!(
            "{}/{}",
            self.0.name(),
            self.0.unroot(&path_arg(args))
        ))
    }
    fn confined(&self, args: &Value) -> bool {
        self.0.resolve(&path_arg(args)).is_ok()
    }
    fn rule_paths(&self, args: &Value) -> Vec<PathBuf> {
        self.0.resolve(&path_arg(args)).into_iter().collect()
    }
    fn expand(&self, args: &Value) -> Option<Result<(&'static str, Value), String>> {
        let path = path_arg(args);
        Some(
            self.0
                .edited(&path, args)
                .map(|text| ("file_write", json!({"path": path, "text": text}))),
        )
    }
    fn call<'a>(&'a self, args: Value) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        // Outside a turn (a turn runs it as a `file_write`).
        let ws = self.0.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let path = path_arg(&args);
                let text = ws.edited(&path, &args).map_err(ToolError::Failed)?;
                let p = ws.write(&path, &text)?;
                Ok(ToolOutput::text(format!("Edited {p}.")))
            })
            .await
            .map_err(|e| ToolError::Failed(e.to_string()))?
        })
    }
}

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
                "Act mode: you may use tools that change things (writing notes or files, renaming files, running commands, MCP tools that act). Call them directly: the user approves each step on its card, so do not ask for permission in prose.",
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
    fn a_first_segment_naming_the_folder_is_the_folder() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("New");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("sum.md"), "x").unwrap();
        assert_eq!(unroot_path(&root, "New"), "");
        assert_eq!(unroot_path(&root, "New/review.md"), "review.md");
        assert_eq!(unroot_path(&root, "./New\\a/b.md"), "a/b.md");
        assert_eq!(unroot_path(&root, "src/New/x.md"), "src/New/x.md");
        assert_eq!(unroot_path(&root, "notes.md"), "notes.md");
        // A real subfolder of that name is kept.
        std::fs::create_dir_all(root.join("New")).unwrap();
        assert_eq!(unroot_path(&root, "New/review.md"), "New/review.md");
        let w = Workspace::open(&root).unwrap();
        assert_eq!(w.unroot("./sum.md"), "sum.md");
    }

    #[test]
    fn writes_make_missing_folders_but_never_outside() {
        let (d, w) = ws();
        assert_eq!(w.write("a/b/c.md", "hi").unwrap(), "a/b/c.md");
        assert_eq!(
            std::fs::read_to_string(d.path().join("a/b/c.md")).unwrap(),
            "hi"
        );
        assert!(w.write("../x/y.md", "no").is_err());
        assert_eq!(w.rename("a/b/c.md", "z/c.md").unwrap().1, "z/c.md");
        assert!(d.path().join("z/c.md").is_file());
    }

    #[test]
    fn refuses_paths_that_leave() {
        let (_d, w) = ws();
        for bad in ["../x", "/etc/passwd", "src/../../x"] {
            assert!(w.resolve(bad).is_err(), "{bad}");
        }
        #[cfg(unix)]
        {
            let d = &_d;
            let outside = tempfile::tempdir().unwrap();
            std::fs::write(outside.path().join("o.txt"), "out").unwrap();
            std::os::unix::fs::symlink(outside.path().join("o.txt"), d.path().join("link.txt"))
                .unwrap();
            assert!(w.read("link.txt").is_err());
        }
    }

    #[test]
    fn full_paths_read_inside_and_outside_with_approval() {
        let (d, w) = ws();
        let inside = d.path().join("README.md");
        let raw = inside.display().to_string();
        assert!(w.read(&raw).unwrap().contains("Paging"));
        assert!(w.read(&format!("\"{raw}\"")).unwrap().contains("Paging"));
        assert_eq!(w.outside(&raw), None, "inside needs no approval");
        assert_eq!(w.outside("README.md"), None, "relative paths stay inside");
        let other = tempfile::tempdir().unwrap();
        let out = other.path().join("My Notes.txt");
        std::fs::write(&out, "outside text").unwrap();
        let raw = out.display().to_string();
        assert!(w.outside(&raw).is_some(), "outside asks");
        assert_eq!(w.read(&format!("'{raw}'")).unwrap(), "outside text");
        let tool = FileRead(Arc::new(w.clone()));
        let args = json!({"path": raw});
        assert!(
            tool.asks_because(&args)
                .unwrap()
                .contains("outside the workspace")
        );
        assert!(tool.asks_because(&json!({"path": "README.md"})).is_none());
        // Relative paths still never leave.
        assert!(w.read("../x").is_err());
        let missing = other.path().join("nope.txt").display().to_string();
        let e = w.read(&missing).unwrap_err().to_string();
        assert!(e.contains("There is no file at"), "{e}");
    }

    #[test]
    fn local_read_takes_full_and_home_paths_only() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("x.md");
        std::fs::write(&f, "# x").unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let t = LocalFileRead;
        let read = |p: String| rt.block_on(t.call(json!({ "path": p })));
        assert_eq!(read(f.display().to_string()).unwrap().content, "# x");
        assert_eq!(read(format!("\"{}\"", f.display())).unwrap().content, "# x");
        assert!(read("x.md".into()).is_err(), "relative needs a workspace");
        assert!(
            t.asks_because(&json!({"path": "/x"})).is_some(),
            "always asks"
        );
        if let Some(h) = crate::paths::home()
            && let Ok(rel) = f.strip_prefix(&h)
        {
            let tilde = format!("~/{}", rel.display());
            assert_eq!(read(tilde).unwrap().content, "# x");
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

    #[test]
    fn edits_append_or_replace_once_and_empty_new_files_are_refused() {
        let (d, w) = ws();
        let w = Arc::new(w);
        let mut tools = ToolRegistry::new();
        w.register(&mut tools);
        let edit = tools.get("file_edit").unwrap();
        // Append: one line at the end, on its own line.
        let (name, args) = edit
            .expand(&json!({"path": "README.md", "append": "A new line."}))
            .unwrap()
            .unwrap();
        assert_eq!(name, "file_write");
        assert_eq!(
            args["text"],
            "# Demo\nPaging maps virtual pages.\nA new line.\n"
        );
        // Find and replace, exactly once.
        let (_, args) = edit
            .expand(&json!({"path": "README.md", "find": "virtual", "replace": "logical"}))
            .unwrap()
            .unwrap();
        assert_eq!(args["text"], "# Demo\nPaging maps logical pages.\n");
        let e = edit
            .expand(&json!({"path": "README.md", "find": "nowhere", "replace": "x"}))
            .unwrap()
            .unwrap_err();
        assert!(e.contains("is not in README.md"), "{e}");
        std::fs::write(d.path().join("twice.md"), "a a").unwrap();
        let e = w
            .edited("twice.md", &json!({"find": "a", "replace": "b"}))
            .unwrap_err();
        assert!(e.contains("2 times"), "{e}");
        assert!(w.edited("README.md", &json!({})).is_err());
        assert!(w.edited("../x.md", &json!({"append": "x"})).is_err());
        // A new file with no text goes back to the model.
        let write = tools.get("file_write").unwrap();
        let why = write
            .check(&json!({"path": "counts.md", "text": "  "}))
            .unwrap();
        assert!(why.contains("was not created"), "{why}");
        assert!(
            write
                .check(&json!({"path": "counts.md", "text": "3"}))
                .is_none()
        );
        // Emptying a file that has text is a change the card shows.
        assert!(
            write
                .check(&json!({"path": "README.md", "text": ""}))
                .is_none()
        );
    }
}
