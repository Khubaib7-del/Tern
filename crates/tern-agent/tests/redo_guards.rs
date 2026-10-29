//! #81, fixes from the #80 Windows test, at the engine level against a
//! scripted model in a real folder: a turn that writes the same file again
//! with the same text is told it is already written (no approval card),
//! a third `file_write` to one path is refused with "answer now", an empty
//! folder still ends with an answer, and a code-mode script's write and
//! rename are said plainly in its result and are not made again by direct
//! calls.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tern_agent::tools::BoxFuture;
use tern_agent::workspace::{TurnMode, Workspace};
use tern_agent::{
    ApprovalAnswer, ApprovalRequest, Approver, Budget, CancelFlag, MemorySink, Model, PolicyGate,
    ToolRegistry, TurnLoop, TurnOutcome,
};
use tern_core::{EventEnvelope, SessionId, TurnState};
use tern_providers::{ChatRequest, ChatResponse, ProviderResult, Usage, UsageLedger};

/// A scripted model: each reply in turn.
struct Script {
    replies: Mutex<VecDeque<String>>,
    seen: Mutex<Vec<ChatRequest>>,
}

impl Script {
    fn new(replies: &[&str]) -> Self {
        Self {
            replies: Mutex::new(replies.iter().map(|r| r.to_string()).collect()),
            seen: Mutex::new(Vec::new()),
        }
    }
    /// The last message of request `i`: what the model read last.
    fn read(&self, i: usize) -> String {
        self.seen.lock().unwrap()[i]
            .messages
            .last()
            .unwrap()
            .content
            .clone()
    }
}

impl Model for Script {
    fn provider_id(&self) -> &str {
        "groq"
    }
    fn complete<'a>(&'a self, req: &'a ChatRequest) -> BoxFuture<'a, ProviderResult<ChatResponse>> {
        self.seen.lock().unwrap().push(req.clone());
        let text = self
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| "out of script".into());
        Box::pin(async move {
            Ok(ChatResponse {
                text,
                usage: Usage::measured(100, 10),
                finish_reason: Some("stop".into()),
                model: None,
            })
        })
    }
}

fn tool_call(name: &str, args: Value) -> String {
    format!("```tool\n{}\n```", json!({"tool": name, "args": args}))
}

/// Approves every card once, and keeps what it was asked.
#[derive(Default)]
struct Approve(Mutex<Vec<ApprovalRequest>>);
impl Approve {
    fn asked(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.tool.clone())
            .collect()
    }
}
impl Approver for Approve {
    fn ask<'a>(&'a self, req: &'a ApprovalRequest) -> BoxFuture<'a, ApprovalAnswer> {
        self.0.lock().unwrap().push(req.clone());
        Box::pin(async { ApprovalAnswer::AllowOnce })
    }
}

/// An Act-mode chat on a real folder.
struct Folder {
    _dir: tempfile::TempDir,
    root: std::path::PathBuf,
    tools: ToolRegistry,
    gate: PolicyGate,
    ledger: UsageLedger,
}

impl Folder {
    fn new(files: &[(&str, &str)]) -> Self {
        let dir = tempfile::tempdir().unwrap();
        for (name, text) in files {
            std::fs::write(dir.path().join(name), text).unwrap();
        }
        let ws = Arc::new(Workspace::open(dir.path()).unwrap());
        let mut tools = ToolRegistry::new();
        ws.register(&mut tools);
        tern_agent::workbench::register_always(&mut tools);
        tools.set_mode(TurnMode::Act);
        let root = ws.root().to_path_buf();
        Self {
            _dir: dir,
            root,
            tools,
            gate: PolicyGate::default(),
            ledger: UsageLedger::default(),
        }
    }

    fn text(&self, name: &str) -> Option<String> {
        std::fs::read_to_string(self.root.join(name)).ok()
    }

    async fn run(
        &mut self,
        model: &dyn Model,
        approver: &dyn Approver,
        sink: &MemorySink,
        msg: &str,
    ) -> TurnOutcome {
        TurnLoop {
            session: SessionId::new(),
            model,
            model_name: "test-model",
            tools: &self.tools,
            gate: &mut self.gate,
            approver,
            sink,
            ledger: &self.ledger,
            budget: Budget::default(),
            cancel: CancelFlag::default(),
            instructions: None,
            max_output_tokens: 4096,
            on_text: None,
            hooks: tern_agent::engine::RunHooks::default(),
        }
        .run(Vec::new(), msg)
        .await
    }
}

fn of_kind(sink: &MemorySink, kind: &str) -> Vec<EventEnvelope> {
    sink.events()
        .into_iter()
        .filter(|e| e.kind == kind)
        .collect()
}

const COUNT: &str = "Count the words in every .md file here and save the counts to counts.md";

// ----- F1. The write loop -----

#[tokio::test]
async fn the_same_write_again_is_already_written_and_a_third_is_refused() {
    let mut f = Folder::new(&[("sum.md", "# Summary\nArrays and lists.\n")]);
    let write = |text: &str| tool_call("file_write", json!({"path": "counts.md", "text": text}));
    let model = Script::new(&[
        &write("sum.md: 5\n"),
        // The same text again: not run, no card.
        &write("sum.md: 5\r\n"),
        // New text: the second write runs (and asks).
        &write("| file | words |\n| sum.md | 5 |\n"),
        // A third write to the same path: refused, no card.
        &write("| file | words |\n| sum.md | 5 |\n| total | 5 |\n"),
        "Saved the counts to counts.md: sum.md has 5 words.",
    ]);
    let sink = MemorySink::default();
    let approver = Approve::default();
    let out = f.run(&model, &approver, &sink, COUNT).await;
    assert_eq!(out.state, TurnState::Completed, "{:?}", out.reason);
    // Two cards, not four.
    assert_eq!(approver.asked(), ["file_write", "file_write"]);
    let same = model.read(2);
    assert!(
        same.contains("counts.md is already written with that content;"),
        "{same}"
    );
    assert!(same.contains("answer the user now"), "{same}");
    let refused = model.read(4);
    assert!(
        refused.contains("counts.md was already written 2 times this turn"),
        "{refused}"
    );
    assert_eq!(of_kind(&sink, "tool.skipped").len(), 1);
    let rejected = of_kind(&sink, "tool.rejected");
    assert_eq!(rejected.len(), 1);
    assert_eq!(rejected[0].payload["tool"], "file_write");
    // The file holds the second write; the answer about it is not taken
    // for a claim of a failed write.
    assert_eq!(
        f.text("counts.md").as_deref(),
        Some("| file | words |\n| sum.md | 5 |\n")
    );
    assert!(of_kind(&sink, "model.false_claim").is_empty());
    assert_eq!(
        out.answer.as_deref(),
        Some("Saved the counts to counts.md: sum.md has 5 words.")
    );
}

#[tokio::test]
async fn an_empty_folder_writes_once_and_ends_with_an_answer() {
    let mut f = Folder::new(&[]);
    let write = tool_call(
        "file_write",
        json!({"path": "counts.md", "text": "# Word counts\n\nNo .md files here.\n"}),
    );
    // The same write twice in one reply (parallel calls).
    let model = Script::new(&[
        &tool_call("files_list", json!({})),
        &format!("{write}\n{write}"),
        "No .md files here; counts.md lists none.",
    ]);
    let sink = MemorySink::default();
    let approver = Approve::default();
    let out = f.run(&model, &approver, &sink, COUNT).await;
    assert_eq!(out.state, TurnState::Completed, "{:?}", out.reason);
    assert_eq!(approver.asked(), ["file_write"]);
    assert_eq!(of_kind(&sink, "tool.skipped").len(), 1);
    assert_eq!(
        out.answer.as_deref(),
        Some("No .md files here; counts.md lists none.")
    );
    assert!(f.text("counts.md").unwrap().contains("No .md files here."));
}

#[tokio::test]
async fn a_model_that_keeps_writing_after_it_was_told_ends_the_turn_answered() {
    let ended = "counts.md is already written. The model kept writing it again after Tern told it so, so Tern ended the turn here.";
    let write = |text: &str| tool_call("file_write", json!({"path": "counts.md", "text": text}));
    // The same call three times: the loop guard ends it as answered.
    let mut f = Folder::new(&[]);
    let w = write("none\n");
    let model = Script::new(&[&format!("{w}\n{w}\n{w}"), "never"]);
    let sink = MemorySink::default();
    let approver = Approve::default();
    let out = f.run(&model, &approver, &sink, COUNT).await;
    assert_eq!(out.state, TurnState::Completed, "{:?}", out.reason);
    assert_eq!(out.answer.as_deref(), Some(ended));
    assert_eq!(approver.asked(), ["file_write"]);
    // The same text in another form, after the steer: ended the same way.
    let mut f = Folder::new(&[]);
    let model = Script::new(&[
        &write("none\n"),
        &write("none\n"),
        &write("none\r\n"),
        "never",
    ]);
    let sink = MemorySink::default();
    let approver = Approve::default();
    let out = f.run(&model, &approver, &sink, COUNT).await;
    assert_eq!(out.state, TurnState::Completed, "{:?}", out.reason);
    assert_eq!(out.answer.as_deref(), Some(ended));
    assert_eq!(approver.asked(), ["file_write"]);
    assert_eq!(f.text("counts.md").as_deref(), Some("none\n"));
}

#[tokio::test]
async fn edits_to_one_file_are_not_counted_as_rewrites() {
    let mut f = Folder::new(&[("notes.md", "one\n")]);
    let model = Script::new(&[
        &tool_call("file_write", json!({"path": "log.md", "text": "a\n"})),
        &tool_call("file_write", json!({"path": "log.md", "text": "b\n"})),
        &tool_call("file_edit", json!({"path": "log.md", "append": "c\n"})),
        "Done.",
    ]);
    let sink = MemorySink::default();
    let approver = Approve::default();
    let out = f
        .run(&model, &approver, &sink, "write a log to log.md")
        .await;
    assert_eq!(out.state, TurnState::Completed, "{:?}", out.reason);
    assert!(of_kind(&sink, "tool.rejected").is_empty());
    assert_eq!(f.text("log.md").as_deref(), Some("b\nc\n"));
}

// ----- F2. No redo after code mode -----

#[tokio::test]
async fn a_scripts_write_and_rename_are_said_and_not_made_again() {
    let mut f = Folder::new(&[("sum.md", "# Summary\n")]);
    let code = "file_write('draft.md', 'hello from tern')\nfile_rename('draft.md', 'hello.md')\nprint('done')";
    let model = Script::new(&[
        &tool_call("run_python", json!({"code": code})),
        // The same two changes again, as direct calls.
        &format!(
            "{}\n{}",
            tool_call(
                "file_write",
                json!({"path": "draft.md", "text": "hello from tern"})
            ),
            tool_call("file_rename", json!({"from": "draft.md", "to": "hello.md"}))
        ),
        "Wrote draft.md and renamed it to hello.md.",
    ]);
    let sink = MemorySink::default();
    let approver = Approve::default();
    let out = f
        .run(
            &model,
            &approver,
            &sink,
            "Using code mode, write hello from tern to draft.md and then rename draft.md to hello.md",
        )
        .await;
    assert_eq!(out.state, TurnState::Completed, "{:?}", out.reason);
    // The script's result says what it did.
    let back = model.read(1);
    assert!(
        back.contains("Done inside the script: wrote draft.md, renamed draft.md -> hello.md."),
        "{back}"
    );
    // Only the script's two calls asked.
    assert_eq!(approver.asked(), ["file_write", "file_rename"]);
    let skipped = of_kind(&sink, "tool.skipped");
    assert_eq!(skipped.len(), 2);
    let again = model.read(2);
    assert!(
        again.contains("draft.md was already written with that content this turn (inside your script) and then renamed to hello.md"),
        "{again}"
    );
    assert!(
        again.contains("draft.md was already renamed to hello.md this turn (inside your script)"),
        "{again}"
    );
    // The folder is as the script left it.
    assert_eq!(f.text("hello.md").as_deref(), Some("hello from tern"));
    assert!(f.text("draft.md").is_none());
}
