//! The turn loop against a scripted model, real tools, and a real store.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Duration;

use serde_json::{Value, json};
use tern_agent::tools::BoxFuture;
use tern_agent::{
    ApprovalAnswer, ApprovalRequest, Approver, Budget, CancelFlag, DenyAll, EventSink, MemorySink,
    Mode, Model, PolicyConfig, PolicyGate, StoreSink, Tool, ToolError, ToolOutput, ToolRegistry,
    TurnLoop, TurnOutcome,
};
use tern_core::{EffectClass, Measure, SessionId, TurnState};
use tern_providers::{
    ChatRequest, ChatResponse, ProviderError, ProviderResult, Usage, UsageLedger,
};
use tern_store::{Harness, Store};

/// Replies with each scripted text in turn and keeps every request it got.
struct Script {
    replies: Mutex<VecDeque<ProviderResult<String>>>,
    seen: Mutex<Vec<ChatRequest>>,
}

impl Script {
    fn new(replies: &[&str]) -> Self {
        Self {
            replies: Mutex::new(replies.iter().map(|r| Ok(r.to_string())).collect()),
            seen: Mutex::new(Vec::new()),
        }
    }
    fn failing(e: ProviderError) -> Self {
        Self {
            replies: Mutex::new(VecDeque::from([Err(e)])),
            seen: Mutex::new(Vec::new()),
        }
    }
    fn requests(&self) -> Vec<ChatRequest> {
        self.seen.lock().unwrap().clone()
    }
}

impl Model for Script {
    fn provider_id(&self) -> &str {
        "script"
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

#[derive(Debug)]
struct Search;
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
        Box::pin(async move {
            Ok(ToolOutput {
                content: format!("[S1] result for {}", args["query"].as_str().unwrap_or("")),
                evidence_refs: vec!["S1".into()],
                ..ToolOutput::default()
            })
        })
    }
}

/// An external side effect that counts how often it really ran.
#[derive(Debug, Default)]
struct Publish(Mutex<u32>);
impl Tool for Publish {
    fn name(&self) -> &'static str {
        "publish"
    }
    fn summary(&self) -> &'static str {
        "Post a note publicly"
    }
    fn effect(&self) -> EffectClass {
        EffectClass::ExternalSideEffect
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object", "properties": {"site": {"type": "string"}}, "required": ["site"]})
    }
    fn target(&self, args: &Value) -> Option<String> {
        args["site"].as_str().map(str::to_owned)
    }
    fn call<'a>(&'a self, _args: Value) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        *self.0.lock().unwrap() += 1;
        Box::pin(async { Ok(ToolOutput::text("posted")) })
    }
}

#[derive(Debug)]
struct Slow;
impl Tool for Slow {
    fn name(&self) -> &'static str {
        "slow"
    }
    fn summary(&self) -> &'static str {
        "Never finishes in time"
    }
    fn effect(&self) -> EffectClass {
        EffectClass::Read
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object"})
    }
    fn timeout(&self) -> Duration {
        Duration::from_millis(20)
    }
    fn call<'a>(&'a self, _args: Value) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async {
            tokio::time::sleep(Duration::from_secs(5)).await;
            Ok(ToolOutput::text("late"))
        })
    }
}

struct Answer(ApprovalAnswer, Mutex<Vec<ApprovalRequest>>);
impl Answer {
    fn new(a: ApprovalAnswer) -> Self {
        Self(a, Mutex::new(Vec::new()))
    }
}
impl Approver for Answer {
    fn ask<'a>(&'a self, req: &'a ApprovalRequest) -> BoxFuture<'a, ApprovalAnswer> {
        self.1.lock().unwrap().push(req.clone());
        let a = self.0;
        Box::pin(async move { a })
    }
}

struct Setup {
    tools: ToolRegistry,
    gate: PolicyGate,
    ledger: UsageLedger,
    budget: Budget,
    cancel: CancelFlag,
}

impl Setup {
    fn new() -> Self {
        let mut tools = ToolRegistry::new();
        tools
            .register(Search)
            .register(Publish::default())
            .register(Slow);
        Self {
            tools,
            gate: PolicyGate::default(),
            ledger: UsageLedger::default(),
            budget: Budget::default(),
            cancel: CancelFlag::default(),
        }
    }

    async fn run(
        &mut self,
        model: &dyn Model,
        approver: &dyn Approver,
        sink: &dyn EventSink,
        session: SessionId,
        msg: &str,
    ) -> TurnOutcome {
        TurnLoop {
            session,
            model,
            model_name: "test-model",
            tools: &self.tools,
            gate: &mut self.gate,
            approver,
            sink,
            ledger: &self.ledger,
            budget: self.budget,
            cancel: self.cancel.clone(),
            instructions: None,
            max_output_tokens: 512,
            on_text: None,
            hooks: Default::default(),
        }
        .run(Vec::new(), msg)
        .await
    }
}

fn kinds(sink: &MemorySink) -> Vec<String> {
    sink.events().into_iter().map(|e| e.kind).collect()
}

#[tokio::test]
async fn answers_directly_and_records_usage() {
    let mut s = Setup::new();
    let model = Script::new(&["Attention is all you need (2017)."]);
    let sink = MemorySink::default();
    let out = s
        .run(&model, &DenyAll, &sink, SessionId::new(), "Which paper?")
        .await;
    assert_eq!(out.state, TurnState::Completed);
    assert_eq!(
        out.answer.as_deref(),
        Some("Attention is all you need (2017).")
    );
    assert_eq!(
        kinds(&sink),
        ["turn.started", "model.replied", "turn.finished"]
    );
    assert_eq!(out.usage, Usage::measured(100, 10));
    assert_eq!(s.ledger.total(), Usage::measured(100, 10));
    let req = &model.requests()[0];
    assert!(req.system.as_deref().unwrap().contains("web_search"));
    let seqs: Vec<_> = sink.events().iter().map(|e| e.seq).collect();
    assert_eq!(seqs, [1, 2, 3]);
}

#[tokio::test]
async fn allowed_tool_runs_and_its_result_goes_back_to_the_model() {
    let mut s = Setup::new();
    let model = Script::new(&[
        &tool_call("web_search", json!({"query": "tern"})),
        "Found it [S1].",
    ]);
    let sink = MemorySink::default();
    let out = s
        .run(&model, &DenyAll, &sink, SessionId::new(), "Search")
        .await;
    assert_eq!(out.state, TurnState::Completed);
    assert_eq!((out.model_calls, out.tool_calls), (2, 1));
    assert_eq!(
        kinds(&sink),
        [
            "turn.started",
            "model.replied",
            "policy.decided",
            "tool.executed",
            "model.replied",
            "turn.finished"
        ]
    );
    let executed = sink
        .events()
        .into_iter()
        .find(|e| e.kind == "tool.executed")
        .unwrap();
    assert_eq!(executed.evidence_refs, ["S1"]);
    let second = &model.requests()[1];
    assert!(
        second
            .messages
            .last()
            .unwrap()
            .content
            .contains("[S1] result for tern")
    );
    assert!(out.states.contains(&TurnState::PolicyCheck));
}

#[tokio::test]
async fn risky_tool_asks_and_a_yes_runs_it() {
    let mut s = Setup::new();
    let model = Script::new(&[
        &tool_call("publish", json!({"site": "blog.example"})),
        "Posted.",
    ]);
    let approver = Answer::new(ApprovalAnswer::AllowOnce);
    let sink = MemorySink::default();
    let out = s
        .run(&model, &approver, &sink, SessionId::new(), "Publish it")
        .await;
    assert_eq!(out.state, TurnState::Completed);
    let asked = approver.1.lock().unwrap().clone();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].target.as_deref(), Some("blog.example"));
    let k = kinds(&sink);
    let pos = |t: &str| k.iter().position(|x| x == t).unwrap();
    assert!(pos("approval.requested") < pos("approval.answered"));
    assert!(pos("approval.answered") < pos("tool.executed"));
    assert!(out.states.contains(&TurnState::ApprovalRequested));
}

#[tokio::test]
async fn a_no_blocks_the_turn_and_nothing_runs() {
    let mut s = Setup::new();
    let model = Script::new(&[&tool_call("publish", json!({"site": "x.example"}))]);
    let sink = MemorySink::default();
    let out = s
        .run(&model, &DenyAll, &sink, SessionId::new(), "Publish")
        .await;
    assert_eq!(out.state, TurnState::Blocked);
    assert!(out.reason.unwrap().contains("declined"));
    assert_eq!(out.tool_calls, 0);
    assert!(!kinds(&sink).contains(&"tool.executed".to_string()));
}

#[tokio::test]
async fn a_deny_rule_blocks_without_asking() {
    let mut s = Setup::new();
    s.gate = PolicyGate::new(PolicyConfig::default().with_mode(EffectClass::Network, Mode::Deny));
    let model = Script::new(&[&tool_call("web_search", json!({"query": "x"}))]);
    let approver = Answer::new(ApprovalAnswer::AllowOnce);
    let sink = MemorySink::default();
    let out = s
        .run(&model, &approver, &sink, SessionId::new(), "Search")
        .await;
    assert_eq!(out.state, TurnState::Blocked);
    assert!(approver.1.lock().unwrap().is_empty());
}

#[tokio::test]
async fn session_grant_covers_the_next_call_to_the_same_target() {
    let mut s = Setup::new();
    let approver = Answer::new(ApprovalAnswer::AllowForSession);
    let sink = MemorySink::default();
    let ses = SessionId::new();
    let call = tool_call("publish", json!({"site": "blog.example"}));
    s.run(&Script::new(&[&call, "ok"]), &approver, &sink, ses, "one")
        .await;
    let out = s
        .run(&Script::new(&[&call, "ok"]), &DenyAll, &sink, ses, "two")
        .await;
    assert_eq!(out.state, TurnState::Completed, "{:?}", out.reason);
    assert_eq!(approver.1.lock().unwrap().len(), 1);
    let other = tool_call("publish", json!({"site": "other.example"}));
    let out = s
        .run(&Script::new(&[&other]), &DenyAll, &sink, ses, "three")
        .await;
    assert_eq!(out.state, TurnState::Blocked, "a new target asks again");
}

#[tokio::test]
async fn model_call_limit_stops_the_turn() {
    let mut s = Setup::new();
    s.budget.max_model_calls = 2;
    let model = Script::new(&[
        &tool_call("web_search", json!({"query": "a"})),
        &tool_call("web_search", json!({"query": "b"})),
        "never reached",
    ]);
    let sink = MemorySink::default();
    let out = s.run(&model, &DenyAll, &sink, SessionId::new(), "go").await;
    assert_eq!(out.state, TurnState::BudgetExceeded);
    assert_eq!(out.model_calls, 2);
    assert_eq!(kinds(&sink).last().unwrap(), "budget.exceeded");
}

#[tokio::test]
async fn token_limit_stops_the_turn() {
    let mut s = Setup::new();
    s.budget.max_tokens = 50;
    let model = Script::new(&["too long"]);
    let out = s
        .run(
            &model,
            &DenyAll,
            &MemorySink::default(),
            SessionId::new(),
            "go",
        )
        .await;
    assert_eq!(out.state, TurnState::BudgetExceeded);
    assert!(out.reason.unwrap().contains("110 tokens"));
}

#[tokio::test]
async fn repeating_the_same_call_is_caught_as_a_loop() {
    let mut s = Setup::new();
    let call = tool_call("web_search", json!({"query": "same"}));
    let model = Script::new(&[&call, &call, &call, "never"]);
    let sink = MemorySink::default();
    let out = s.run(&model, &DenyAll, &sink, SessionId::new(), "go").await;
    assert_eq!(out.state, TurnState::Failed);
    assert!(kinds(&sink).contains(&"loop.suspected".to_string()));
    assert_eq!(out.tool_calls, 2);
}

#[tokio::test]
async fn unknown_tools_and_bad_input_go_back_to_the_model() {
    let mut s = Setup::new();
    let model = Script::new(&[
        &tool_call("rm_rf", json!({})),
        &tool_call("web_search", json!({"q": 1})),
        "```tool\nnot json\n```",
        "Fine, here is the answer.",
    ]);
    let sink = MemorySink::default();
    let out = s.run(&model, &DenyAll, &sink, SessionId::new(), "go").await;
    assert_eq!(out.state, TurnState::Completed);
    assert_eq!(out.tool_calls, 0);
    let rejected = kinds(&sink)
        .iter()
        .filter(|k| *k == "tool.rejected")
        .count();
    assert_eq!(rejected, 3);
    let reqs = model.requests();
    assert!(
        reqs[1]
            .messages
            .last()
            .unwrap()
            .content
            .contains("no tool called `rm_rf`")
    );
    assert!(
        reqs[2]
            .messages
            .last()
            .unwrap()
            .content
            .contains("missing `query`")
    );
}

#[tokio::test]
async fn a_tool_timeout_is_reported_not_hidden() {
    let mut s = Setup::new();
    let model = Script::new(&[&tool_call("slow", json!({})), "It timed out."]);
    let sink = MemorySink::default();
    let out = s.run(&model, &DenyAll, &sink, SessionId::new(), "go").await;
    assert_eq!(out.state, TurnState::Completed);
    assert!(kinds(&sink).contains(&"tool.failed".to_string()));
    assert!(
        model.requests()[1]
            .messages
            .last()
            .unwrap()
            .content
            .contains("timed out")
    );
}

#[tokio::test]
async fn provider_errors_fail_plainly_with_no_fallback() {
    let mut s = Setup::new();
    let model = Script::failing(ProviderError::Auth {
        provider: "OpenAI",
        status: 401,
        detail: String::new(),
    });
    let out = s
        .run(
            &model,
            &DenyAll,
            &MemorySink::default(),
            SessionId::new(),
            "go",
        )
        .await;
    assert_eq!(out.state, TurnState::Failed);
    assert!(out.reason.unwrap().contains("refused the key"));
    assert_eq!(model.requests().len(), 1);
}

#[tokio::test]
async fn cancel_before_start_stops_without_calling_the_model() {
    let mut s = Setup::new();
    s.cancel.cancel();
    let model = Script::new(&["hi"]);
    let sink = MemorySink::default();
    let out = s.run(&model, &DenyAll, &sink, SessionId::new(), "go").await;
    assert_eq!(out.state, TurnState::Cancelled);
    assert!(model.requests().is_empty());
    assert_eq!(kinds(&sink).last().unwrap(), "run.cancelled");
}

#[tokio::test]
async fn estimated_usage_stays_marked_estimated() {
    struct Local;
    impl Model for Local {
        fn provider_id(&self) -> &str {
            "ollama"
        }
        fn complete<'a>(
            &'a self,
            _r: &'a ChatRequest,
        ) -> BoxFuture<'a, ProviderResult<ChatResponse>> {
            Box::pin(async {
                Ok(ChatResponse {
                    text: "ok".into(),
                    usage: Usage::estimated(40, 8),
                    finish_reason: None,
                    model: None,
                })
            })
        }
    }
    let mut s = Setup::new();
    let sink = MemorySink::default();
    let out = s.run(&Local, &DenyAll, &sink, SessionId::new(), "go").await;
    assert_eq!(out.usage.measure, Measure::Estimated);
    let last = sink.events().pop().unwrap();
    assert_eq!(last.usage_delta.unwrap().measure, Measure::Estimated);
}

#[tokio::test]
async fn the_whole_turn_replays_from_the_store_in_order() {
    let store = Store::open_in_memory().unwrap();
    let project = store.create_project("FYP", None).unwrap();
    let ses = store
        .create_session(project, Harness::Research, "Survey")
        .unwrap();
    let sink = StoreSink::new(store);
    let mut s = Setup::new();
    let model = Script::new(&[
        &tool_call(
            "publish",
            json!({"site": "blog.example", "token": "sk-proj-abcdefghijklmnopqrstuvwxyz0123456789"}),
        ),
        "Done.",
    ]);
    let approver = Answer::new(ApprovalAnswer::AllowOnce);
    let out = s.run(&model, &approver, &sink, ses, "go").await;
    assert_eq!(out.state, TurnState::Completed);
    let store = sink.into_inner();
    let events = store.replay(ses, 0).unwrap();
    let k: Vec<_> = events.iter().map(|e| e.kind.as_str()).collect();
    assert_eq!(
        k,
        [
            "turn.started",
            "model.replied",
            "policy.decided",
            "approval.requested",
            "approval.answered",
            "tool.executed",
            "model.replied",
            "turn.finished"
        ]
    );
    assert!(events.iter().all(|e| e.turn_id == Some(out.turn_id)));
    let text = serde_json::to_string(&events).unwrap();
    assert!(
        !text.contains("sk-proj-abcdefghij"),
        "a key reached the log"
    );
}

#[tokio::test]
async fn streamed_text_is_numbered_per_model_call() {
    let s = Setup::new();
    let model = Script::new(&[
        &tool_call("web_search", json!({"query": "tern"})),
        "Found it [S1].",
    ]);
    let seen = std::sync::Mutex::new(Vec::<(u32, String)>::new());
    let on_text = |call: u32, t: &str| seen.lock().unwrap().push((call, t.to_owned()));
    let mut gate = PolicyGate::default();
    let out = TurnLoop {
        session: SessionId::new(),
        model: &model,
        model_name: "test-model",
        tools: &s.tools,
        gate: &mut gate,
        approver: &DenyAll,
        sink: &MemorySink::default(),
        ledger: &s.ledger,
        budget: s.budget,
        cancel: s.cancel.clone(),
        instructions: None,
        max_output_tokens: 512,
        on_text: Some(&on_text),
        hooks: Default::default(),
    }
    .run(Vec::new(), "Search")
    .await;
    assert_eq!(out.answer.as_deref(), Some("Found it [S1]."));
    let seen = seen.into_inner().unwrap();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].0, 1);
    assert!(seen[0].1.starts_with("```tool"));
    assert_eq!(seen[1], (2, "Found it [S1].".to_owned()));
}

/// Streams a reasoning delta, then the answer, and records the effort it was asked for.
struct Thinker(Mutex<Option<tern_providers::Effort>>);

impl Model for Thinker {
    fn provider_id(&self) -> &str {
        "script"
    }
    fn complete<'a>(
        &'a self,
        _req: &'a ChatRequest,
    ) -> BoxFuture<'a, ProviderResult<ChatResponse>> {
        unreachable!("the loop streams when it has a text sink")
    }
    fn complete_deltas<'a>(
        &'a self,
        req: &'a ChatRequest,
        on: &'a (dyn Fn(tern_providers::Delta<'_>) -> bool + Send + Sync),
    ) -> BoxFuture<'a, ProviderResult<ChatResponse>> {
        *self.0.lock().unwrap() = req.effort;
        Box::pin(async move {
            on(tern_providers::Delta::Reasoning("Paging splits memory "));
            on(tern_providers::Delta::Reasoning("into pages."));
            on(tern_providers::Delta::Text("Paging maps pages to frames."));
            Ok(ChatResponse {
                text: "Paging maps pages to frames.".into(),
                usage: Usage::measured(10, 5),
                finish_reason: Some("stop".into()),
                model: None,
            })
        })
    }
}

#[tokio::test]
async fn reasoning_streams_to_its_own_sink_and_is_logged_with_the_effort() {
    let s = Setup::new();
    let model = Thinker(Mutex::new(None));
    let sink = MemorySink::default();
    let text = Mutex::new(String::new());
    let thought = Mutex::new(String::new());
    let on_text = |_: u32, t: &str| text.lock().unwrap().push_str(t);
    let on_reasoning = |_: u32, t: &str| thought.lock().unwrap().push_str(t);
    let mut gate = PolicyGate::default();
    let out = TurnLoop {
        session: SessionId::new(),
        model: &model,
        model_name: "test-model",
        tools: &s.tools,
        gate: &mut gate,
        approver: &DenyAll,
        sink: &sink,
        ledger: &s.ledger,
        budget: s.budget,
        cancel: CancelFlag::default(),
        instructions: None,
        max_output_tokens: 512,
        on_text: Some(&on_text),
        hooks: tern_agent::engine::RunHooks {
            effort: Some(tern_providers::Effort::High),
            on_reasoning: Some(&on_reasoning),
            ..Default::default()
        },
    }
    .run(Vec::new(), "What is paging?")
    .await;
    assert_eq!(out.answer.as_deref(), Some("Paging maps pages to frames."));
    assert_eq!(*text.lock().unwrap(), "Paging maps pages to frames.");
    assert_eq!(*thought.lock().unwrap(), "Paging splits memory into pages.");
    assert_eq!(*model.0.lock().unwrap(), Some(tern_providers::Effort::High));
    let replied = sink
        .events()
        .into_iter()
        .find(|e| e.kind == "model.replied")
        .unwrap();
    assert_eq!(
        replied.payload["reasoning"],
        "Paging splits memory into pages."
    );
    assert_eq!(replied.payload["effort"], "high");
    let mut run = tern_agent::Run::new();
    for e in sink.events() {
        run.push(&e);
    }
    assert_eq!(run.thought().unwrap().0, "Paging splits memory into pages.");
    assert!(run.started_rfc3339().is_some());
}
