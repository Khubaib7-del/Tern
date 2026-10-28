//! #77 at the engine level, against a scripted model: the plan tool, a
//! task graph whose independent tasks run in parallel, code mode (one
//! script, many calls, each through the gate), and the result cache.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use tern_agent::tools::BoxFuture;
use tern_agent::{
    ApprovalAnswer, ApprovalRequest, Approver, Budget, CancelFlag, EventSink, MemorySink, Model,
    PolicyGate, Tool, ToolError, ToolOutput, ToolRegistry, TurnLoop, TurnOutcome,
};
use tern_core::{EffectClass, EventEnvelope, SessionId, TurnState};
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

/// A search that takes a moment, counts its calls, and remembers how many
/// ran at the same time.
#[derive(Debug, Default)]
struct SlowSearch {
    calls: AtomicUsize,
    now: AtomicUsize,
    most: AtomicUsize,
    queries: Mutex<Vec<String>>,
}

impl Tool for SlowSearch {
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
            self.calls.fetch_add(1, Ordering::SeqCst);
            let n = self.now.fetch_add(1, Ordering::SeqCst) + 1;
            self.most.fetch_max(n, Ordering::SeqCst);
            let q = args["query"].as_str().unwrap_or("").to_owned();
            self.queries.lock().unwrap().push(q.clone());
            tokio::time::sleep(Duration::from_millis(60)).await;
            self.now.fetch_sub(1, Ordering::SeqCst);
            Ok(ToolOutput::text(format!(
                "[S1] result for {q} https://example.org/{}",
                q.replace(' ', "-")
            )))
        })
    }
}

/// A change outside Tern: always asks.
#[derive(Debug, Default)]
struct Publish(AtomicUsize);

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
        json!({"type": "object", "properties": {"site": {"type": "string"}, "text": {"type": "string"}}, "required": ["site"]})
    }
    fn target(&self, args: &Value) -> Option<String> {
        args["site"].as_str().map(str::to_owned)
    }
    fn call<'a>(&'a self, _args: Value) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(ToolOutput::text("posted")) })
    }
}

struct Answer(ApprovalAnswer, Mutex<Vec<ApprovalRequest>>);
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
    search: std::sync::Arc<SlowSearch>,
}

/// Lets the tests keep a handle on the search they registered.
#[derive(Debug)]
struct Shared(std::sync::Arc<SlowSearch>);
impl Tool for Shared {
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

impl Setup {
    fn new() -> Self {
        let search = std::sync::Arc::new(SlowSearch::default());
        let mut tools = ToolRegistry::new();
        tools
            .register(Shared(search.clone()))
            .register(Publish::default())
            .register(tern_agent::python::RunPython)
            .register(tern_agent::planner::PlanTool)
            .register(tern_agent::planner::RunTasksTool);
        Self {
            tools,
            gate: PolicyGate::default(),
            ledger: UsageLedger::default(),
            search,
        }
    }

    async fn run(
        &mut self,
        model: &dyn Model,
        approver: &dyn Approver,
        sink: &dyn EventSink,
        cache: Option<&tern_agent::toolcache::ToolCache>,
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
                cache,
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
async fn the_plan_tool_logs_each_update_and_has_no_tool_card_noise() {
    let mut s = Setup::new();
    let model = Script::new(&[
        &tool_call(
            "plan",
            json!({"steps": [{"title": "Find sources", "status": "in_progress"}, "Write the summary"]}),
        ),
        &tool_call(
            "plan",
            json!({"steps": [{"title": "Find sources", "status": "done"}, {"title": "Write the summary", "status": "in_progress"}]}),
        ),
        "Here is the summary.",
    ]);
    let sink = MemorySink::default();
    let out = s
        .run(
            &model,
            &Answer(ApprovalAnswer::Deny, Mutex::default()),
            &sink,
            None,
            "Plan it",
        )
        .await;
    assert_eq!(out.state, TurnState::Completed, "{:?}", out.reason);
    let plans = of_kind(&sink, "plan.updated");
    assert_eq!(plans.len(), 2);
    assert_eq!(plans[1].payload["kind"], "plan");
    assert_eq!(plans[1].payload["steps"][0]["status"], "done");
    assert_eq!(plans[1].payload["steps"][1]["status"], "in_progress");
    assert!(plans[1].user_summary.starts_with("Plan · 1 of 2 done"));
    // The model gets a short receipt, not the plan back.
    let req = &model.requests()[1];
    assert!(
        req.messages
            .last()
            .unwrap()
            .content
            .contains("Plan updated (0 of 2 done)")
    );
    // Every chat's system prompt lists the plan and the graph.
    let system = model.requests()[0].system.clone().unwrap();
    assert!(system.contains("- plan") || system.contains("`plan`") || system.contains("plan:"));
    assert!(system.contains("run_tasks"));
}

#[tokio::test]
async fn a_task_graph_runs_independent_tasks_at_once_and_feeds_dependents() {
    let mut s = Setup::new();
    let graph = json!({"goal": "Compare two papers", "tasks": [
        {"id": "a", "tool": "web_search", "args": {"query": "flare paper"}},
        {"id": "b", "tool": "web_search", "args": {"query": "self-rag paper"}},
        {"id": "c", "tool": "web_search", "args": {"query": "rag survey"}},
        {"id": "d", "tool": "web_search", "args": {"query": "follow {{a}}"}, "depends_on": ["a"]},
        {"id": "e", "tool": "publish", "args": {"site": "blog.example", "text": "{{b}}"}}
    ]});
    let model = Script::new(&[&tool_call("run_tasks", graph), "Compared."]);
    let sink = MemorySink::default();
    let approver = Answer(ApprovalAnswer::AllowOnce, Mutex::default());
    let out = s.run(&model, &approver, &sink, None, "Compare").await;
    assert_eq!(out.state, TurnState::Completed, "{:?}", out.reason);
    assert_eq!(s.search.calls.load(Ordering::SeqCst), 4);
    assert!(
        s.search.most.load(Ordering::SeqCst) >= 2,
        "independent searches ran one by one"
    );
    // d got a's result.
    let qs = s.search.queries.lock().unwrap().clone();
    assert!(
        qs.iter()
            .any(|q| q.starts_with("follow [S1] result for flare paper")),
        "{qs:?}"
    );
    // The change still asked, once, and waited for b.
    let asked = approver.1.lock().unwrap().clone();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].tool, "publish");
    // Every task's events name the graph's call and the task.
    let outer = of_kind(&sink, "tool.queued")[0].payload["calls"][0]["call"]
        .as_u64()
        .unwrap();
    let inner: Vec<_> = of_kind(&sink, "tool.executed")
        .into_iter()
        .filter(|e| e.payload["tool"] != "run_tasks")
        .collect();
    assert_eq!(inner.len(), 5);
    assert!(inner.iter().all(|e| e.payload["parent"] == outer));
    assert!(inner.iter().any(|e| e.payload["task"] == "d"));
    // The plan card's graph ends with every task done, in waves.
    let last = of_kind(&sink, "plan.updated").pop().unwrap();
    assert_eq!(last.payload["kind"], "graph");
    assert_eq!(last.payload["waves"], 2);
    assert!(
        last.payload["steps"]
            .as_array()
            .unwrap()
            .iter()
            .all(|s| s["status"] == "done"),
        "{}",
        last.payload
    );
    // The model reads every task's result in one message.
    let back = model.requests()[1].messages.last().unwrap().content.clone();
    assert!(
        back.contains("Task graph: 5 tasks in 2 waves; 5 done"),
        "{back}"
    );
    assert!(back.contains("### Task d · web_search · Done"));
}

#[tokio::test]
async fn a_bad_graph_goes_back_to_the_model_with_an_example() {
    let mut s = Setup::new();
    let graph = json!({"tasks": [
        {"id": "a", "tool": "web_search", "args": {"query": "x"}, "after": ["b"]},
        {"id": "b", "tool": "web_search", "args": {"query": "y"}, "after": ["a"]}
    ]});
    let model = Script::new(&[&tool_call("run_tasks", graph), "Sorry."]);
    let sink = MemorySink::default();
    let out = s
        .run(
            &model,
            &Answer(ApprovalAnswer::Deny, Mutex::default()),
            &sink,
            None,
            "Go",
        )
        .await;
    assert_eq!(out.state, TurnState::Completed);
    assert_eq!(s.search.calls.load(Ordering::SeqCst), 0);
    let back = model.requests()[1].messages.last().unwrap().content.clone();
    assert!(back.contains("circle"), "{back}");
}

#[tokio::test]
async fn code_mode_calls_many_tools_and_only_the_print_comes_back() {
    let mut s = Setup::new();
    let code = "found = []\nfor q in ['flare', 'self-rag', 'raptor']:\n    r = web_search(q)\n    found.append(len(r))\npublish('blog.example', text='done')\nprint('searched', len(found), 'papers')";
    let model = Script::new(&[
        &tool_call("run_python", json!({"code": code})),
        "Searched three.",
    ]);
    let sink = MemorySink::default();
    let approver = Answer(ApprovalAnswer::AllowOnce, Mutex::default());
    let out = s.run(&model, &approver, &sink, None, "Search three").await;
    assert_eq!(out.state, TurnState::Completed, "{:?}", out.reason);
    assert_eq!(s.search.calls.load(Ordering::SeqCst), 3);
    // The write inside the script still asked.
    let asked = approver.1.lock().unwrap().clone();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].tool, "publish");
    // Only the print reached the model, not the search results.
    let back = model.requests()[1].messages.last().unwrap().content.clone();
    assert!(back.contains("searched 3 papers"), "{back}");
    assert!(!back.contains("[S1] result for flare"), "{back}");
    // The inner calls nest under the script's call.
    let outer = of_kind(&sink, "tool.queued")[0].payload["calls"][0]["call"]
        .as_u64()
        .unwrap();
    let inner: Vec<_> = of_kind(&sink, "policy.decided")
        .into_iter()
        .filter(|e| e.payload["tool"] != "run_python")
        .collect();
    assert_eq!(inner.len(), 4);
    assert!(inner.iter().all(|e| e.payload["parent"] == outer));
    assert_eq!(inner[0].payload["args"]["query"], "flare");
    let script = of_kind(&sink, "tool.executed")
        .into_iter()
        .find(|e| e.payload["tool"] == "run_python")
        .unwrap();
    assert_eq!(script.payload["meta"]["inner_calls"], 4);
    assert_eq!(script.payload["meta"]["code_mode"], true);
    // The prompt lists the stubs.
    let system = model.requests()[0].system.clone().unwrap();
    assert!(
        system.contains("def web_search(query: str) -> str:"),
        "{system}"
    );
    assert!(!system.contains("def run_python("));
}

#[tokio::test]
async fn a_declined_write_inside_a_script_stops_the_turn() {
    let mut s = Setup::new();
    let code = "web_search('x')\npublish('blog.example')\nprint('after')";
    let model = Script::new(&[&tool_call("run_python", json!({"code": code})), "never"]);
    let sink = MemorySink::default();
    let approver = Answer(ApprovalAnswer::Deny, Mutex::default());
    let out = s.run(&model, &approver, &sink, None, "Go").await;
    assert_eq!(out.state, TurnState::Blocked);
    assert!(out.reason.unwrap().contains("declined"));
    assert_eq!(model.requests().len(), 1);
}

#[tokio::test]
async fn plain_python_still_runs_without_code_mode() {
    let mut s = Setup::new();
    let model = Script::new(&[
        &tool_call("run_python", json!({"code": "print(sum([1, 2, 3]))"})),
        "Six.",
    ]);
    let sink = MemorySink::default();
    let out = s
        .run(
            &model,
            &Answer(ApprovalAnswer::Deny, Mutex::default()),
            &sink,
            None,
            "Sum",
        )
        .await;
    assert_eq!(out.state, TurnState::Completed);
    let script = of_kind(&sink, "tool.executed").pop().unwrap();
    assert_eq!(script.payload["meta"]["code_mode"], false);
    assert!(script.payload["output"].as_str().unwrap().contains('6'));
}

#[tokio::test]
async fn the_same_search_twice_is_served_from_the_cache_and_marked() {
    let mut s = Setup::new();
    let cache = tern_agent::toolcache::ToolCache::memory();
    let model = Script::new(&[
        &tool_call("web_search", json!({"query": "rag survey"})),
        &tool_call("web_search", json!({"query": "  rag   survey "})),
        &tool_call("publish", json!({"site": "x.example"})),
        &tool_call("publish", json!({"site": "x.example"})),
        "Done.",
    ]);
    let sink = MemorySink::default();
    let approver = Answer(ApprovalAnswer::AllowOnce, Mutex::default());
    let out = s
        .run(&model, &approver, &sink, Some(&cache), "Search")
        .await;
    assert_eq!(out.state, TurnState::Completed, "{:?}", out.reason);
    assert_eq!(s.search.calls.load(Ordering::SeqCst), 1);
    let ran = of_kind(&sink, "tool.executed");
    assert!(ran[0].payload.get("cached").is_none());
    assert_eq!(ran[1].payload["cached"], true);
    assert!(ran[1].user_summary.contains("cached"));
    // Changes are never cached: both publishes asked and ran.
    assert_eq!(approver.1.lock().unwrap().len(), 2);
    assert!(ran[2].payload.get("cached").is_none());
    assert!(ran[3].payload.get("cached").is_none());
}
