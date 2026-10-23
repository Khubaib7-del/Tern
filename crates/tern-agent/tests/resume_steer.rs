//! Quick turns: checkpoints, resume after the app is killed, and steering.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use tern_agent::checkpoint::{CheckpointSink, MemoryCheckpoints, RunMode};
use tern_agent::engine::RunHooks;
use tern_agent::tools::BoxFuture;
use tern_agent::{
    Budget, CancelFlag, DenyAll, MemorySink, Model, PolicyGate, Run, Steering, Tool, ToolError,
    ToolOutput, ToolRegistry, TurnLoop, TurnOutcome,
};
use tern_core::{EffectClass, SessionId, TurnState};
use tern_providers::{ChatMessage, ChatRequest, ChatResponse, ProviderResult, Usage, UsageLedger};

/// Replies in turn; `None` never replies (the app is killed meanwhile).
struct Script {
    replies: Mutex<VecDeque<Option<String>>>,
    seen: Mutex<Vec<ChatRequest>>,
}

impl Script {
    fn new(replies: &[Option<&str>]) -> Self {
        Self {
            replies: Mutex::new(replies.iter().map(|r| r.map(str::to_owned)).collect()),
            seen: Mutex::new(Vec::new()),
        }
    }
}

impl Model for Script {
    fn provider_id(&self) -> &str {
        "script"
    }
    fn complete<'a>(&'a self, req: &'a ChatRequest) -> BoxFuture<'a, ProviderResult<ChatResponse>> {
        self.seen.lock().unwrap().push(req.clone());
        let next = self.replies.lock().unwrap().pop_front().flatten();
        Box::pin(async move {
            let Some(text) = next else {
                std::future::pending::<()>().await;
                unreachable!()
            };
            Ok(ChatResponse {
                text,
                usage: Usage::measured(100, 10),
                finish_reason: Some("stop".into()),
                model: None,
            })
        })
    }
}

#[derive(Debug, Default)]
struct Search(AtomicUsize);

impl Tool for Search {
    fn name(&self) -> &'static str {
        "web_search"
    }
    fn summary(&self) -> &'static str {
        "Search the web"
    }
    fn effect(&self) -> EffectClass {
        EffectClass::Network
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object", "properties": {"query": {"type": "string"}}, "required": ["query"]})
    }
    fn call<'a>(&'a self, args: Value) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            Ok(ToolOutput {
                content: format!("[S1] result for {}", args["query"].as_str().unwrap_or("")),
                ..ToolOutput::default()
            })
        })
    }
}

fn tool_call(name: &str, args: Value) -> String {
    format!("```tool\n{}\n```", json!({"tool": name, "args": args}))
}

async fn turn(
    session: SessionId,
    model: &Script,
    tools: &ToolRegistry,
    sink: &MemorySink,
    hooks: RunHooks<'_>,
    history: Vec<ChatMessage>,
) -> TurnOutcome {
    let mut gate = PolicyGate::default();
    TurnLoop {
        session,
        model,
        model_name: "test-model",
        tools,
        gate: &mut gate,
        approver: &DenyAll,
        sink,
        ledger: &UsageLedger::default(),
        budget: Budget::default(),
        cancel: CancelFlag::default(),
        instructions: None,
        max_output_tokens: 512,
        on_text: None,
        hooks,
    }
    .run(history, "What is a tern?")
    .await
}

#[tokio::test]
async fn a_killed_quick_turn_resumes_after_its_last_tool_result() {
    let session = SessionId::new();
    let saved = MemoryCheckpoints::default();
    let search = std::sync::Arc::new(Search::default());
    let mut tools = ToolRegistry::new();
    tools.register(ArcSearch(search.clone()));
    let first = Script::new(&[
        Some(&tool_call("web_search", json!({"query": "tern bird"}))),
        None,
    ]);
    let sink = MemorySink::default();
    let killed = tokio::time::timeout(
        Duration::from_millis(300),
        turn(
            session,
            &first,
            &tools,
            &sink,
            RunHooks {
                checkpoints: Some(&saved),
                ..RunHooks::default()
            },
            Vec::new(),
        ),
    )
    .await;
    assert!(killed.is_err());
    assert_eq!(search.0.load(Ordering::SeqCst), 1);
    let ck = saved.latest().expect("a checkpoint");
    assert_eq!(ck.mode, RunMode::Quick);
    assert_eq!(ck.message, "What is a tern?");
    assert_eq!((ck.model_calls, ck.tool_calls), (1, 1));
    let q = ck.quick.clone().unwrap();
    assert_eq!(
        q.messages.len(),
        3,
        "the question, the tool call, its result"
    );
    assert_eq!(ck.progress(), "Quick turn · 1 tool result");

    let second = Script::new(&[Some("Terns are seabirds [S1].")]);
    let sink2 = MemorySink::default();
    let out = turn(
        session,
        &second,
        &tools,
        &sink2,
        RunHooks {
            checkpoints: Some(&saved),
            resume: Some(&ck),
            ..RunHooks::default()
        },
        Vec::new(),
    )
    .await;
    assert_eq!(out.state, TurnState::Completed, "{:?}", out.reason);
    assert_eq!(out.answer.as_deref(), Some("Terns are seabirds [S1]."));
    assert_eq!(
        search.0.load(Ordering::SeqCst),
        1,
        "the search did not run again"
    );
    let req = second.seen.lock().unwrap()[0].clone();
    assert_eq!(req.messages.len(), 3);
    assert!(req.messages[2].content.contains("result for tern bird"));
    assert!(saved.latest().is_none(), "the answer clears the checkpoint");
    let mut run = Run::new();
    for e in sink2.events() {
        run.push(&e);
    }
    assert!(run.resumed.is_some());
    assert_eq!(run.steps[0].payload["message"], "What is a tern?");
}

#[tokio::test]
async fn a_steer_lands_before_the_next_model_call() {
    let steer = Steering::new();
    let search = std::sync::Arc::new(Search::default());
    let mut tools = ToolRegistry::new();
    tools.register(ArcSearch(search.clone()));
    let model = Script::new(&[
        Some(&tool_call("web_search", json!({"query": "tern"}))),
        Some("Done [S1]."),
    ]);
    steer.send("focus on 2024 sources");
    let sink = MemorySink::default();
    let saved = MemoryCheckpoints::default();
    let out = turn(
        SessionId::new(),
        &model,
        &tools,
        &sink,
        RunHooks {
            checkpoints: Some(&saved),
            steering: Some(&steer),
            resume: None,
            ..RunHooks::default()
        },
        Vec::new(),
    )
    .await;
    assert_eq!(out.state, TurnState::Completed);
    let reqs = model.seen.lock().unwrap().clone();
    // One user message, with the steer folded in.
    assert_eq!(reqs[0].messages.len(), 1);
    assert!(reqs[0].messages[0].content.contains("<steer>"));
    assert!(
        reqs[0].messages[0]
            .content
            .contains("focus on 2024 sources")
    );
    let kinds: Vec<String> = sink.events().into_iter().map(|e| e.kind).collect();
    let received = kinds.iter().position(|k| k == "steer.received").unwrap();
    let applied = kinds.iter().position(|k| k == "steer.applied").unwrap();
    let first_reply = kinds.iter().position(|k| k == "model.replied").unwrap();
    assert!(received < applied && applied < first_reply);
    let mut run = Run::new();
    for e in sink.events() {
        run.push(&e);
    }
    assert_eq!(run.steers.len(), 1);
    assert_eq!(run.steers[0].applied, Some(true));
    assert!(!run.resumable);
    // A sink that clears is a no-op on a chat with nothing saved.
    saved.clear("none").unwrap();
}

/// Shares one counting search between registries.
#[derive(Debug)]
struct ArcSearch(std::sync::Arc<Search>);

impl Tool for ArcSearch {
    fn name(&self) -> &'static str {
        self.0.name()
    }
    fn summary(&self) -> &'static str {
        self.0.summary()
    }
    fn effect(&self) -> EffectClass {
        self.0.effect()
    }
    fn input_schema(&self) -> Value {
        self.0.input_schema()
    }
    fn call<'a>(&'a self, args: Value) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        self.0.call(args)
    }
}
