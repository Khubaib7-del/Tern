//! The tool result cache (#77): a search, a page fetch or a file read
//! asked again with the same arguments gets its earlier result instead of
//! running again, within a time limit. Kept per chat in memory, and on
//! disk (`<data>/cache/tools`) for the web tools, so a reopened chat or a
//! second chat asking the same thing does not fetch it twice.
//!
//! Only tools whose result depends on their arguments alone are cached,
//! and only successful results. Writes, renames and commands never are,
//! and any successful change empties the cached file reads, so a file
//! read after a write is read again. The key is the tool and its
//! arguments, normalised (key order, spacing, a URL's fragment and
//! trailing slash).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::tools::ToolOutput;

/// How long a tool's result stays good, or `None` when it is never
/// cached.
pub fn ttl(tool: &str) -> Option<Duration> {
    let mins = |m: u64| Some(Duration::from_secs(m * 60));
    match tool {
        "web_search" => mins(60),
        "scholar_search" | "paper_read" | "paper_snowball" => mins(24 * 60),
        "read_url" => mins(6 * 60),
        // Local reads: short, and emptied by any change.
        "file_read" | "files_list" | "file_search" | "vault_read" | "vault_search" => mins(10),
        _ => None,
    }
}

/// Whether a tool's cached results may go to disk (web results; local
/// files stay in memory).
fn on_disk(tool: &str) -> bool {
    matches!(
        tool,
        "web_search" | "scholar_search" | "paper_read" | "paper_snowball" | "read_url"
    )
}

/// Tools whose success empties the cached local reads.
pub fn changes_files(tool: &str) -> bool {
    matches!(
        tool,
        "file_write" | "file_edit" | "file_rename" | "vault_write" | "run_command" | "export"
    ) || tool.starts_with("mcp__")
}

fn local(tool: &str) -> bool {
    ttl(tool).is_some() && !on_disk(tool)
}

/// Arguments in one canonical form: object keys sorted, strings trimmed
/// with inner runs of spaces collapsed; a `url` loses its `#fragment`
/// and trailing `/`.
pub fn normalize(args: &Value) -> Value {
    fn norm(k: Option<&str>, v: &Value) -> Value {
        match v {
            Value::String(s) => {
                let mut t = s.split_whitespace().collect::<Vec<_>>().join(" ");
                if k == Some("url") {
                    if let Some(i) = t.find('#') {
                        t.truncate(i);
                    }
                    while t.ends_with('/') && t.matches('/').count() > 3 {
                        t.pop();
                    }
                }
                Value::String(t)
            }
            Value::Array(a) => Value::Array(a.iter().map(|x| norm(None, x)).collect()),
            Value::Object(o) => {
                let mut keys: Vec<&String> = o.keys().collect();
                keys.sort();
                let mut m = serde_json::Map::new();
                for key in keys {
                    m.insert(key.clone(), norm(Some(key), &o[key]));
                }
                Value::Object(m)
            }
            other => other.clone(),
        }
    }
    norm(None, args)
}

/// The cache key for a call.
pub fn key(tool: &str, args: &Value) -> String {
    let mut h = Sha256::new();
    h.update(tool.as_bytes());
    h.update([0u8]);
    h.update(normalize(args).to_string().as_bytes());
    let d = h.finalize();
    d.iter().take(16).map(|b| format!("{b:02x}")).collect()
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

/// One cached result.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Entry {
    tool: String,
    /// Unix seconds when it was stored.
    at: u64,
    content: String,
    /// The chat that stored it (#85, A9): a search from another chat is
    /// not handed to this one as its own.
    #[serde(default)]
    chat: Option<String>,
    #[serde(default)]
    evidence_refs: Vec<String>,
    #[serde(default)]
    meta: Value,
}

/// A hit: the stored output and how old it is.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub output: ToolOutput,
    pub age: Duration,
}

/// A chat's tool cache.
#[derive(Debug, Default)]
pub struct ToolCache {
    mem: Mutex<HashMap<String, Entry>>,
    dir: Option<PathBuf>,
    /// The chat the cache serves now (#85, A9).
    chat: Mutex<Option<String>>,
}

impl ToolCache {
    /// In memory only (tests, and when there is no data folder).
    pub fn memory() -> Self {
        Self::default()
    }

    /// In memory, with web results kept in `dir` too.
    pub fn with_dir(dir: impl Into<PathBuf>) -> Self {
        Self {
            mem: Mutex::default(),
            dir: Some(dir.into()),
            chat: Mutex::default(),
        }
    }

    /// Sets the chat the next calls are for (#85, A9): searches are kept
    /// per chat, and another chat's search is run again rather than
    /// handed over as this chat's result.
    pub fn set_chat(&self, chat: &str) {
        *self.chat.lock().unwrap_or_else(|e| e.into_inner()) = Some(chat.to_owned());
    }

    fn current_chat(&self) -> Option<String> {
        self.chat.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// The folder under the app's data folder.
    pub fn dir_in(data_dir: &Path) -> PathBuf {
        data_dir.join("cache").join("tools")
    }

    fn path(&self, key: &str) -> Option<PathBuf> {
        self.dir.as_ref().map(|d| d.join(format!("{key}.json")))
    }

    /// The cached result of `tool` with `args`, when there is a fresh one.
    pub fn get(&self, tool: &str, args: &Value) -> Option<Hit> {
        if disabled() {
            return None;
        }
        let ttl = ttl(tool)?;
        let k = key(tool, args);
        // A search another chat ran is not this chat's (#85, A9: one
        // chat's prefetched results were injected into another).
        let chat = self.current_chat();
        let same_chat = |e: &Entry| {
            !per_chat(tool)
                || match (&e.chat, &chat) {
                    (Some(a), Some(b)) => a == b,
                    _ => true,
                }
        };
        let fresh = |e: &Entry| {
            e.tool == tool && now().saturating_sub(e.at) < ttl.as_secs() && same_chat(e)
        };
        let found = {
            let mem = self.mem.lock().unwrap_or_else(|e| e.into_inner());
            mem.get(&k).filter(|e| fresh(e)).cloned()
        };
        let found = found.or_else(|| {
            if !on_disk(tool) {
                return None;
            }
            let p = self.path(&k)?;
            let e: Entry = serde_json::from_str(&std::fs::read_to_string(&p).ok()?).ok()?;
            if !same_chat(&e) {
                crate::diag::note(&format!(
                    "cache: `{tool}` from another chat not reused (searches are kept per chat)"
                ));
                return None;
            }
            if !fresh(&e) {
                let _ = std::fs::remove_file(&p);
                return None;
            }
            self.mem
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(k.clone(), e.clone());
            Some(e)
        })?;
        Some(Hit {
            age: Duration::from_secs(now().saturating_sub(found.at)),
            output: ToolOutput {
                content: found.content,
                evidence_refs: found.evidence_refs,
                meta: found.meta,
            },
        })
    }

    /// Keeps a successful result, when the tool is cached.
    pub fn put(&self, tool: &str, args: &Value, out: &ToolOutput) {
        if disabled() || ttl(tool).is_none() || out.content.trim().is_empty() {
            return;
        }
        let k = key(tool, args);
        let e = Entry {
            tool: tool.to_owned(),
            at: now(),
            content: out.content.clone(),
            chat: self.current_chat(),
            evidence_refs: out.evidence_refs.clone(),
            meta: out.meta.clone(),
        };
        if on_disk(tool)
            && let Some(p) = self.path(&k)
            && let Some(dir) = p.parent()
            && std::fs::create_dir_all(dir).is_ok()
            && let Ok(text) = serde_json::to_string(&e)
        {
            let _ = std::fs::write(&p, text);
        }
        self.mem
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(k, e);
    }

    /// Forgets the cached local reads (after a write, a rename, a command).
    pub fn forget_local(&self) {
        self.mem
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|_, e| !local(&e.tool));
    }

    /// How many results the chat holds in memory.
    pub fn len(&self) -> usize {
        self.mem.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Tools whose cached results belong to the chat that ran them (#85, A9):
/// searches, whose results a chat's answer is built on.
fn per_chat(tool: &str) -> bool {
    matches!(tool, "web_search" | "scholar_search" | "paper_snowball")
}

/// `TERN_NO_TOOL_CACHE=1` turns the cache off.
fn disabled() -> bool {
    std::env::var_os("TERN_NO_TOOL_CACHE").is_some_and(|v| v != "0" && !v.is_empty())
}

/// "cached 4 min ago" for a card.
pub fn age_line(age: Duration) -> String {
    let s = age.as_secs();
    if s < 60 {
        "cached just now".to_owned()
    } else if s < 3600 {
        format!("cached {} min ago", s / 60)
    } else {
        format!("cached {} h ago", s / 3600)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn keys_ignore_order_spacing_and_fragments() {
        assert_eq!(
            key("web_search", &json!({"query": "  rag   survey ", "n": 5})),
            key("web_search", &json!({"n": 5, "query": "rag survey"}))
        );
        assert_eq!(
            key("read_url", &json!({"url": "https://x.org/a/#top"})),
            key("read_url", &json!({"url": "https://x.org/a"}))
        );
        assert_ne!(
            key("read_url", &json!({"url": "https://x.org/a"})),
            key("web_search", &json!({"url": "https://x.org/a"}))
        );
        assert_eq!(
            normalize(&json!({"url": "https://x.org/"}))["url"],
            "https://x.org/"
        );
    }

    #[test]
    fn searches_are_kept_per_chat_and_reads_are_shared() {
        let dir = tempfile::tempdir().unwrap();
        let a = ToolCache::with_dir(dir.path());
        a.set_chat("chat-a");
        let q = json!({"query": "flare retrieval"});
        let page = json!({"url": "https://arxiv.org/abs/2305.06983"});
        a.put("web_search", &q, &ToolOutput::text("[S1] results"));
        a.put("read_url", &page, &ToolOutput::text("page text"));
        assert!(a.get("web_search", &q).is_some(), "same chat");
        let b = ToolCache::with_dir(dir.path());
        b.set_chat("chat-b");
        assert!(b.get("web_search", &q).is_none(), "another chat's search");
        assert!(b.get("read_url", &page).is_some(), "a page read is shared");
    }

    #[test]
    fn only_reads_are_cached_and_changes_empty_local_reads() {
        let c = ToolCache::memory();
        let out = ToolOutput::text("[S1] result");
        c.put("web_search", &json!({"query": "q"}), &out);
        c.put(
            "file_read",
            &json!({"path": "a.md"}),
            &ToolOutput::text("old"),
        );
        c.put(
            "file_write",
            &json!({"path": "a.md"}),
            &ToolOutput::text("saved"),
        );
        c.put(
            "run_command",
            &json!({"command": "ls"}),
            &ToolOutput::text("x"),
        );
        c.put(
            "web_search",
            &json!({"query": "empty"}),
            &ToolOutput::text("  "),
        );
        assert_eq!(c.len(), 2);
        let hit = c.get("web_search", &json!({"query": " q "})).unwrap();
        assert_eq!(hit.output.content, "[S1] result");
        assert!(c.get("file_write", &json!({"path": "a.md"})).is_none());
        assert!(c.get("file_read", &json!({"path": "a.md"})).is_some());
        assert!(changes_files("file_write") && !changes_files("web_search"));
        c.forget_local();
        assert!(c.get("file_read", &json!({"path": "a.md"})).is_none());
        assert!(c.get("web_search", &json!({"query": "q"})).is_some());
        assert_eq!(age_line(Duration::from_secs(300)), "cached 5 min ago");
    }

    #[test]
    fn web_results_survive_on_disk_and_expire() {
        let dir =
            std::env::temp_dir().join(format!("tern-toolcache-{}-{}", std::process::id(), now()));
        let a = ToolCache::with_dir(&dir);
        a.put(
            "read_url",
            &json!({"url": "https://x.org/p"}),
            &ToolOutput::text("page"),
        );
        a.put(
            "file_read",
            &json!({"path": "a"}),
            &ToolOutput::text("local"),
        );
        // A second chat finds the page, not the local read.
        let b = ToolCache::with_dir(&dir);
        assert_eq!(
            b.get("read_url", &json!({"url": "https://x.org/p#x"}))
                .unwrap()
                .output
                .content,
            "page"
        );
        assert!(b.get("file_read", &json!({"path": "a"})).is_none());
        // An old entry on disk is dropped.
        let k = key("read_url", &json!({"url": "https://x.org/old"}));
        std::fs::write(
            dir.join(format!("{k}.json")),
            serde_json::to_string(&Entry {
                tool: "read_url".into(),
                at: now() - 7 * 3600,
                content: "stale".into(),
                evidence_refs: Vec::new(),
                meta: Value::Null,
            })
            .unwrap(),
        )
        .unwrap();
        assert!(
            b.get("read_url", &json!({"url": "https://x.org/old"}))
                .is_none()
        );
        assert!(!dir.join(format!("{k}.json")).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
