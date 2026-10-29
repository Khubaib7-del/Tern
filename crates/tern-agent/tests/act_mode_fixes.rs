//! #78, fixes from the #77 Windows test, at the engine level against a
//! scripted model in a real folder: a plan the model stops updating, an
//! empty new file, a task graph with an empty `{{id}}`, `file_edit`, a
//! prose "shall I?" before a change, "read sum.md" after a write, and
//! plain Python that reaches for the disk.

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
    dir: tempfile::TempDir,
    tools: ToolRegistry,
    gate: PolicyGate,
    ledger: UsageLedger,
}

impl Folder {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("sum.md"), "# Summary\nArrays and lists.\n").unwrap();
        let ws = Arc::new(Workspace::open(dir.path()).unwrap());
        let mut tools = ToolRegistry::new();
        ws.register(&mut tools);
        tern_agent::workbench::register_always(&mut tools);
        Self {
            dir,
            tools,
            gate: PolicyGate::default(),
            ledger: UsageLedger::default(),
        }
    }

    fn file(&self, name: &str) -> Option<String> {
        std::fs::read_to_string(self.dir.path().join(name)).ok()
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
async fn a_plan_the_model_stops_updating_is_reminded_then_ended() {
    let mut f = Folder::new();
    let model = Script::new(&[
        &tool_call(
            "plan",
            json!({"steps": [
                {"title": "List the folder", "status": "in_progress"},
                {"title": "Read sum.md", "status": "pending"},
                {"title": "Answer", "status": "pending"}
            ]}),
        ),
        &tool_call("files_list", json!({})),
        "The folder has sum.md.",
    ]);
    let sink = MemorySink::default();
    let out = f
        .run(
            &model,
            &Answer::new(ApprovalAnswer::Deny),
            &sink,
            false,
            "What is in the folder?",
        )
        .await;
    assert_eq!(out.state, TurnState::Completed, "{:?}", out.reason);
    // After a round of tools that left the plan alone, the model is told
    // where it stands; not after the plan call itself.
    assert!(!model.read(1).contains("your plan is at"));
    let back = model.read(2);
    assert!(
        back.contains("Tern note: your plan is at 0 of 3 done"),
        "{back}"
    );
    assert!(
        back.contains("\"List the folder\" is in progress"),
        "{back}"
    );
    // The run ended with the plan open: the last plan says where.
    let last = of_kind(&sink, "plan.updated").pop().unwrap();
    assert_eq!(last.payload["ended"], true);
    assert!(
        last.payload["steps"]
            .as_array()
            .unwrap()
            .iter()
            .all(|s| s["status"] == "skipped"),
        "{}",
        last.payload
    );
    assert!(last.user_summary.starts_with("Plan · ended at 0 of 3 done"));
    // It is logged before the turn's end, so the reopened chat has it.
    let kinds: Vec<String> = sink.events().into_iter().map(|e| e.kind).collect();
    let at = kinds.iter().rposition(|k| k == "plan.updated").unwrap();
    assert!(kinds[at + 1..].iter().any(|k| k == "turn.finished"));
}

#[tokio::test]
async fn a_finished_plan_is_not_ended_or_reminded() {
    let mut f = Folder::new();
    let model = Script::new(&[
        &tool_call(
            "plan",
            json!({"steps": [{"title": "List", "status": "in_progress"}]}),
        ),
        &tool_call("files_list", json!({})),
        &tool_call(
            "plan",
            json!({"steps": [{"title": "List", "status": "done"}]}),
        ),
        "Done.",
    ]);
    let sink = MemorySink::default();
    let out = f
        .run(
            &model,
            &Answer::new(ApprovalAnswer::Deny),
            &sink,
            false,
            "List",
        )
        .await;
    assert_eq!(out.state, TurnState::Completed);
    assert!(!model.read(3).contains("your plan is at"));
    let plans = of_kind(&sink, "plan.updated");
    assert_eq!(plans.len(), 2);
    assert!(plans[1].payload.get("ended").is_none());
}

#[tokio::test]
async fn an_empty_new_file_goes_back_to_the_model_and_never_asks() {
    let mut f = Folder::new();
    let model = Script::new(&[
        &tool_call("file_write", json!({"path": "counts.md", "text": ""})),
        "Sorry.",
    ]);
    let sink = MemorySink::default();
    let approver = Answer::new(ApprovalAnswer::AllowOnce);
    let out = f
        .run(&model, &approver, &sink, false, "Write counts.md")
        .await;
    assert_eq!(out.state, TurnState::Completed);
    assert!(approver.asked().is_empty(), "an approval card for nothing");
    assert!(of_kind(&sink, "approval.requested").is_empty());
    assert_eq!(f.file("counts.md"), None);
    let back = model.read(1);
    assert!(
        back.contains("`text` is empty, so counts.md was not created"),
        "{back}"
    );
}

#[tokio::test]
async fn a_graph_task_with_an_empty_placeholder_fails_instead_of_writing_nothing() {
    let mut f = Folder::new();
    std::fs::write(f.dir.path().join("empty.md"), "").unwrap();
    let graph = json!({"tasks": [
        {"id": "read_empty", "tool": "file_read", "args": {"path": "empty.md"}},
        {"id": "write_counts", "tool": "file_write", "args": {"path": "counts.md", "text": "{{read_empty}}"}, "after": ["read_empty"]},
        {"id": "write_more", "tool": "file_write", "args": {"path": "more.md", "text": "{{read_empty.text}}"}}
    ]});
    let model = Script::new(&[&tool_call("run_tasks", graph), "Could not."]);
    let sink = MemorySink::default();
    let approver = Answer::new(ApprovalAnswer::AllowOnce);
    let out = f.run(&model, &approver, &sink, false, "Count").await;
    assert_eq!(out.state, TurnState::Completed, "{:?}", out.reason);
    assert!(approver.asked().is_empty());
    assert_eq!(f.file("counts.md"), None);
    assert_eq!(f.file("more.md"), None);
    let last = of_kind(&sink, "plan.updated").pop().unwrap();
    let steps = last.payload["steps"].as_array().unwrap().clone();
    assert_eq!(steps[0]["status"], "done");
    assert_eq!(steps[1]["status"], "failed");
    assert_eq!(steps[2]["status"], "failed");
    let back = model.read(1);
    assert!(back.contains("gave an empty result"), "{back}");
    assert!(back.contains("write {{read_empty}}"), "{back}");
}

#[tokio::test]
async fn file_edit_appends_through_the_write_card_with_a_small_diff() {
    let mut f = Folder::new();
    let model = Script::new(&[
        &tool_call(
            "file_edit",
            json!({"path": "sum.md", "append": "Reviewed on 29 October."}),
        ),
        &tool_call(
            "file_edit",
            json!({"path": "sum.md", "find": "Arrays and lists.", "replace": "Arrays, lists and trees."}),
        ),
        &tool_call(
            "file_edit",
            json!({"path": "sum.md", "find": "not there", "replace": "x"}),
        ),
        "Added.",
    ]);
    let sink = MemorySink::default();
    let approver = Answer::new(ApprovalAnswer::AllowOnce);
    let out = f
        .run(&model, &approver, &sink, false, "add a line to sum.md")
        .await;
    assert_eq!(out.state, TurnState::Completed, "{:?}", out.reason);
    assert_eq!(
        f.file("sum.md").unwrap(),
        "# Summary\nArrays, lists and trees.\nReviewed on 29 October.\n"
    );
    // Queued, decided and asked as the write of the whole new text, so the
    // card and its diff are the write's.
    let queued = of_kind(&sink, "tool.queued");
    assert_eq!(queued[0].payload["calls"][0]["tool"], "file_write");
    let decided = of_kind(&sink, "policy.decided");
    assert!(
        decided
            .iter()
            .take(2)
            .all(|e| e.payload["tool"] == "file_write")
    );
    for a in approver.asked() {
        assert_eq!(a.tool, "file_write");
    }
    if let Some(first) = approver.asked().first() {
        assert_eq!(
            first.args["text"],
            "# Summary\nArrays and lists.\nReviewed on 29 October.\n"
        );
    }
    // A passage that is not there goes back to the model, unasked.
    let back = model.read(3);
    assert!(back.contains("`find` is not in sum.md"), "{back}");
    assert_eq!(of_kind(&sink, "tool.rejected").len(), 1);
}

#[tokio::test]
async fn a_prose_shall_i_before_a_change_is_turned_into_the_call() {
    let mut f = Folder::new();
    let model = Script::new(&[
        "I will add \"Reviewed.\" at the end of sum.md. Would you like me to proceed with adding this line?",
        &tool_call(
            "file_edit",
            json!({"path": "sum.md", "append": "Reviewed."}),
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
            "add a line \"Reviewed.\" to sum.md",
        )
        .await;
    assert_eq!(out.state, TurnState::Completed, "{:?}", out.reason);
    assert_eq!(out.answer.as_deref(), Some("Added the line."));
    assert_eq!(of_kind(&sink, "model.asked_in_prose").len(), 1);
    assert!(
        model
            .read(1)
            .contains("do not ask in prose whether to go ahead")
    );
    assert!(f.file("sum.md").unwrap().ends_with("Reviewed.\n"));
    // The prompt says so too.
    let system = model.requests()[0].system.clone().unwrap();
    assert!(system.contains("that card is the confirmation"));
    assert!(system.contains("- file_edit"));
}

#[tokio::test]
async fn a_question_that_is_not_about_a_change_is_left_alone() {
    let mut f = Folder::new();
    let model = Script::new(&["Which do you mean, sum.md or notes.md? Shall I check both?"]);
    let sink = MemorySink::default();
    let out = f
        .run(
            &model,
            &Answer::new(ApprovalAnswer::Deny),
            &sink,
            false,
            "what does the summary say",
        )
        .await;
    assert_eq!(out.state, TurnState::Completed);
    assert!(of_kind(&sink, "model.asked_in_prose").is_empty());
    assert_eq!(model.requests().len(), 1);
}

#[tokio::test]
async fn read_a_file_after_a_write_reads_it_again() {
    let mut f = Folder::new();
    std::fs::write(f.dir.path().join("sum.md"), "# Summary\nNew line here.\n").unwrap();
    let model = Script::new(&["It has a title and one line."]);
    let sink = MemorySink::default();
    let out = f
        .run(
            &model,
            &Answer::new(ApprovalAnswer::Deny),
            &sink,
            true,
            "read sum.md",
        )
        .await;
    assert_eq!(out.state, TurnState::Completed);
    let ran = of_kind(&sink, "tool.executed");
    assert_eq!(ran.len(), 1);
    assert_eq!(ran[0].payload["tool"], "file_read");
    assert_eq!(ran[0].payload["args"]["path"], "sum.md");
    // The model answers from the file as it is now.
    let read = model.read(0);
    assert!(read.contains("New line here."), "{read}");
    let system = model.requests()[0].system.clone().unwrap();
    assert!(system.contains("call `file_read` even if you wrote or read it earlier"));
}

#[tokio::test]
async fn plain_python_that_reaches_for_the_disk_is_told_the_functions() {
    let mut f = Folder::new();
    let code = "from pathlib import Path\nfor p in Path('.').iterdir():\n    print(p)";
    let model = Script::new(&[
        &tool_call("run_python", json!({"code": code})),
        "I could not list it.",
    ]);
    let sink = MemorySink::default();
    let out = f
        .run(
            &model,
            &Answer::new(ApprovalAnswer::Deny),
            &sink,
            false,
            "Count lines",
        )
        .await;
    assert_eq!(out.state, TurnState::Completed);
    let back = model.read(1);
    assert!(back.starts_with("Tool `run_python` failed"), "{back}");
    assert!(
        back.contains("no file system, OS or network access"),
        "{back}"
    );
    assert!(
        back.contains("files_list(), file_read(), file_write()"),
        "{back}"
    );
    // The prompt says it before the first script, with an example.
    let system = model.requests()[0].system.clone().unwrap();
    assert!(
        system.contains("`run_python` has no file system"),
        "{system}"
    );
    assert!(
        system.contains("For example:\nnames = files_list()"),
        "{system}"
    );
}
