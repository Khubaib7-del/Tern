//! #80, fixes from the #79 Windows test, at the engine level against a
//! scripted model in a real folder: a Chat-mode folder chat that is asked
//! for a change ends with "needs Act mode" (before any model call, when
//! the model calls a write, when a task graph plans one, when the answer
//! hands the work back), code mode refuses scripts that define the tool
//! functions, raises failed calls, gives `files_list` as a list and routes
//! pathlib to the file tools, an answer that claims a failed write is
//! steered, a request too large for the model's cap is sent again made to
//! fit, and writing requests never trigger Tern's own web search.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tern_agent::tools::{BoxFuture, Tool, ToolError, ToolOutput};
use tern_agent::workspace::{TurnMode, Workspace};
use tern_agent::{
    ApprovalAnswer, ApprovalRequest, Approver, Budget, CancelFlag, MemorySink, Model, PolicyGate,
    ToolRegistry, TurnLoop, TurnOutcome,
};
use tern_core::{EffectClass, EventEnvelope, SessionId, TurnState};
use tern_providers::{
    ChatRequest, ChatResponse, ProviderError, ProviderResult, Usage, UsageLedger,
};

/// A scripted model: each reply in turn, or an error.
struct Script {
    replies: Mutex<VecDeque<Result<String, ProviderError>>>,
    seen: Mutex<Vec<ChatRequest>>,
}

impl Script {
    fn new(replies: &[&str]) -> Self {
        Self::with(replies.iter().map(|r| Ok(r.to_string())).collect())
    }
    fn with(replies: Vec<Result<String, ProviderError>>) -> Self {
        Self {
            replies: Mutex::new(replies.into()),
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
        "groq"
    }
    fn complete<'a>(&'a self, req: &'a ChatRequest) -> BoxFuture<'a, ProviderResult<ChatResponse>> {
        self.seen.lock().unwrap().push(req.clone());
        let next = self
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Ok("out of script".into()));
        Box::pin(async move {
            next.map(|text| ChatResponse {
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
}
impl Approver for Answer {
    fn ask<'a>(&'a self, req: &'a ApprovalRequest) -> BoxFuture<'a, ApprovalAnswer> {
        self.1.lock().unwrap().push(req.clone());
        let a = self.0;
        Box::pin(async move { a })
    }
}

/// A web search that only counts its calls.
#[derive(Debug, Clone, Default)]
struct CountingSearch(Arc<AtomicUsize>);

impl Tool for CountingSearch {
    fn name(&self) -> &'static str {
        "web_search"
    }
    fn summary(&self) -> &'static str {
        "Search the web."
    }
    fn effect(&self) -> EffectClass {
        EffectClass::Network
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object", "required": ["query"], "properties": {"query": {"type": "string"}}})
    }
    fn call<'a>(&'a self, _args: Value) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(ToolOutput::text("1. Essays about arrays (example.com)")) })
    }
}

/// A chat with a folder holding sum.md, in `mode`.
struct Folder {
    _dir: tempfile::TempDir,
    root: std::path::PathBuf,
    tools: ToolRegistry,
    gate: PolicyGate,
    ledger: UsageLedger,
    budget: Budget,
}

impl Folder {
    fn new(mode: TurnMode) -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("sum.md"), "# Summary\nArrays and lists.\n").unwrap();
        let ws = Arc::new(Workspace::open(dir.path()).unwrap());
        let mut tools = ToolRegistry::new();
        ws.register(&mut tools);
        tern_agent::workbench::register_always(&mut tools);
        tools.set_mode(mode);
        let root = ws.root().to_path_buf();
        Self {
            _dir: dir,
            root,
            tools,
            gate: PolicyGate::default(),
            ledger: UsageLedger::default(),
            budget: Budget::default(),
        }
    }

    fn exists(&self, name: &str) -> bool {
        self.root.join(name).exists()
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
            budget: self.budget,
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

// ----- 1. Chat mode in a folder chat -----

#[tokio::test]
async fn a_write_request_in_chat_mode_needs_act_before_any_model_call() {
    let mut f = Folder::new(TurnMode::Chat);
    let model = Script::new(&["never"]);
    let sink = MemorySink::default();
    let out = f
        .run(
            &model,
            &Answer::new(ApprovalAnswer::Deny),
            &sink,
            "List the files here, then write a short review of sum.md to review.md",
        )
        .await;
    assert_eq!(out.state, TurnState::Completed, "{:?}", out.reason);
    let answer = out.answer.unwrap();
    assert!(answer.starts_with("This needs Act mode."), "{answer}");
    assert!(answer.contains("write review.md"), "{answer}");
    assert!(model.requests().is_empty(), "no model call was paid for");
    let e = of_kind(&sink, "mode.needs_act");
    assert_eq!(e.len(), 1);
    assert_eq!(e[0].payload["tool"], "file_write");
    assert_eq!(e[0].payload["path"], "review.md");
    assert!(
        e[0].payload["message"]
            .as_str()
            .unwrap()
            .contains("review.md")
    );
    assert!(!f.exists("review.md"));
}

#[tokio::test]
async fn a_write_the_model_calls_in_chat_mode_needs_act_not_a_failed_tool() {
    let mut f = Folder::new(TurnMode::Chat);
    let model = Script::new(&[
        &tool_call(
            "file_write",
            json!({"path": "reviews.md", "text": "# Review\n"}),
        ),
        "never",
    ]);
    let sink = MemorySink::default();
    let out = f
        .run(
            &model,
            &Answer::new(ApprovalAnswer::AllowOnce),
            &sink,
            "Write a short review of sum.md",
        )
        .await;
    assert_eq!(out.state, TurnState::Completed, "{:?}", out.reason);
    assert!(out.answer.unwrap().contains("write reviews.md"));
    assert!(
        of_kind(&sink, "tool.rejected").is_empty(),
        "no failed tool card"
    );
    assert_eq!(of_kind(&sink, "mode.needs_act").len(), 1);
    assert_eq!(model.requests().len(), 1);
    assert!(!f.exists("reviews.md"));
    // The prompt in Chat mode lists no write tools.
    let system = model.requests()[0].system.clone().unwrap();
    assert!(!system.contains("def file_write("), "{system}");
}

#[tokio::test]
async fn a_task_graph_that_plans_a_write_in_chat_mode_needs_act() {
    let mut f = Folder::new(TurnMode::Chat);
    let model = Script::new(&[
        &tool_call(
            "run_tasks",
            json!({"tasks": [
                {"id": "read", "tool": "file_read", "args": {"path": "sum.md"}},
                {"id": "write_review", "tool": "file_write", "args": {"path": "reviews.md", "text": "{{read}}"}, "after": ["read"]}
            ]}),
        ),
        "never",
    ]);
    let sink = MemorySink::default();
    let out = f
        .run(
            &model,
            &Answer::new(ApprovalAnswer::Deny),
            &sink,
            "Review sum.md",
        )
        .await;
    assert_eq!(out.state, TurnState::Completed, "{:?}", out.reason);
    let e = of_kind(&sink, "mode.needs_act");
    assert_eq!(e.len(), 1);
    assert_eq!(e[0].payload["path"], "reviews.md");
    // The graph never ran: no read happened.
    assert!(of_kind(&sink, "tool.executed").is_empty());
}

#[tokio::test]
async fn an_answer_that_hands_the_change_back_in_chat_mode_needs_act() {
    let mut f = Folder::new(TurnMode::Chat);
    let model = Script::new(&[
        "I can't do that here. Please manually create a file named reviews.md and paste the content below.",
    ]);
    let sink = MemorySink::default();
    let out = f
        .run(
            &model,
            &Answer::new(ApprovalAnswer::Deny),
            &sink,
            "Write a short review of sum.md",
        )
        .await;
    assert_eq!(out.state, TurnState::Completed);
    let answer = out.answer.unwrap();
    assert!(answer.starts_with("This needs Act mode."), "{answer}");
    assert!(!answer.contains("manually"), "{answer}");
}

#[tokio::test]
async fn a_question_in_chat_mode_is_answered_as_usual() {
    let mut f = Folder::new(TurnMode::Chat);
    let model = Script::new(&[
        &tool_call("file_read", json!({"path": "sum.md"})),
        "sum.md is about arrays and lists.",
    ]);
    let sink = MemorySink::default();
    let out = f
        .run(
            &model,
            &Answer::new(ApprovalAnswer::Deny),
            &sink,
            "what is in sum.md?",
        )
        .await;
    assert_eq!(out.state, TurnState::Completed);
    assert_eq!(
        out.answer.as_deref(),
        Some("sum.md is about arrays and lists.")
    );
    assert!(of_kind(&sink, "mode.needs_act").is_empty());
}

// ----- 2-4. Code mode -----

#[tokio::test]
async fn a_script_that_defines_its_own_tool_functions_is_not_run() {
    let mut f = Folder::new(TurnMode::Act);
    let fake = "def file_write(path, text):\n    return 'File written successfully'\ndef file_rename(a, b):\n    return 'ok'\nfile_write('draft.md', 'hello from tern')\nfile_rename('draft.md', 'hello.md')\nprint('done')";
    let real = "file_write('draft.md', 'hello from tern')\nfile_rename('draft.md', 'hello.md')\nprint('done')";
    let model = Script::new(&[
        &tool_call("run_python", json!({"code": fake})),
        &tool_call("run_python", json!({"code": real})),
        "Wrote draft.md and renamed it to hello.md.",
    ]);
    let sink = MemorySink::default();
    let approver = Answer::new(ApprovalAnswer::AllowOnce);
    let out = f
        .run(
            &model,
            &approver,
            &sink,
            "Using code mode, write hello from tern to draft.md and then rename draft.md to hello.md",
        )
        .await;
    assert_eq!(out.state, TurnState::Completed, "{:?}", out.reason);
    let back = model.read(1);
    assert!(back.contains("Tern did not run this script"), "{back}");
    assert!(back.contains("file_write(), file_rename()"), "{back}");
    let rejected = of_kind(&sink, "tool.rejected");
    assert_eq!(
        rejected[0].payload["shadowed"],
        json!(["file_write", "file_rename"])
    );
    // The real script wrote and renamed.
    assert!(f.exists("hello.md") && !f.exists("draft.md"));
}

#[tokio::test]
async fn a_failed_call_inside_a_script_raises_and_a_claim_of_saving_is_steered() {
    let mut f = Folder::new(TurnMode::Act);
    let code = "names = [e['path'] for e in files_list() if not e['is_dir']]\ncounts = {}\nfile_write('counts.md', '')\nprint('Word counts have been saved')";
    let model = Script::new(&[
        &tool_call("run_python", json!({"code": code})),
        "Word counts have been successfully saved to counts.md.",
        "counts.md was not written: the text was empty.",
    ]);
    let sink = MemorySink::default();
    let out = f
        .run(
            &model,
            &Answer::new(ApprovalAnswer::AllowOnce),
            &sink,
            "Write one python script that counts the words in every .md file here and saves the counts to counts.md",
        )
        .await;
    assert_eq!(out.state, TurnState::Completed, "{:?}", out.reason);
    // The script failed with the write's reason; its print never ran.
    let failed = of_kind(&sink, "tool.failed");
    let script = failed
        .iter()
        .find(|e| e.payload["tool"] == "run_python")
        .expect("the run_python card shows failed");
    let err = script.payload["error"].to_string();
    assert!(err.contains("ToolError"), "{err}");
    assert!(err.contains("text"), "{err}");
    let back = model.read(1);
    assert!(
        !back.contains("printed:\nWord counts have been saved"),
        "{back}"
    );
    // The answer that claimed counts.md was saved was sent back once.
    assert_eq!(of_kind(&sink, "model.false_claim").len(), 1);
    let note = model.read(2);
    assert!(note.contains("counts.md was not written"), "{note}");
    assert_eq!(
        out.answer.as_deref(),
        Some("counts.md was not written: the text was empty.")
    );
    assert!(!f.exists("counts.md"));
}

#[tokio::test]
async fn files_list_is_a_list_of_dicts_inside_a_script() {
    let mut f = Folder::new(TurnMode::Act);
    std::fs::create_dir_all(f.root.join("notes")).unwrap();
    let code = "files = [e for e in files_list() if not e['is_dir']]\nprint('files', len(files), files[0]['path'], files[0]['size'] > 0)";
    let model = Script::new(&[&tool_call("run_python", json!({"code": code})), "One file."]);
    let sink = MemorySink::default();
    let out = f
        .run(
            &model,
            &Answer::new(ApprovalAnswer::Deny),
            &sink,
            "count the files here with code mode",
        )
        .await;
    assert_eq!(out.state, TurnState::Completed, "{:?}", out.reason);
    let back = model.read(1);
    assert!(back.contains("files 1 sum.md True"), "{back}");
    // The prompt documents the shape.
    let system = model.requests()[0].system.clone().unwrap();
    assert!(
        system.contains("def files_list(path: str = None) -> list  # of {\"path\": str, \"size\": int, \"is_dir\": bool}"),
        "{system}"
    );
}

#[tokio::test]
async fn pathlib_in_act_mode_runs_through_the_file_tools() {
    let mut f = Folder::new(TurnMode::Act);
    let code = "from pathlib import Path\nprint(len(Path('sum.md').read_text().split()))";
    let model = Script::new(&[
        &tool_call("run_python", json!({"code": code})),
        "Five words.",
    ]);
    let sink = MemorySink::default();
    let out = f
        .run(
            &model,
            &Answer::new(ApprovalAnswer::Deny),
            &sink,
            "count the words in sum.md with python",
        )
        .await;
    assert_eq!(out.state, TurnState::Completed, "{:?}", out.reason);
    let back = model.read(1);
    assert!(back.contains("printed:\n5"), "{back}");
    let script = of_kind(&sink, "tool.executed")
        .into_iter()
        .find(|e| e.payload["tool"] == "run_python")
        .unwrap();
    assert_eq!(script.payload["meta"]["code_mode"], true);
    assert_eq!(script.payload["meta"]["inner_calls"], 1);
}

#[tokio::test]
async fn a_script_that_calls_no_tool_for_a_file_request_is_told_once() {
    let mut f = Folder::new(TurnMode::Act);
    let code = "if False:\n    file_write('draft.md', 'x')\nprint('done')";
    let model = Script::new(&[
        &tool_call("run_python", json!({"code": code})),
        "Nothing was written.",
    ]);
    let sink = MemorySink::default();
    let out = f
        .run(
            &model,
            &Answer::new(ApprovalAnswer::Deny),
            &sink,
            "write hello to draft.md using code mode",
        )
        .await;
    assert_eq!(out.state, TurnState::Completed, "{:?}", out.reason);
    let back = model.read(1);
    assert!(
        back.contains("called none of Tern's tool functions"),
        "{back}"
    );
}

// ----- 5. A request too large for the model's cap -----

fn too_large(limit: u64, requested: u64) -> ProviderError {
    ProviderError::Http {
        provider: "Groq",
        status: 413,
        body: format!(
            "{{\"error\":{{\"message\":\"Request too large for model `openai/gpt-oss-120b` in organization `org_x` service tier `on_demand` on tokens per minute (TPM): Limit {limit}, Requested {requested}, please reduce your message size and try again.\",\"code\":\"rate_limit_exceeded\"}}}}"
        ),
    }
}

#[tokio::test]
async fn a_request_too_large_is_sent_again_made_to_fit() {
    let mut f = Folder::new(TurnMode::Act);
    let model = Script::with(vec![
        Err(too_large(8000, 17096)),
        Ok("Arrays store items in order.".into()),
    ]);
    let sink = MemorySink::default();
    let out = f
        .run(
            &model,
            &Answer::new(ApprovalAnswer::Deny),
            &sink,
            "what are arrays?",
        )
        .await;
    assert_eq!(out.state, TurnState::Completed, "{:?}", out.reason);
    let reqs = model.requests();
    assert_eq!(reqs.len(), 2);
    let first = reqs[0].system.as_ref().unwrap().len();
    let second = reqs[1].system.as_ref().unwrap().len();
    assert!(
        second < first,
        "the system prompt is shorter: {second} < {first}"
    );
    assert!(u64::from(reqs[1].max_tokens) < 8000);
    assert_eq!(of_kind(&sink, "provider.request_cap").len(), 1);
}

#[tokio::test]
async fn a_request_that_cannot_fit_says_so_in_plain_words() {
    let mut f = Folder::new(TurnMode::Act);
    let model = Script::with(vec![Err(too_large(300, 14455)), Ok("never".into())]);
    let sink = MemorySink::default();
    let out = f
        .run(
            &model,
            &Answer::new(ApprovalAnswer::Deny),
            &sink,
            "what are arrays?",
        )
        .await;
    assert_eq!(out.state, TurnState::Failed);
    let why = out.reason.unwrap();
    assert!(
        why.starts_with(
            "test-model on your Groq plan accepts 300 tokens per request; this chat needs about"
        ),
        "{why}"
    );
    assert!(
        why.contains("Pick a model with a larger limit or start a new chat"),
        "{why}"
    );
    assert!(!why.contains('{'), "no raw JSON: {why}");
    assert_eq!(model.requests().len(), 1);
}

// ----- 8. Writing requests -----

#[tokio::test]
async fn an_essay_request_never_triggers_a_web_search() {
    let mut f = Folder::new(TurnMode::Act);
    let search = CountingSearch::default();
    f.tools.register(search.clone());
    let model = Script::new(&[
        "I don't have real-time access to the internet, but here is an essay: arrays are...",
    ]);
    let sink = MemorySink::default();
    let out = f
        .run(
            &model,
            &Answer::new(ApprovalAnswer::Deny),
            &sink,
            "Write a long essay about arrays",
        )
        .await;
    assert_eq!(out.state, TurnState::Completed);
    assert_eq!(search.0.load(Ordering::SeqCst), 0);
    assert!(of_kind(&sink, "web.safety_net").is_empty());
}

#[tokio::test]
async fn a_refused_writing_request_is_asked_once_more() {
    let mut f = Folder::new(TurnMode::Act);
    let model = Script::new(&[
        "I'm sorry, but I can't assist with that request.",
        "Arrays are the simplest data structure...",
    ]);
    let sink = MemorySink::default();
    let out = f
        .run(
            &model,
            &Answer::new(ApprovalAnswer::Deny),
            &sink,
            "Write a long essay about arrays, then press Stop.",
        )
        .await;
    assert_eq!(out.state, TurnState::Completed);
    assert_eq!(
        out.answer.as_deref(),
        Some("Arrays are the simplest data structure...")
    );
    assert_eq!(of_kind(&sink, "model.refused").len(), 1);
    assert!(model.read(1).contains("ordinary request"));
}

// ----- 7. Diagrams -----

#[tokio::test]
async fn the_prompt_says_mermaid_blocks_are_drawn() {
    let mut f = Folder::new(TurnMode::Act);
    let model = Script::new(&["```mermaid\nflowchart TD\n  A --> B\n```"]);
    let sink = MemorySink::default();
    f.run(
        &model,
        &Answer::new(ApprovalAnswer::Deny),
        &sink,
        "draw a flowchart of f calling g",
    )
    .await;
    let system = model.requests()[0].system.clone().unwrap();
    assert!(
        system.contains("```mermaid fenced blocks, drawn as diagrams"),
        "{system}"
    );
}
