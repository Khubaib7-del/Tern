//! TUI state and key handling. Pure: no terminal, no async, so it is tested directly.

use std::time::{Duration, Instant};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use tern_agent::{ApprovalAnswer, ApprovalRequest, TurnOutcome, Usage};
use tern_core::{Measure, TurnState};

/// One row group in the transcript.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    User(String),
    Assistant(String),
    /// A step the agent took, shown dim: `• Searched 14 sources`.
    Step(String),
    /// Something the user must read: a stop, an error.
    Notice(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Idle,
    Working {
        since: Instant,
        label: String,
    },
    /// The run waits for the user. The working timer keeps its start.
    Approval {
        since: Instant,
        request: ApprovalRequest,
    },
}

/// What a `/` command does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SlashKind {
    Help,
    Quit,
    /// An MCP prompt: its server, its name, and its argument names
    /// (required ones first).
    Prompt {
        server: String,
        prompt: String,
        args: Vec<String>,
    },
}

/// One `/` command the composer completes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlashItem {
    /// What follows `/`: `help`, `notes:summarize`.
    pub name: String,
    pub detail: String,
    /// `Tern` or the MCP server's name.
    pub source: String,
    pub kind: SlashKind,
}

impl SlashItem {
    /// Tern's own commands.
    pub fn builtins() -> Vec<Self> {
        vec![
            Self {
                name: "help".into(),
                detail: "Keyboard shortcuts".into(),
                source: "Tern".into(),
                kind: SlashKind::Help,
            },
            Self {
                name: "quit".into(),
                detail: "Leave the chat".into(),
                source: "Tern".into(),
                kind: SlashKind::Quit,
            },
        ]
    }

    /// The prompts the chat's MCP servers offer, as `/server:prompt`.
    pub fn prompts(offers: &[tern_agent::mcp::MenuItem]) -> Vec<Self> {
        offers
            .iter()
            .filter(|i| i.kind == tern_agent::mcp::ItemKind::Prompt)
            .map(|i| {
                let mut args: Vec<&tern_agent::mcp::PromptArg> = i.args.iter().collect();
                args.sort_by_key(|a| !a.required);
                let mut detail = i
                    .description
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ");
                if !args.is_empty() {
                    let list: Vec<String> = args
                        .iter()
                        .map(|a| {
                            if a.required {
                                format!("{}=…", a.name)
                            } else {
                                format!("[{}=…]", a.name)
                            }
                        })
                        .collect();
                    detail = format!("{} {detail}", list.join(" "));
                }
                Self {
                    name: format!("{}:{}", i.server, i.name),
                    detail,
                    source: i.server.clone(),
                    kind: SlashKind::Prompt {
                        server: i.server.clone(),
                        prompt: i.name.clone(),
                        args: args.iter().map(|a| a.name.clone()).collect(),
                    },
                }
            })
            .collect()
    }
}

/// What the run loop should do after a key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    None,
    Send(String),
    /// Ask an MCP server to fill in a prompt; its text goes in the
    /// composer, never straight to the model.
    Prompt {
        server: String,
        prompt: String,
        args: Vec<(String, String)>,
    },
    Cancel,
    Approve(ApprovalAnswer),
    Quit,
}

#[derive(Debug)]
pub struct App {
    pub entries: Vec<Entry>,
    pub input: Vec<char>,
    /// Cursor position in `input`, in chars.
    pub cursor: usize,
    pub status: Status,
    pub provider: String,
    pub model: String,
    pub harness: &'static str,
    /// The web search service the chat can use, if one is set up.
    pub search: Option<&'static str>,
    pub usage: Usage,
    /// Lines scrolled up from the bottom of the transcript. 0 follows new output.
    pub scroll: u16,
    pub show_help: bool,
    pub tick: usize,
    /// Set after Esc or Ctrl+C while working; cleared when the turn ends.
    pub stopping: bool,
    /// The `/` commands: Tern's and the MCP servers' prompts.
    pub slash: Vec<SlashItem>,
}

impl App {
    pub fn new(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            entries: Vec::new(),
            input: Vec::new(),
            cursor: 0,
            status: Status::Idle,
            provider: provider.into(),
            model: model.into(),
            harness: "Research",
            search: None,
            usage: Usage {
                input_tokens: 0,
                output_tokens: 0,
                measure: Measure::Measured,
            },
            scroll: 0,
            show_help: false,
            tick: 0,
            stopping: false,
            slash: SlashItem::builtins(),
        }
    }

    /// The `/` commands matching what is typed, when the composer holds a
    /// single `/word`.
    pub fn slash_matches(&self) -> Vec<&SlashItem> {
        let text = self.input_text();
        let Some(q) = text.strip_prefix('/') else {
            return Vec::new();
        };
        if q.chars().any(char::is_whitespace) {
            return Vec::new();
        }
        let mut m: Vec<&SlashItem> = self
            .slash
            .iter()
            .filter(|i| tern_agent::mention::matches(q, &i.name))
            .collect();
        m.sort_by_key(|i| tern_agent::mention::rank(q, &i.name));
        m.truncate(6);
        m
    }

    /// Runs a typed `/command args…` when its name is one Tern knows;
    /// `None` means it is an ordinary message.
    fn run_slash(&mut self, text: &str) -> Option<Action> {
        let rest = text.strip_prefix('/')?;
        let (name, tail) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
        let item = self.slash.iter().find(|i| i.name == name)?.clone();
        Some(match item.kind {
            SlashKind::Help => {
                self.show_help = true;
                Action::None
            }
            SlashKind::Quit => Action::Quit,
            SlashKind::Prompt {
                server,
                prompt,
                args,
            } => {
                // `k=v` pairs; the other words, joined, fill the first
                // argument not given (required ones come first).
                let mut out: Vec<(String, String)> = Vec::new();
                let mut rest: Vec<&str> = Vec::new();
                for w in tail.split_whitespace() {
                    match w.split_once('=') {
                        Some((k, v)) if args.iter().any(|a| a == k) => {
                            out.push((k.to_owned(), v.to_owned()))
                        }
                        _ => rest.push(w),
                    }
                }
                if !rest.is_empty()
                    && let Some(a) = args.iter().find(|a| !out.iter().any(|(k, _)| k == *a))
                {
                    out.insert(0, (a.clone(), rest.join(" ")));
                }
                Action::Prompt {
                    server,
                    prompt,
                    args: out,
                }
            }
        })
    }

    /// A prompt came back from its server: its text goes in the composer
    /// for the user to read and send (or not), with a note saying whose
    /// text it is.
    pub fn insert_prompt(&mut self, r: Result<tern_agent::mcp::Inserted, String>) {
        match r {
            Ok(ins) => {
                self.entries.push(Entry::Notice(format!(
                    "From MCP server “{}”, prompt “{}”: the server's text is in the composer. Read it, then press Enter to send it, or Ctrl+U to clear it.",
                    ins.server, ins.prompt
                )));
                for f in &ins.flags {
                    self.entries.push(Entry::Step(format!(
                        "⚠ the prompt {}{}",
                        f.reason,
                        if f.excerpt.is_empty() {
                            String::new()
                        } else {
                            format!(": “{}”", f.excerpt)
                        }
                    )));
                }
                self.input = ins.text.chars().filter(|c| *c != '\r').collect();
                self.cursor = self.input.len();
            }
            Err(e) => self.entries.push(Entry::Notice(e)),
        }
    }

    pub fn is_busy(&self) -> bool {
        !matches!(self.status, Status::Idle)
    }

    pub fn input_text(&self) -> String {
        self.input.iter().collect()
    }

    fn insert(&mut self, s: &str) {
        for c in s.chars() {
            // Windows pastes CRLF; a lone CR is a newline too.
            if c == '\r' {
                continue;
            }
            self.input.insert(self.cursor, c);
            self.cursor += 1;
        }
    }

    pub fn on_paste(&mut self, text: &str) {
        if !matches!(self.status, Status::Approval { .. }) {
            self.insert(text);
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) -> Action {
        if key.kind == KeyEventKind::Release {
            return Action::None;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && matches!(key.code, KeyCode::Char('c')) {
            return if self.is_busy() && !self.stopping {
                self.stopping = true;
                Action::Cancel
            } else {
                Action::Quit
            };
        }
        if let Status::Approval { .. } = self.status {
            return match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    Action::Approve(ApprovalAnswer::AllowOnce)
                }
                KeyCode::Char('s') | KeyCode::Char('S') => {
                    Action::Approve(ApprovalAnswer::AllowForSession)
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    Action::Approve(ApprovalAnswer::Deny)
                }
                _ => Action::None,
            };
        }
        if self.show_help {
            if matches!(key.code, KeyCode::Esc | KeyCode::Char('?') | KeyCode::Enter) {
                self.show_help = false;
            }
            return Action::None;
        }
        match key.code {
            KeyCode::Char('d') if ctrl && self.input.is_empty() => Action::Quit,
            KeyCode::Esc if self.is_busy() && !self.stopping => {
                self.stopping = true;
                Action::Cancel
            }
            KeyCode::Esc => Action::None,
            KeyCode::Char('?') if self.input.is_empty() => {
                self.show_help = true;
                Action::None
            }
            KeyCode::Enter
                if key
                    .modifiers
                    .intersects(KeyModifiers::SHIFT | KeyModifiers::ALT) =>
            {
                self.insert("\n");
                Action::None
            }
            KeyCode::Char('j') if ctrl => {
                self.insert("\n");
                Action::None
            }
            KeyCode::Tab => {
                if let Some(first) = self.slash_matches().first() {
                    let done = format!("/{} ", first.name);
                    self.input = done.chars().collect();
                    self.cursor = self.input.len();
                }
                Action::None
            }
            KeyCode::Enter => {
                let text = self.input_text();
                if text.trim_start().starts_with('/')
                    && let Some(a) = self.run_slash(text.trim())
                {
                    if !matches!(a, Action::Quit) {
                        self.input.clear();
                        self.cursor = 0;
                    }
                    return a;
                }
                if self.is_busy() || text.trim().is_empty() {
                    return Action::None;
                }
                self.input.clear();
                self.cursor = 0;
                self.scroll = 0;
                Action::Send(text.trim().to_owned())
            }
            KeyCode::Char('u') if ctrl => {
                self.input.drain(..self.cursor);
                self.cursor = 0;
                Action::None
            }
            KeyCode::Char(c) if !ctrl => {
                let mut b = [0u8; 4];
                self.insert(c.encode_utf8(&mut b));
                Action::None
            }
            KeyCode::Backspace if self.cursor > 0 => {
                self.cursor -= 1;
                self.input.remove(self.cursor);
                Action::None
            }
            KeyCode::Delete if self.cursor < self.input.len() => {
                self.input.remove(self.cursor);
                Action::None
            }
            KeyCode::Left => {
                self.cursor = self.cursor.saturating_sub(1);
                Action::None
            }
            KeyCode::Right => {
                self.cursor = (self.cursor + 1).min(self.input.len());
                Action::None
            }
            KeyCode::Home => {
                self.cursor = 0;
                Action::None
            }
            KeyCode::End => {
                self.cursor = self.input.len();
                Action::None
            }
            KeyCode::PageUp => {
                self.scroll = self.scroll.saturating_add(10);
                Action::None
            }
            KeyCode::PageDown => {
                self.scroll = self.scroll.saturating_sub(10);
                Action::None
            }
            _ => Action::None,
        }
    }

    /// The user sent a message and the turn started.
    pub fn start_turn(&mut self, text: String) {
        self.entries.push(Entry::User(text));
        self.status = Status::Working {
            since: Instant::now(),
            label: "Thinking".into(),
        };
        self.stopping = false;
    }

    /// An event from the run, by type and its one-line summary.
    pub fn on_event(&mut self, kind: &str, summary: &str) {
        let label = match kind {
            "turn.started" | "tool.executed" | "tool.failed" | "tool.rejected" => Some("Thinking"),
            "model.replied" => Some("Reading the reply"),
            "policy.decided" => Some("Checking permissions"),
            "approval.answered" => Some("Working"),
            _ => None,
        };
        if matches!(
            kind,
            "tool.executed"
                | "tool.failed"
                | "tool.rejected"
                | "approval.answered"
                | "loop.suspected"
                // #77: the plan and the task graph, one line each time
                // they change ("Plan · 1 of 3 done: ✓ … · ▸ … · ○ …").
                | "plan.updated"
        ) || (kind == "policy.decided" && !summary.contains("allowed"))
        {
            self.entries.push(Entry::Step(summary.to_owned()));
        }
        if let (Some(l), Status::Working { label, .. }) = (label, &mut self.status) {
            *label = l.to_owned();
        }
    }

    pub fn ask_approval(&mut self, request: ApprovalRequest) {
        let since = match &self.status {
            Status::Working { since, .. } | Status::Approval { since, .. } => *since,
            Status::Idle => Instant::now(),
        };
        self.status = Status::Approval { since, request };
    }

    /// The user answered the dialog; the run carries on.
    pub fn approval_answered(&mut self) {
        if let Status::Approval { since, .. } = self.status {
            self.status = Status::Working {
                since,
                label: "Working".into(),
            };
        }
    }

    pub fn finish_turn(&mut self, out: &TurnOutcome, usage: Usage) {
        self.usage = usage;
        self.status = Status::Idle;
        self.stopping = false;
        if let Some(a) = &out.answer
            && !a.is_empty()
        {
            self.entries.push(Entry::Assistant(a.clone()));
        }
        if out.state != TurnState::Completed
            && let Some(r) = &out.reason
        {
            self.entries.push(Entry::Notice(r.clone()));
        }
    }

    /// Seconds the current run has taken, if one is running.
    pub fn elapsed(&self) -> Option<Duration> {
        match &self.status {
            Status::Working { since, .. } | Status::Approval { since, .. } => Some(since.elapsed()),
            Status::Idle => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tern_core::{EffectClass, TurnId};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }
    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }
    fn typed(app: &mut App, s: &str) {
        for c in s.chars() {
            app.on_key(key(KeyCode::Char(c)));
        }
    }
    fn outcome(state: TurnState, answer: Option<&str>, reason: Option<&str>) -> TurnOutcome {
        TurnOutcome {
            turn_id: TurnId::new(),
            state,
            answer: answer.map(Into::into),
            reason: reason.map(Into::into),
            states: vec![],
            usage: Usage::measured(1, 1),
            model_calls: 1,
            tool_calls: 0,
        }
    }

    #[test]
    fn slash_completes_and_runs_prompts_into_the_composer() {
        use tern_agent::mcp::{ItemKind, MenuItem, PromptArg};
        let mut app = App::new("p", "m");
        app.slash.extend(SlashItem::prompts(&[MenuItem {
            server: "notes".into(),
            kind: ItemKind::Prompt,
            name: "review".into(),
            title: None,
            description: "Reviews code.".into(),
            uri: String::new(),
            mime: None,
            args: vec![
                PromptArg {
                    name: "focus".into(),
                    title: None,
                    description: String::new(),
                    required: false,
                },
                PromptArg {
                    name: "code".into(),
                    title: None,
                    description: String::new(),
                    required: true,
                },
            ],
            http: false,
            target: "notes-mcp".into(),
        }]));
        typed(&mut app, "/rev");
        assert_eq!(app.slash_matches()[0].name, "notes:review");
        app.on_key(key(KeyCode::Tab));
        assert_eq!(app.input_text(), "/notes:review ");
        typed(&mut app, "fn x() focus=names");
        let a = app.on_key(key(KeyCode::Enter));
        assert_eq!(
            a,
            Action::Prompt {
                server: "notes".into(),
                prompt: "review".into(),
                args: vec![
                    ("code".into(), "fn x()".into()),
                    ("focus".into(), "names".into())
                ],
            }
        );
        assert!(app.input.is_empty());
        app.insert_prompt(Ok(tern_agent::mcp::Inserted {
            server: "notes".into(),
            prompt: "review".into(),
            text: "Review this".into(),
            description: None,
            flags: Vec::new(),
        }));
        assert_eq!(app.input_text(), "Review this", "inserted, not sent");
        assert!(matches!(app.entries.last(), Some(Entry::Notice(n)) if n.contains("notes")));
        // An ordinary message starting with / is still a message.
        app.input.clear();
        app.cursor = 0;
        typed(&mut app, "/etc/hosts explain");
        assert!(matches!(app.on_key(key(KeyCode::Enter)), Action::Send(_)));
        typed(&mut app, "/help");
        assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
        assert!(app.show_help);
    }

    #[test]
    fn typing_editing_and_sending() {
        let mut a = App::new("Groq", "llama");
        typed(&mut a, "helo");
        a.on_key(key(KeyCode::Left));
        a.on_key(key(KeyCode::Char('l')));
        assert_eq!(a.input_text(), "hello");
        a.on_key(key(KeyCode::End));
        a.on_key(key(KeyCode::Backspace));
        assert_eq!(a.input_text(), "hell");
        assert_eq!(a.on_key(key(KeyCode::Enter)), Action::Send("hell".into()));
        assert!(a.input.is_empty());
    }

    #[test]
    fn empty_input_and_busy_runs_do_not_send() {
        let mut a = App::new("p", "m");
        typed(&mut a, "   ");
        assert_eq!(a.on_key(key(KeyCode::Enter)), Action::None);
        a.start_turn("q".into());
        typed(&mut a, "next");
        assert_eq!(a.on_key(key(KeyCode::Enter)), Action::None);
        assert_eq!(a.input_text(), "   next", "draft is kept while busy");
    }

    #[test]
    fn shift_enter_and_paste_make_newlines() {
        let mut a = App::new("p", "m");
        typed(&mut a, "a");
        a.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
        a.on_paste("b\r\nc");
        assert_eq!(a.input_text(), "a\nb\nc");
    }

    #[test]
    fn esc_and_ctrl_c_stop_a_run_once_then_quit() {
        let mut a = App::new("p", "m");
        assert_eq!(a.on_key(key(KeyCode::Esc)), Action::None);
        a.start_turn("q".into());
        assert_eq!(a.on_key(key(KeyCode::Esc)), Action::Cancel);
        assert_eq!(a.on_key(key(KeyCode::Esc)), Action::None);
        assert_eq!(a.on_key(ctrl('c')), Action::Quit);
        let mut idle = App::new("p", "m");
        assert_eq!(idle.on_key(ctrl('c')), Action::Quit);
        assert_eq!(idle.on_key(ctrl('d')), Action::Quit);
    }

    #[test]
    fn approval_keys_answer_and_nothing_else_types() {
        let mut a = App::new("p", "m");
        a.start_turn("q".into());
        a.ask_approval(ApprovalRequest {
            tool: "publish".into(),
            summary: "Post".into(),
            effect: EffectClass::ExternalSideEffect,
            target: Some("x.example".into()),
            args: json!({}),
        });
        assert_eq!(a.on_key(key(KeyCode::Char('z'))), Action::None);
        a.on_paste("ignored");
        assert!(a.input.is_empty());
        assert_eq!(
            a.on_key(key(KeyCode::Char('s'))),
            Action::Approve(ApprovalAnswer::AllowForSession)
        );
        assert_eq!(
            a.on_key(key(KeyCode::Esc)),
            Action::Approve(ApprovalAnswer::Deny)
        );
        a.approval_answered();
        assert!(matches!(a.status, Status::Working { .. }));
    }

    #[test]
    fn help_toggles_only_from_an_empty_composer() {
        let mut a = App::new("p", "m");
        a.on_key(key(KeyCode::Char('?')));
        assert!(a.show_help);
        a.on_key(key(KeyCode::Esc));
        assert!(!a.show_help);
        typed(&mut a, "why?");
        assert!(!a.show_help);
        assert_eq!(a.input_text(), "why?");
    }

    #[test]
    fn finished_turns_add_answers_and_stop_reasons() {
        let mut a = App::new("p", "m");
        a.start_turn("q".into());
        a.finish_turn(
            &outcome(TurnState::Completed, Some("A."), None),
            Usage::measured(3, 4),
        );
        assert_eq!(a.entries.last(), Some(&Entry::Assistant("A.".into())));
        assert_eq!(a.usage.total(), 7);
        a.start_turn("q2".into());
        a.finish_turn(
            &outcome(
                TurnState::BudgetExceeded,
                None,
                Some("The turn reached its limit."),
            ),
            Usage::measured(3, 4),
        );
        assert_eq!(
            a.entries.last(),
            Some(&Entry::Notice("The turn reached its limit.".into()))
        );
        assert!(!a.is_busy());
    }

    #[test]
    fn events_become_steps_only_when_worth_showing() {
        let mut a = App::new("p", "m");
        a.start_turn("q".into());
        a.on_event("model.replied", "m replied");
        a.on_event(
            "policy.decided",
            "`web_search` allowed: network steps are allowed",
        );
        a.on_event("tool.executed", "Ran `web_search`");
        let steps: Vec<_> = a
            .entries
            .iter()
            .filter(|e| matches!(e, Entry::Step(_)))
            .collect();
        assert_eq!(steps, [&Entry::Step("Ran `web_search`".into())]);
    }

    /// #77: the plan shows as a step line each time it changes.
    #[test]
    fn plan_updates_show_as_steps() {
        let mut a = App::new("p", "m");
        a.start_turn("q".into());
        a.on_event(
            "plan.updated",
            "Plan · 1 of 2 done: ✓ Find sources · ▸ Write the summary",
        );
        assert!(
            a.entries
                .iter()
                .any(|e| matches!(e, Entry::Step(s) if s.starts_with("Plan · 1 of 2 done")))
        );
    }
}
