//! #79, fixes from the #78 Windows test, at the engine level against a
//! scripted model in a real folder: wrapped tool calls, call limits on a
//! plan turn, a printed script, a rewrite of a file the user asked to add
//! to, a folder named like a path's first segment, missing parent folders,
//! and `run_python` refused inside a task graph.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tern_agent::tools::BoxFuture;
use tern_agent::workspace::Workspace;
use tern_agent::{
    ApprovalAnswer, ApprovalRequest, Approver, Budget, CancelFlag, MemorySink, Model, PolicyGate,
    ToolRegistry, TurnLoop, TurnOutcome,
};
use tern_core::{EventEnvelope, SessionId, TurnState};
use tern_providers::{ChatRequest, ChatResponse, ProviderResult, Usage, UsageLedger};

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
    fn requests(&self) -> Vec<ChatRequest> {
        self.seen.lock().unwrap().clone()
    }
    /// The last message of request `i`: what the model read last.
    fn read(&self, i: usize) -> String {
        self.requests()[i].messages.last().unwrap().content.clone()
    }
}

impl Model for Script {
    fn provider_id(&self) -> &str {
        "script"
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

struct Answer(ApprovalAnswer, Mutex<Vec<ApprovalRequest>>);
impl Answer {
    fn new(a: ApprovalAnswer) -> Self {
        Self(a, Mutex::default())
    }
    fn asked(&self) -> Vec<ApprovalRequest> {
        self.1.lock().unwrap().clone()
    }
}
impl Approver for Answer {
    fn ask<'a>(&'a self, req: &'a ApprovalRequest) -> BoxFuture<'a, ApprovalAnswer> {
        self.1.lock().unwrap().push(req.clone());
        let a = self.0;
        Box::pin(async move { a })
    }
}

/// A chat with a folder, in Act mode: the file tools, the workbench's
/// tools, the plan and the graph.
struct Folder {
    /// Kept so the folder lives as long as the test.
    _dir: tempfile::TempDir,
    root: std::path::PathBuf,
    tools: ToolRegistry,
    gate: PolicyGate,
    ledger: UsageLedger,
}

impl Folder {
    fn new() -> Self {
        Self::named("")
    }

    /// A chat whose folder is called `name` (inside a temp folder), or the
    /// temp folder itself when `name` is empty.
    fn named(name: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = if name.is_empty() {
            dir.path().to_path_buf()
        } else {
            dir.path().join(name)
        };
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("sum.md"), "# Summary\nArrays and lists.\n").unwrap();
        let ws = Arc::new(Workspace::open(&root).unwrap());
        let mut tools = ToolRegistry::new();
        ws.register(&mut tools);
        tern_agent::workbench::register_always(&mut tools);
        let root = ws.root().to_path_buf();
        Self {
            _dir: dir,
            root,
            tools,
            gate: PolicyGate::default(),
            ledger: UsageLedger::default(),
        }
    }

    fn file(&self, name: &str) -> Option<String> {
        std::fs::read_to_string(self.root.join(name)).ok()
    }

    /// The chat's folder.
    fn root(&self) -> std::path::PathBuf {
        self.root.clone()
    }

    async fn run(
        &mut self,
        model: &dyn Model,
        approver: &dyn Approver,
        sink: &MemorySink,
        prefetch: bool,
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
            max_output_tokens: 512,
            on_text: None,
            hooks: tern_agent::engine::RunHooks {
                prefetch,
                ..Default::default()
            },
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

#[tokio::test]
async fn wrapped_tool_calls_run_as_the_call_inside() {
    let mut f = Folder::new();
    let model = Script::new(&[
        // codestral: a tool named `tool`.
        &tool_call(
            "tool",
            json!({"tool": "file_read", "args": {"path": "sum.md"}}),
        ),
        // ministral: the name is the call itself, with a stray brace.
        &format!(
            "```tool\n{}\n```",
            json!({"tool": "{\"tool\": \"files_list\", \"args\": {\"path\": \".\"}}}", "args": {}})
        ),
        "sum.md says Arrays and lists.",
    ]);
    let sink = MemorySink::default();
    let out = f
        .run(
            &model,
            &Answer::new(ApprovalAnswer::Deny),
            &sink,
            false,
            "what is in sum.md",
        )
        .await;
    assert_eq!(out.state, TurnState::Completed, "{:?}", out.reason);
    let queued = of_kind(&sink, "tool.queued");
    assert_eq!(queued[0].payload["calls"][0]["tool"], "file_read");
    assert_eq!(queued[1].payload["calls"][0]["tool"], "files_list");
    assert!(
        model.read(1).contains("Arrays and lists."),
        "{}",
        model.read(1)
    );
    assert!(model.read(2).contains("sum.md"), "{}", model.read(2));
    assert!(of_kind(&sink, "tool.rejected").is_empty());
}

fn plan(status: &str) -> String {
    tool_call(
        "plan",
        json!({"steps": [
            {"title": "Read the notes", "status": status},
            {"title": "Write the review", "status": "pending"}
        ]}),
    )
}

#[tokio::test]
async fn an_open_plan_in_act_mode_gets_twice_the_model_calls() {
    let mut f = Folder::new();
    // A plan, then nine different reads: 11 model calls with the answer,
    // past the default 8.
    let mut replies = vec![plan("in_progress")];
    for i in 0..9 {
        replies.push(tool_call(
            "file_search",
            json!({"query": format!("word{i}")}),
        ));
    }
    replies.push("Done reading.".to_owned());
    let refs: Vec<&str> = replies.iter().map(String::as_str).collect();
    let model = Script::new(&refs);
    let sink = MemorySink::default();
    let out = f
        .run(
            &model,
            &Answer::new(ApprovalAnswer::Deny),
            &sink,
            false,
            "review the notes",
        )
        .await;
    assert_eq!(out.state, TurnState::Completed, "{:?}", out.reason);
    assert_eq!(model.requests().len(), 11);
}

#[tokio::test]
async fn without_a_plan_the_model_call_limit_holds_and_says_so() {
    let mut f = Folder::new();
    let mut replies = Vec::new();
    for i in 0..10 {
        replies.push(tool_call(
            "file_search",
            json!({"query": format!("word{i}")}),
        ));
    }
    let refs: Vec<&str> = replies.iter().map(String::as_str).collect();
    let model = Script::new(&refs);
    let sink = MemorySink::default();
    let out = f
        .run(
            &model,
            &Answer::new(ApprovalAnswer::Deny),
            &sink,
            false,
            "search the notes",
        )
        .await;
    assert_eq!(out.state, TurnState::BudgetExceeded);
    assert_eq!(
        out.reason.as_deref(),
        Some("The turn reached its limit of 8 model calls.")
    );
    assert_eq!(model.requests().len(), 8);
}

#[tokio::test]
async fn a_printed_script_is_steered_to_run_python_once() {
    let mut f = Folder::new();
    let printed = "Here is a script:\n```python\nimport os\nfor n in os.listdir('.'):\n    print(n)\n```\nHow to use this script: save it as count.py and run it.";
    let model = Script::new(&[printed, printed]);
    let sink = MemorySink::default();
    let out = f
        .run(
            &model,
            &Answer::new(ApprovalAnswer::Deny),
            &sink,
            false,
            "write one python script that counts the words in every .md file here and saves to counts.md",
        )
        .await;
    // Told once; the second printed script is the answer.
    assert_eq!(out.state, TurnState::Completed, "{:?}", out.reason);
    assert_eq!(of_kind(&sink, "model.printed_script").len(), 1);
    let back = model.read(1);
    assert!(
        back.contains("Run it yourself now with `run_python`"),
        "{back}"
    );
    assert!(back.contains("file_write(path, text)"), "{back}");
    assert_eq!(model.requests().len(), 2);
}

#[tokio::test]
async fn a_python_answer_to_a_plain_question_is_left_alone() {
    let mut f = Folder::new();
    let model = Script::new(&["```python\nsorted([3, 1, 2])\n```"]);
    let sink = MemorySink::default();
    let out = f
        .run(
            &model,
            &Answer::new(ApprovalAnswer::Deny),
            &sink,
            false,
            "how do I sort a list in python?",
        )
        .await;
    assert_eq!(out.state, TurnState::Completed);
    assert!(of_kind(&sink, "model.printed_script").is_empty());
    assert_eq!(model.requests().len(), 1);
}

#[tokio::test]
async fn a_rewrite_of_a_file_the_user_asked_to_add_to_is_steered_to_file_edit() {
    let mut f = Folder::new();
    let model = Script::new(&[
        // Drops "Arrays and lists." while adding the line.
        &tool_call(
            "file_write",
            json!({"path": "sum.md", "text": "# Summary\nReviewed on 29 October.\n"}),
        ),
        &tool_call(
            "file_edit",
            json!({"path": "sum.md", "append": "Reviewed on 29 October."}),
        ),
        "Added the line.",
    ]);
    let sink = MemorySink::default();
    let approver = Answer::new(ApprovalAnswer::AllowOnce);
    let out = f
        .run(
            &model,
            &approver,
            &sink,
            false,
            "add a line 'Reviewed on 29 October' to sum.md",
        )
        .await;
    assert_eq!(out.state, TurnState::Completed, "{:?}", out.reason);
    assert_eq!(
        f.file("sum.md").unwrap(),
        "# Summary\nArrays and lists.\nReviewed on 29 October.\n"
    );
    let rejected = of_kind(&sink, "tool.rejected");
    assert_eq!(rejected.len(), 1);
    assert_eq!(rejected[0].payload["tool"], "file_write");
    let back = model.read(1);
    assert!(back.contains("Call `file_edit` on sum.md"), "{back}");
    // The rewrite was never asked about.
    for a in approver.asked() {
        assert!(
            a.args["text"]
                .as_str()
                .unwrap()
                .contains("Arrays and lists.")
        );
    }
}

#[tokio::test]
async fn an_append_written_as_a_whole_file_runs_and_shows_plus_one() {
    let mut f = Folder::new();
    let model = Script::new(&[
        &tool_call(
            "file_write",
            json!({"path": "sum.md", "text": "# Summary\nArrays and lists.\nReviewed.\n"}),
        ),
        "Added.",
    ]);
    let sink = MemorySink::default();
    let out = f
        .run(
            &model,
            &Answer::new(ApprovalAnswer::AllowOnce),
            &sink,
            false,
            "add a line 'Reviewed.' to sum.md",
        )
        .await;
    assert_eq!(out.state, TurnState::Completed, "{:?}", out.reason);
    assert!(of_kind(&sink, "tool.rejected").is_empty());
    assert!(f.file("sum.md").unwrap().ends_with("Reviewed.\n"));
    // The card's +N -M comes with the start of the write.
    let started = of_kind(&sink, "tool.started");
    assert_eq!(
        started[0].payload["diff"],
        json!({"added": 1, "removed": 0})
    );
}

#[tokio::test]
async fn paths_that_repeat_the_folder_name_mean_the_folder() {
    let mut f = Folder::named("New");
    let root = f.root();
    assert!(root.ends_with("New"));
    let model = Script::new(&[
        &tool_call("files_list", json!({"path": "New"})),
        &tool_call(
            "file_write",
            json!({"path": "New/review.md", "text": "# Review\nsum.md covers arrays.\n"}),
        ),
        "Wrote review.md.",
    ]);
    let sink = MemorySink::default();
    let out = f
        .run(
            &model,
            &Answer::new(ApprovalAnswer::AllowOnce),
            &sink,
            false,
            "review the notes in review.md",
        )
        .await;
    assert_eq!(out.state, TurnState::Completed, "{:?}", out.reason);
    // `New` listed the folder itself.
    assert!(model.read(1).contains("sum.md"), "{}", model.read(1));
    assert!(root.join("review.md").is_file());
    assert!(!root.join("New").exists());
    assert!(
        model.read(2).contains("Wrote review.md."),
        "{}",
        model.read(2)
    );
}

#[tokio::test]
async fn file_write_makes_missing_folders() {
    let mut f = Folder::new();
    let model = Script::new(&[
        &tool_call(
            "file_write",
            json!({"path": "notes/2026/today.md", "text": "# Today\n"}),
        ),
        "Wrote it.",
    ]);
    let sink = MemorySink::default();
    let out = f
        .run(
            &model,
            &Answer::new(ApprovalAnswer::AllowOnce),
            &sink,
            false,
            "write today's notes in notes/2026/today.md",
        )
        .await;
    assert_eq!(out.state, TurnState::Completed, "{:?}", out.reason);
    assert_eq!(f.file("notes/2026/today.md").as_deref(), Some("# Today\n"));
}

#[tokio::test]
async fn run_python_in_a_task_graph_says_to_call_it_separately() {
    let mut f = Folder::new();
    let graph = json!({"tasks": [
        {"id": "read", "tool": "file_read", "args": {"path": "sum.md"}},
        {"id": "count", "tool": "run_python", "args": {"code": "print(1)"}}
    ]});
    let model = Script::new(&[&tool_call("run_tasks", graph), "Could not."]);
    let sink = MemorySink::default();
    let out = f
        .run(
            &model,
            &Answer::new(ApprovalAnswer::Deny),
            &sink,
            false,
            "count",
        )
        .await;
    assert_eq!(out.state, TurnState::Completed, "{:?}", out.reason);
    let back = model.read(1);
    assert!(back.contains("Call `run_python` separately"), "{back}");
}
