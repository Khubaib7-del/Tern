//! What the agent's workbench is doing right now, for the desktop's
//! right rail (#75): commands running in the Terminal tab, a file being
//! written (streamed from the tool call's arguments as the model types
//! them), and panel requests from `show_in_panel`.
//!
//! One process-wide board with a version number: the window reads it when
//! the number moved, so a quiet board costs one atomic load per frame.
//! Nothing here is persisted; the event log keeps what happened.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Most runs the board keeps (oldest dropped first).
const MAX_RUNS: usize = 40;
/// Most output kept per run for the Terminal tab, in bytes.
const MAX_OUTPUT: usize = 512 * 1024;

/// Which pipe a piece of output came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stream {
    Stdout,
    Stderr,
    /// A line Tern wrote itself (the sandbox's note, a kill).
    Tern,
}

/// How a run is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunState {
    Running,
    Exited,
    Killed,
    TimedOut,
    /// The sandbox refused it before it started.
    Denied,
    /// It could not start (program missing).
    Failed,
}

impl RunState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Exited => "exited",
            Self::Killed => "killed",
            Self::TimedOut => "timed out",
            Self::Denied => "denied by the sandbox",
            Self::Failed => "could not start",
        }
    }
}

/// One command in the Terminal tab.
#[derive(Clone, Debug)]
pub struct TermRun {
    pub id: u64,
    /// What ran, as one line.
    pub command: String,
    /// Where it ran.
    pub cwd: String,
    /// The sandbox's one-line summary ("network off · 2 GB · 120 s").
    pub sandbox: String,
    pub started: Instant,
    pub output: Vec<(Stream, String)>,
    /// Bytes of output dropped past the cap.
    pub dropped: usize,
    pub state: RunState,
    pub exit: Option<i32>,
    pub took: Option<Duration>,
}

impl TermRun {
    /// How long it ran, or has been running.
    pub fn elapsed(&self) -> Duration {
        self.took.unwrap_or_else(|| self.started.elapsed())
    }

    fn bytes(&self) -> usize {
        self.output.iter().map(|(_, t)| t.len()).sum()
    }
}

/// A file the model is writing, shown in the Files tab as it arrives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WritePreview {
    /// The path as the model gave it (relative to the chat's folder).
    pub path: String,
    /// The text so far.
    pub text: String,
    /// The tool call's arguments are complete.
    pub done: bool,
}

/// A request from `show_in_panel`: which tab, and a file to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PanelRequest {
    pub seq: u64,
    pub tab: String,
    pub path: Option<String>,
}

type Killer = Arc<dyn Fn() + Send + Sync>;

#[derive(Default)]
struct Board {
    runs: Vec<TermRun>,
    killers: HashMap<u64, Killer>,
    preview: Option<WritePreview>,
    panel: Option<PanelRequest>,
    next: u64,
}

static VERSION: AtomicU64 = AtomicU64::new(0);

fn board() -> &'static Mutex<Board> {
    static B: OnceLock<Mutex<Board>> = OnceLock::new();
    B.get_or_init(Mutex::default)
}

fn with<T>(f: impl FnOnce(&mut Board) -> T) -> T {
    let mut b = board().lock().unwrap_or_else(|e| e.into_inner());
    let out = f(&mut b);
    VERSION.fetch_add(1, Ordering::Relaxed);
    out
}

/// Moves on every change; compare it with the last one read.
pub fn version() -> u64 {
    VERSION.load(Ordering::Relaxed)
}

/// A copy of the Terminal tab's runs, oldest first.
pub fn runs() -> Vec<TermRun> {
    board()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .runs
        .clone()
}

/// Whether any command is still running.
pub fn any_running() -> bool {
    board()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .runs
        .iter()
        .any(|r| r.state == RunState::Running)
}

/// Adds a run; returns its id.
pub fn start_run(command: &str, cwd: &str, sandbox: &str) -> u64 {
    with(|b| {
        b.next += 1;
        let id = b.next;
        b.runs.push(TermRun {
            id,
            command: command.to_owned(),
            cwd: cwd.to_owned(),
            sandbox: sandbox.to_owned(),
            started: Instant::now(),
            output: Vec::new(),
            dropped: 0,
            state: RunState::Running,
            exit: None,
            took: None,
        });
        if b.runs.len() > MAX_RUNS {
            let cut = b.runs.len() - MAX_RUNS;
            b.runs.drain(..cut);
        }
        id
    })
}

/// Appends output to a run (joined to the last piece from the same pipe).
pub fn push_output(id: u64, stream: Stream, text: &str) {
    if text.is_empty() {
        return;
    }
    with(|b| {
        let Some(r) = b.runs.iter_mut().find(|r| r.id == id) else {
            return;
        };
        if r.bytes() + text.len() > MAX_OUTPUT {
            r.dropped += text.len();
            return;
        }
        match r.output.last_mut() {
            Some((s, t)) if *s == stream => t.push_str(text),
            _ => r.output.push((stream, text.to_owned())),
        }
    });
}

/// Ends a run.
pub fn finish_run(id: u64, state: RunState, exit: Option<i32>) {
    with(|b| {
        b.killers.remove(&id);
        if let Some(r) = b.runs.iter_mut().find(|r| r.id == id) {
            r.state = state;
            r.exit = exit;
            r.took = Some(r.started.elapsed());
        }
    });
}

/// How to stop run `id` (the sandbox kills its whole process tree).
pub fn set_killer(id: u64, kill: Killer) {
    with(|b| {
        b.killers.insert(id, kill);
    });
}

/// The Terminal tab's kill button. Returns whether the run was live.
pub fn kill(id: u64) -> bool {
    let k = board()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .killers
        .get(&id)
        .cloned();
    match k {
        Some(k) => {
            crate::diag::note(&format!("sandbox: kill requested for run {id}"));
            k();
            true
        }
        None => false,
    }
}

/// Clears finished runs from the Terminal tab.
pub fn clear_finished() {
    with(|b| b.runs.retain(|r| r.state == RunState::Running));
}

/// Shows a file being written; `done` once the arguments are complete.
pub fn preview_write(path: &str, text: &str, done: bool) {
    with(|b| {
        b.preview = Some(WritePreview {
            path: path.to_owned(),
            text: text.to_owned(),
            done,
        });
    });
}

/// The file being written, if any.
pub fn preview() -> Option<WritePreview> {
    board()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .preview
        .clone()
}

pub fn clear_preview() {
    with(|b| b.preview = None);
}

/// A file was renamed from `from` to `to` (paths relative to the chat's
/// folder): a preview of `from` follows it to `to` (#79: after a script
/// wrote notes.md and renamed it to hello.md, the panel kept "notes.md
/// Not written"). Returns whether the preview moved.
pub fn rename_preview(from: &str, to: &str) -> bool {
    let norm = |p: &str| {
        let p = p.trim().replace('\\', "/");
        p.strip_prefix("./").unwrap_or(&p).to_owned()
    };
    let (from, to) = (norm(from), norm(to));
    let mut moved = false;
    with(|b| {
        if let Some(w) = b.preview.as_mut()
            && (norm(&w.path) == from
                || (cfg!(windows) && norm(&w.path).eq_ignore_ascii_case(&from)))
        {
            w.path = to.clone();
            moved = true;
        }
    });
    moved
}

/// Asks the window to show a tab (and a file in it).
pub fn request_panel(tab: &str, path: Option<&str>) {
    with(|b| {
        b.next += 1;
        b.panel = Some(PanelRequest {
            seq: b.next,
            tab: tab.to_owned(),
            path: path.map(str::to_owned),
        });
    });
}

/// The latest panel request, if newer than `after`.
pub fn panel_request(after: u64) -> Option<PanelRequest> {
    board()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .panel
        .clone()
        .filter(|p| p.seq > after)
}

/// Pulls the `path` and the `text` written so far out of a `file_write`
/// call's arguments while they are still arriving as JSON text. Escapes
/// are decoded; a cut escape at the end is left out.
pub fn partial_write_args(json: &str) -> (Option<String>, Option<String>) {
    (partial_string(json, "path"), partial_string(json, "text"))
}

fn partial_string(json: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let at = json.find(&needle)? + needle.len();
    let rest = json[at..].trim_start();
    let rest = rest.strip_prefix(':')?.trim_start();
    let mut chars = rest.strip_prefix('"')?.chars();
    let mut out = String::new();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('b') => out.push('\u{8}'),
                Some('f') => out.push('\u{c}'),
                Some('u') => {
                    let hex: String = chars.by_ref().take(4).collect();
                    if hex.len() < 4 {
                        return Some(out);
                    }
                    if let Some(ch) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                        out.push(ch);
                    }
                }
                Some(other) => out.push(other),
                None => return Some(out),
            },
            c => out.push(c),
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_arguments_give_the_text_so_far() {
        let (p, t) = partial_write_args(
            r##"{"path": "notes/a.md", "text": "# Title\nLine \"one\"\u00e9 and"##,
        );
        assert_eq!(p.as_deref(), Some("notes/a.md"));
        assert_eq!(t.as_deref(), Some("# Title\nLine \"one\"é and"));
        let (p, t) = partial_write_args(r#"{"pa"#);
        assert!(p.is_none() && t.is_none());
        let (_, t) = partial_write_args(r#"{"path":"a","text":"cut \"#);
        assert_eq!(t.as_deref(), Some("cut "));
    }

    #[test]
    fn a_preview_follows_its_file_when_renamed() {
        preview_write("./drafts/notes-79.md", "hello", true);
        assert!(!rename_preview("other.md", "x.md"));
        assert!(rename_preview("drafts\\notes-79.md", "drafts/hello-79.md"));
        let w = preview().unwrap();
        assert_eq!(
            (w.path.as_str(), w.text.as_str()),
            ("drafts/hello-79.md", "hello")
        );
        clear_preview();
        assert!(!rename_preview("drafts/hello-79.md", "y.md"));
    }

    #[test]
    fn runs_stream_and_finish() {
        let id = start_run("pandoc a.md", "/tmp", "network off");
        let v = version();
        push_output(id, Stream::Stdout, "one ");
        push_output(id, Stream::Stdout, "two");
        push_output(id, Stream::Stderr, "warn");
        assert!(version() > v);
        let r = runs().into_iter().find(|r| r.id == id).unwrap();
        assert_eq!(r.output.len(), 2);
        assert_eq!(r.output[0].1, "one two");
        let hit = Arc::new(AtomicU64::new(0));
        let h = hit.clone();
        set_killer(
            id,
            Arc::new(move || {
                h.fetch_add(1, Ordering::Relaxed);
            }),
        );
        assert!(kill(id));
        assert_eq!(hit.load(Ordering::Relaxed), 1);
        finish_run(id, RunState::Killed, None);
        assert!(!kill(id));
        let r = runs().into_iter().find(|r| r.id == id).unwrap();
        assert_eq!(r.state, RunState::Killed);
        assert!(r.took.is_some());
    }
}
