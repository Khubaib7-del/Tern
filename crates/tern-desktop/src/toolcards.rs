//! Tool cards (#75), GPUI-free: every tool call in a run as one card that
//! moves through a state machine, grouped by the model step that asked
//! for it, coloured by tool family.
//!
//! | Event | Card state |
//! | --- | --- |
//! | `tool.queued` | Queued (the model asked; not started yet) |
//! | `approval.requested` | Waiting for approval (yellow outline, pulse) |
//! | `approval.answered` (declined), `policy.decided` (refused), `sandbox.denied` | Failed |
//! | `tool.started` | Running |
//! | `tool.executed` | Done |
//! | `tool.failed`, `tool.rejected` | Failed |
//! | the run stopped with the card still open | Cancelled |
//!
//! Events carry the call's number (`call`) from #75 on; older runs have
//! none, and their finished calls show as Done or Failed cards.

use std::time::Duration;

use serde_json::Value;
use tern_agent::{Run, RunEnd};

/// Where a card is in its life.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CardState {
    Queued,
    Running,
    Waiting,
    Done,
    Failed,
    Cancelled,
}

impl CardState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Queued => "Queued",
            Self::Running => "Running",
            Self::Waiting => "Waiting for you",
            Self::Done => "Done",
            Self::Failed => "Failed",
            Self::Cancelled => "Cancelled",
        }
    }

    /// Whether the card can still change.
    pub fn open(self) -> bool {
        matches!(self, Self::Queued | Self::Running | Self::Waiting)
    }
}

/// A tool's family: its colour on the card's stripe and status dot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Family {
    Search,
    Read,
    Write,
    Command,
    LocalModel,
    Verify,
    Git,
    SubAgent,
    Other,
}

impl Family {
    pub fn of(tool: &str) -> Self {
        match tool {
            "web_search" | "scholar_search" | "vault_search" | "file_search" => Self::Search,
            "read_url" | "file_read" | "files_list" | "vault_read" | "read_result"
            | "load_skill" => Self::Read,
            "file_write" | "vault_write" | "export" | "show_in_panel" => Self::Write,
            "run_command" | "run_python" => Self::Command,
            "transcribe" | "describe_image" | "ocr" | "read_aloud" => Self::LocalModel,
            "verify_claims" | "check_claims" => Self::Verify,
            t if t.starts_with("git") => Self::Git,
            t if t.starts_with("sub_") || t == "spawn_agent" => Self::SubAgent,
            _ => Self::Other,
        }
    }

    /// The family's hue and saturation (HSL, 0..1), for light and dark
    /// themes alike. Restrained: no lime, pink or purple.
    pub fn hue(self) -> (f32, f32) {
        match self {
            Self::Search => (215. / 360., 0.70),
            Self::Read => (175. / 360., 0.50),
            Self::Write => (38. / 360., 0.80),
            Self::Command => (205. / 360., 0.18),
            Self::LocalModel => (150. / 360., 0.40),
            Self::Verify => (46. / 360., 0.75),
            Self::Git => (22. / 360., 0.75),
            Self::SubAgent => (195. / 360., 0.55),
            Self::Other => (220. / 360., 0.08),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Search => "Search",
            Self::Read => "Read",
            Self::Write => "Write",
            Self::Command => "Command",
            Self::LocalModel => "Local model",
            Self::Verify => "Verify",
            Self::Git => "Git",
            Self::SubAgent => "Sub-agent",
            Self::Other => "Tool",
        }
    }
}

/// One tool call.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolCard {
    /// The call's number in its turn (synthetic for old runs).
    pub call: u32,
    /// The model step that asked for it.
    pub step: u32,
    pub tool: String,
    pub family: Family,
    pub state: CardState,
    pub args: Value,
    /// What the call is about, in a few words (a query, a path, a command).
    pub title: String,
    /// The output's head and tail (at most ~3,000 characters).
    pub output: String,
    /// Why it failed, was refused or declined.
    pub error: Option<String>,
    /// The full output's id for `read_result`, when it was cut.
    pub saved_as: Option<String>,
    /// The output's full length, in characters.
    pub chars: Option<u64>,
    /// When it started and ended, from the start of the turn.
    pub started: Option<Duration>,
    pub ended: Option<Duration>,
    /// The Terminal tab's run for a command.
    pub terminal_run: Option<u64>,
    pub exit: Option<i64>,
    /// Why it waited for approval.
    pub asked_because: Option<String>,
}

impl ToolCard {
    fn new(call: u32, step: u32, tool: &str, args: &Value) -> Self {
        Self {
            call,
            step,
            tool: tool.to_owned(),
            family: Family::of(tool),
            state: CardState::Queued,
            args: args.clone(),
            title: title_of(tool, args),
            output: String::new(),
            error: None,
            saved_as: None,
            chars: None,
            started: None,
            ended: None,
            terminal_run: None,
            exit: None,
            asked_because: None,
        }
    }

    /// How long it ran, when it finished.
    pub fn took(&self) -> Option<Duration> {
        match (self.started, self.ended) {
            (Some(a), Some(b)) if b >= a => Some(b - a),
            _ => None,
        }
    }

    /// The arguments as short lines (`key: value`), long values cut.
    pub fn arg_lines(&self) -> Vec<(String, String)> {
        let Some(obj) = self.args.as_object() else {
            return Vec::new();
        };
        obj.iter()
            .map(|(k, v)| {
                let s = match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                (k.clone(), s)
            })
            .collect()
    }
}

/// The few words a card's title shows for a call.
pub fn title_of(tool: &str, args: &Value) -> String {
    let s = |k: &str| args.get(k).and_then(Value::as_str).map(str::trim);
    let t = match tool {
        "run_python" => s("code").map(|c| {
            let first = c.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
            let n = c.lines().count();
            if n > 1 {
                format!("{first} (+{} lines)", n - 1)
            } else {
                first.to_owned()
            }
        }),
        "show_in_panel" => s("tab").map(|t| match s("path") {
            Some(p) => format!("{t}: {p}"),
            None => t.to_owned(),
        }),
        _ => s("command")
            .or_else(|| s("query"))
            .or_else(|| s("url"))
            .or_else(|| s("path"))
            .or_else(|| s("name"))
            .map(str::to_owned),
    };
    let t = t.unwrap_or_default();
    if t.chars().count() > 90 {
        format!("{}…", t.chars().take(89).collect::<String>())
    } else {
        t
    }
}

/// The cards one model step asked for.
#[derive(Clone, Debug, PartialEq)]
pub struct StepGroup {
    pub step: u32,
    pub cards: Vec<ToolCard>,
}

impl StepGroup {
    /// The state the folded row shows: the most pressing card's.
    pub fn state(&self) -> CardState {
        let has = |s: CardState| self.cards.iter().any(|c| c.state == s);
        [
            CardState::Waiting,
            CardState::Running,
            CardState::Failed,
            CardState::Queued,
            CardState::Cancelled,
        ]
        .into_iter()
        .find(|s| has(*s))
        .unwrap_or(CardState::Done)
    }

    /// Whether the group opens by itself: something waits, runs or
    /// failed, or a card writes or runs a command.
    pub fn opens_by_itself(&self) -> bool {
        matches!(
            self.state(),
            CardState::Waiting | CardState::Running | CardState::Failed
        ) || self
            .cards
            .iter()
            .any(|c| matches!(c.family, Family::Write | Family::Command))
    }

    /// "web_search, read_url ×3".
    pub fn tools_line(&self) -> String {
        let mut out: Vec<(String, usize)> = Vec::new();
        for c in &self.cards {
            match out.iter_mut().find(|(t, _)| *t == c.tool) {
                Some((_, n)) => *n += 1,
                None => out.push((c.tool.clone(), 1)),
            }
        }
        out.into_iter()
            .map(|(t, n)| if n > 1 { format!("{t} ×{n}") } else { t })
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// The run's tool calls as cards, grouped by step, in order.
pub fn cards_of(run: &Run) -> Vec<StepGroup> {
    let mut cards: Vec<ToolCard> = Vec::new();
    let mut step: u32 = 0;
    let mut synthetic: u32 = 100_000;
    for s in &run.steps {
        let p = &s.payload;
        let tool = p["tool"].as_str().unwrap_or_default();
        // Deep research workers run their own loops; their calls show in
        // the Workers view.
        if p.get("worker").is_some_and(|w| !w.is_null()) {
            continue;
        }
        let call = p["call"].as_u64().map(|c| c as u32);
        let find = |cards: &[ToolCard], call: u32| cards.iter().position(|c| c.call == call);
        match s.event.as_str() {
            "model.replied" => step += 1,
            "tool.queued" => {
                let st = p["step"].as_u64().map(|v| v as u32).unwrap_or(step);
                for c in p["calls"].as_array().into_iter().flatten() {
                    let (Some(n), Some(t)) = (c["call"].as_u64(), c["tool"].as_str()) else {
                        continue;
                    };
                    if find(&cards, n as u32).is_none() {
                        cards.push(ToolCard::new(n as u32, st, t, &c["args"]));
                    }
                }
            }
            "policy.decided" | "approval.requested" | "approval.answered" | "tool.started"
            | "tool.executed" | "tool.failed" | "tool.rejected" | "sandbox.denied" => {
                if tool.is_empty() {
                    continue;
                }
                let n = call.unwrap_or_else(|| {
                    // Old runs: one card per finished call.
                    if matches!(s.event.as_str(), "tool.executed" | "tool.failed") {
                        synthetic += 1;
                        synthetic
                    } else {
                        0
                    }
                });
                if n == 0 {
                    continue;
                }
                let at = match find(&cards, n) {
                    Some(i) => i,
                    None => {
                        cards.push(ToolCard::new(n, step.max(1), tool, &p["args"]));
                        cards.len() - 1
                    }
                };
                let c = &mut cards[at];
                if !p["args"].is_null() && c.args.is_null() {
                    c.args = p["args"].clone();
                    c.title = title_of(tool, &c.args);
                }
                match s.event.as_str() {
                    "policy.decided" => {
                        if p["decision"]["decision"] == "deny" {
                            c.state = CardState::Failed;
                            c.error = p["decision"]["reason"].as_str().map(str::to_owned);
                            c.ended = Some(s.at);
                        }
                    }
                    "approval.requested" => {
                        c.state = CardState::Waiting;
                        c.asked_because = p["summary"].as_str().map(str::to_owned);
                    }
                    "approval.answered" => {
                        if p["answer"] == "deny" {
                            c.state = CardState::Failed;
                            c.error = Some("You declined it.".into());
                            c.ended = Some(s.at);
                        } else if c.state == CardState::Waiting {
                            c.state = CardState::Queued;
                        }
                    }
                    "tool.started" => {
                        c.state = CardState::Running;
                        c.started = Some(s.at);
                    }
                    "tool.executed" => {
                        // A command that was killed or ran out of time
                        // did not finish, whatever its output says.
                        match p["meta"]["state"].as_str() {
                            Some("killed") => {
                                c.state = CardState::Cancelled;
                                c.error = Some("Killed before it finished.".into());
                            }
                            Some("timed out") => {
                                c.state = CardState::Failed;
                                c.error = Some("Hit its time limit.".into());
                            }
                            Some("could not start") => {
                                c.state = CardState::Failed;
                                c.error = Some("The program could not start.".into());
                            }
                            _ => c.state = CardState::Done,
                        }
                        c.started.get_or_insert(s.at);
                        c.ended = Some(s.at);
                        c.output = p["output"].as_str().unwrap_or_default().to_owned();
                        c.saved_as = p["saved_as"].as_str().map(str::to_owned);
                        c.chars = p["chars"].as_u64();
                        c.terminal_run = p["meta"]["run"].as_u64();
                        c.exit = p["meta"]["exit"].as_i64();
                    }
                    _ => {
                        c.state = CardState::Failed;
                        c.ended = Some(s.at);
                        c.error = p["reason"]
                            .as_str()
                            .map(str::to_owned)
                            .or_else(|| p["error"]["message"].as_str().map(str::to_owned))
                            .or_else(|| p["error"].as_str().map(str::to_owned))
                            .or_else(|| Some(s.summary.clone()));
                    }
                }
            }
            _ => {}
        }
    }
    if run.end.is_some() {
        for c in cards.iter_mut().filter(|c| c.state.open()) {
            c.state = CardState::Cancelled;
            if matches!(run.end, Some(RunEnd::Stopped(_))) {
                c.error
                    .get_or_insert_with(|| "The run stopped first.".into());
            }
        }
    }
    let mut groups: Vec<StepGroup> = Vec::new();
    for c in cards {
        match groups.iter_mut().find(|g| g.step == c.step) {
            Some(g) => g.cards.push(c),
            None => groups.push(StepGroup {
                step: c.step,
                cards: vec![c],
            }),
        }
    }
    groups.sort_by_key(|g| g.step);
    groups
}

/// The line a waiting card shows. The approval card under it says what
/// the call does, so a long reason (a tool's description) is not repeated.
pub fn waiting_line(why: &str) -> String {
    let why = why.trim();
    if why.is_empty() || why.chars().count() > 90 {
        "Needs your approval below.".to_owned()
    } else {
        format!("Needs your approval: {why}")
    }
}

/// The first `head` and last `tail` lines of a tool's output, with a line
/// saying how many were left out between them.
pub fn preview_lines(text: &str, head: usize, tail: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= head + tail + 1 {
        return text.trim_end().to_owned();
    }
    let hidden = lines.len() - head - tail;
    let mut out = lines[..head].join("\n");
    out.push_str(&format!(
        "\n… {hidden} more lines (Terminal or Full output) …\n"
    ));
    out.push_str(&lines[lines.len() - tail..].join("\n"));
    out
}

/// A file name whose logo stands for a program (`pandoc` → a Markdown
/// file's), for command cards and Terminal rows.
pub fn program_logo_name(command: &str) -> &'static str {
    let first = command.split_whitespace().next().unwrap_or("");
    match tern_agent::sandbox::program_stem(first).as_str() {
        "python" | "python3" | "py" | "uv" => "x.py",
        "pandoc" => "x.md",
        "typst" => "x.typ",
        "git" => ".gitignore",
        "node" => "x.js",
        "rscript" => "x.r",
        "ffmpeg" | "ffprobe" => "x.mp4",
        "magick" | "tesseract" | "pdftoppm" => "x.png",
        "qpdf" | "pdftotext" | "pdfinfo" => "x.pdf",
        "latexmk" | "pdflatex" | "xelatex" | "lualatex" | "bibtex" | "biber" => "x.tex",
        "sqlite3" => "x.sqlite",
        "jq" => "x.json",
        "dot" => "x.dot",
        "mmdc" => "x.mmd",
        "soffice" | "libreoffice" => "x.docx",
        _ => "x.sh",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tern_core::{EffectClass, EventEnvelope, SessionId};

    fn ev(kind: &str, payload: Value) -> EventEnvelope {
        EventEnvelope::new(SessionId::new(), kind, EffectClass::Analysis, kind, payload)
    }

    fn run(events: &[EventEnvelope]) -> Run {
        let mut r = Run::new();
        for e in events {
            r.push(e);
        }
        r
    }

    #[test]
    fn a_call_moves_through_its_states() {
        let mut evs = vec![
            ev("model.replied", json!({})),
            ev(
                "tool.queued",
                json!({"step": 1, "calls": [
                    {"call": 1, "tool": "run_command", "args": {"command": "pandoc a.md -o a.docx"}},
                    {"call": 2, "tool": "file_write", "args": {"path": "b.md", "text": "x"}}
                ]}),
            ),
        ];
        let g = cards_of(&run(&evs));
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].cards.len(), 2);
        assert!(g[0].cards.iter().all(|c| c.state == CardState::Queued));
        assert_eq!(g[0].cards[0].family, Family::Command);
        assert_eq!(g[0].cards[0].title, "pandoc a.md -o a.docx");
        evs.push(ev(
            "approval.requested",
            json!({"tool": "run_command", "call": 1, "summary": "Run it"}),
        ));
        let g = cards_of(&run(&evs));
        assert_eq!(g[0].cards[0].state, CardState::Waiting);
        assert_eq!(g[0].state(), CardState::Waiting);
        assert!(g[0].opens_by_itself());
        evs.push(ev(
            "approval.answered",
            json!({"tool": "run_command", "call": 1, "answer": "allow_once"}),
        ));
        evs.push(ev(
            "tool.started",
            json!({"tool": "run_command", "call": 1}),
        ));
        assert_eq!(cards_of(&run(&evs))[0].cards[0].state, CardState::Running);
        evs.push(ev(
            "tool.executed",
            json!({"tool": "run_command", "call": 1, "output": "Exit code 0", "meta": {"run": 7, "exit": 0}}),
        ));
        let c = &cards_of(&run(&evs))[0].cards[0];
        assert_eq!(c.state, CardState::Done);
        assert_eq!(c.terminal_run, Some(7));
        assert_eq!(c.output, "Exit code 0");
        evs.push(ev(
            "approval.requested",
            json!({"tool": "file_write", "call": 2}),
        ));
        evs.push(ev(
            "approval.answered",
            json!({"tool": "file_write", "call": 2, "answer": "deny"}),
        ));
        let c = &cards_of(&run(&evs))[0].cards[1];
        assert_eq!(c.state, CardState::Failed);
        assert_eq!(c.error.as_deref(), Some("You declined it."));
    }

    #[test]
    fn open_cards_are_cancelled_when_the_run_ends() {
        let evs = vec![
            ev("model.replied", json!({})),
            ev(
                "tool.queued",
                json!({"step": 1, "calls": [{"call": 1, "tool": "run_python", "args": {"code": "1+1"}}]}),
            ),
            ev("tool.started", json!({"tool": "run_python", "call": 1})),
            ev("run.cancelled", json!({"state": "cancelled"})),
        ];
        let r = run(&evs);
        if r.end.is_some() {
            assert_eq!(cards_of(&r)[0].cards[0].state, CardState::Cancelled);
        }
    }

    #[test]
    fn denials_and_old_runs() {
        let evs = vec![
            ev("model.replied", json!({})),
            ev(
                "sandbox.denied",
                json!({"tool": "run_command", "call": 1, "args": {"command": "bash"}, "reason": "not on the list"}),
            ),
            ev(
                "tool.executed",
                json!({"tool": "web_search", "args": {"query": "q"}}),
            ),
            ev(
                "tool.executed",
                json!({"tool": "web_search", "worker": 2, "args": {"query": "w"}}),
            ),
        ];
        let g = cards_of(&run(&evs));
        let all: Vec<&ToolCard> = g.iter().flat_map(|g| &g.cards).collect();
        assert_eq!(all.len(), 2, "worker calls stay out");
        assert_eq!(all[0].state, CardState::Failed);
        assert_eq!(all[0].error.as_deref(), Some("not on the list"));
        assert_eq!(all[1].state, CardState::Done);
        assert_eq!(g[0].tools_line(), "run_command, web_search");
    }

    #[test]
    fn killed_commands_are_cancelled_and_long_output_folds() {
        let r = run(&[
            ev(
                "tool.queued",
                json!({"step": 1, "calls": [{"call": 1, "tool": "run_command", "args": {"command": "python3 x.py"}}]}),
            ),
            ev("tool.started", json!({"tool": "run_command", "call": 1})),
            ev(
                "tool.executed",
                json!({"tool": "run_command", "call": 1, "output": "The command was killed after 2.0 s.", "meta": {"state": "killed", "run": 3}}),
            ),
        ]);
        let c = &cards_of(&r)[0].cards[0];
        assert_eq!(c.state, CardState::Cancelled);
        assert_eq!(c.terminal_run, Some(3));
        let long: String = (1..=40).map(|i| format!("line {i}\n")).collect();
        let p = preview_lines(&long, 8, 4);
        assert!(p.starts_with("line 1\n"));
        assert!(p.contains("… 28 more lines"));
        assert!(p.ends_with("line 40"));
        assert_eq!(preview_lines("a\nb\n", 8, 4), "a\nb");
        assert_eq!(waiting_line("a pipe"), "Needs your approval: a pipe");
        assert_eq!(waiting_line(&"x".repeat(200)), "Needs your approval below.");
    }

    #[test]
    fn families_and_logos() {
        assert_eq!(Family::of("read_url"), Family::Read);
        assert_eq!(Family::of("mcp__x__y"), Family::Other);
        assert_eq!(program_logo_name("pandoc a.md"), "x.md");
        assert_eq!(program_logo_name("C:\\Python\\python.exe -c 1"), "x.py");
        assert!(title_of("run_python", &json!({"code": "a = 1\nb = 2"})).contains("+1 lines"));
    }
}
