//! Durable turns (#85): acceptance tests A-T2, A-T3, A-T4, A-T9, A-T13 and
//! the governor's wait and pre-fit, against scripted models. No live keys.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tern_agent::checkpoint::MemoryCheckpoints;
use tern_agent::engine::{RESUME_PROMPT, RunHooks, WRAP_UP_PROMPT};
use tern_agent::tools::BoxFuture;
use tern_agent::{
    Budget, CancelFlag, DenyAll, MemorySink, Model, PolicyGate, Tool, ToolError, ToolOutput,
    ToolRegistry, TurnLoop, TurnOutcome,
};
use tern_core::{EffectClass, EventEnvelope, SessionId, TurnReceipt, TurnState};
use tern_providers::estimate::Estimator;
use tern_providers::governor::Governor;
use tern_providers::{
    ChatRequest, ChatResponse, ProviderError, ProviderResult, Usage, UsageLedger,
};

/// One scripted step: a reply, an error, a reply that streams some text
/// and then drops, or a reply that never comes.
enum Step {
    Reply(String),
    Fail(ProviderError),
    DropAfter(String),
    Hang,
}

struct Script {
    steps: Mutex<VecDeque<Step>>,
    seen: Mutex<Vec<ChatRequest>>,
}

impl Script {
    fn new(steps: Vec<Step>) -> Self {
        Self {
            steps: Mutex::new(steps.into()),
            seen: Mutex::new(Vec::new()),
        }
    }
    fn requests(&self) -> Vec<ChatRequest> {
        self.seen.lock().unwrap().clone()
    }
    fn next(&self, req: &ChatRequest) -> Step {
        self.seen.lock().unwrap().push(req.clone());
        self.steps
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Step::Reply("out of script".into()))
    }
}

/// The characters a request sends, for a realistic reported token count
/// (about four characters a token, like a real tokenizer on English).
fn sent_chars(req: &ChatRequest) -> usize {
    req.system.as_deref().map_or(0, str::len)
        + req.messages.iter().map(|m| m.content.len()).sum::<usize>()
}

fn reply(text: String, sent: usize) -> ProviderResult<ChatResponse> {
    let input = (sent as u64).div_ceil(4).max(1);
    let output = (text.len() as u64).div_ceil(4).max(1);
    Ok(ChatResponse {
        text,
        usage: Usage::measured(input, output),
        finish_reason: Some("stop".into()),
        model: None,
    })
}

impl Model for Script {
    fn provider_id(&self) -> &str {
        "script"
    }
    fn complete<'a>(&'a self, req: &'a ChatRequest) -> BoxFuture<'a, ProviderResult<ChatResponse>> {
        let step = self.next(req);
        let sent = sent_chars(req);
        Box::pin(async move {
            match step {
                Step::Reply(t) | Step::DropAfter(t) => reply(t, sent),
                Step::Fail(e) => Err(e),
                Step::Hang => {
                    tokio::time::sleep(Duration::from_secs(600)).await;
                    reply("too late".into(), sent)
                }
            }
        })
    }
    fn complete_stream<'a>(
        &'a self,
        req: &'a ChatRequest,
        on_text: &'a (dyn Fn(&str) -> bool + Send + Sync),
    ) -> BoxFuture<'a, ProviderResult<ChatResponse>> {
        let step = self.next(req);
        let sent = sent_chars(req);
        Box::pin(async move {
            match step {
                Step::Reply(t) => {
                    on_text(&t);
                    reply(t, sent)
                }
                Step::DropAfter(t) => {
                    on_text(&t);
                    Err(ProviderError::Network {
                        provider: "Script",
                        reason: "the connection was reset".into(),
                    })
                }
                Step::Fail(e) => Err(e),
                Step::Hang => {
                    on_text("Thinking about it");
                    tokio::time::sleep(Duration::from_secs(600)).await;
                    reply("too late".into(), sent)
                }
            }
        })
    }
}

/// A web read that counts how often it really ran and returns a page.
#[derive(Debug, Default)]
struct Read(AtomicUsize);

impl Tool for Read {
    fn name(&self) -> &'static str {
        "read_url"
    }
    fn summary(&self) -> &'static str {
        "Read a page"
    }
    fn effect(&self) -> EffectClass {
        EffectClass::Network
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object", "properties": {"url": {"type": "string"}}, "required": ["url"]})
    }
    fn call<'a>(&'a self, args: Value) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            let url = args["url"].as_str().unwrap_or_default().to_owned();
            if url.contains("broken") {
                return Err(ToolError::Failed("HTTP 403".into()));
            }
            Ok(ToolOutput {
                content: format!("Page text of {url}"),
                meta: json!({"page": {"title": "A page", "url": url, "site": "a.org"}}),
                evidence_refs: vec![url],
            })
        })
    }
}

/// A tool that never finishes on its own.
#[derive(Debug)]
struct Stuck;

impl Tool for Stuck {
    fn name(&self) -> &'static str {
        "stuck"
    }
    fn summary(&self) -> &'static str {
        "Never finishes"
    }
    fn effect(&self) -> EffectClass {
        EffectClass::Read
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object"})
    }
    fn timeout(&self) -> Duration {
        Duration::from_secs(600)
    }
    fn call<'a>(&'a self, _args: Value) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async {
            tokio::time::sleep(Duration::from_secs(600)).await;
            Ok(ToolOutput::text("late"))
        })
    }
}

fn read(url: &str) -> Step {
    Step::Reply(format!(
        "```tool\n{}\n```",
        json!({"tool": "read_url", "args": {"url": url}})
    ))
}

struct Ctx<'a> {
    tools: &'a ToolRegistry,
    sink: &'a MemorySink,
    session: SessionId,
    budget: Budget,
    cancel: CancelFlag,
    hooks: RunHooks<'a>,
    stream: bool,
}

impl<'a> Ctx<'a> {
    fn new(tools: &'a ToolRegistry, sink: &'a MemorySink) -> Self {
        Self {
            tools,
            sink,
            session: SessionId::new(),
            budget: Budget::default(),
            cancel: CancelFlag::default(),
            // Each run calibrates its own estimator, so tests running in
            // parallel never skew each other's estimates (#85, A3b).
            hooks: RunHooks {
                estimator: Some(Box::leak(Box::new(Estimator::new()))),
                ..RunHooks::default()
            },
            stream: true,
        }
    }

    async fn run(&self, model: &Script, msg: &str) -> TurnOutcome {
        let mut gate = PolicyGate::default();
        let on_text = |_: u32, _: &str| {};
        TurnLoop {
            session: self.session,
            model,
            model_name: "test-model",
            tools: self.tools,
            gate: &mut gate,
            approver: &DenyAll,
            sink: self.sink,
            ledger: &UsageLedger::default(),
            budget: self.budget,
            cancel: self.cancel.clone(),
            instructions: None,
            max_output_tokens: 512,
            on_text: if self.stream { Some(&on_text) } else { None },
            hooks: self.hooks,
        }
        .run(Vec::new(), msg)
        .await
    }
}

fn kinds(events: &[EventEnvelope]) -> Vec<String> {
    events.iter().map(|e| e.kind.clone()).collect()
}

fn receipt(events: &[EventEnvelope]) -> TurnReceipt {
    let e = events
        .iter()
        .rev()
        .find(|e| e.kind == "turn.receipt")
        .expect("a receipt");
    serde_json::from_value(e.payload.clone()).expect("a receipt payload")
}

fn reader() -> (ToolRegistry, std::sync::Arc<Read>) {
    let r = std::sync::Arc::new(Read::default());
    let mut tools = ToolRegistry::new();
    tools.register(ArcRead(r.clone())).register(Stuck);
    (tools, r)
}

#[derive(Debug)]
struct ArcRead(std::sync::Arc<Read>);

impl Tool for ArcRead {
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

/// A-T2: a 429 with Retry-After 10 after seven tool results is one
/// visible wait, and the same turn goes on; the sources read never go down.
#[tokio::test(start_paused = true)]
async fn a_short_rate_limit_waits_once_and_the_turn_continues() {
    let (tools, reads) = reader();
    let sink = MemorySink::default();
    let ctx = Ctx::new(&tools, &sink);
    let mut steps: Vec<Step> = (0..7)
        .map(|i| read(&format!("https://a.org/{i}")))
        .collect();
    steps.push(Step::Fail(ProviderError::RateLimited {
        provider: "Mistral",
        retry_after: Some(10),
        detail: "Rate limit reached for tokens per minute".into(),
    }));
    steps.push(Step::Reply("FLARE retrieves when unsure.".into()));
    let model = Script::new(steps);
    let budget = Budget {
        max_model_calls: 20,
        ..Budget::default()
    };
    let ctx = Ctx { budget, ..ctx };
    let out = ctx.run(&model, "How does FLARE work?").await;
    assert_eq!(out.state, TurnState::Completed, "{:?}", out.reason);
    assert_eq!(reads.0.load(Ordering::SeqCst), 7);
    let events = sink.events();
    let waits: Vec<_> = events
        .iter()
        .filter(|e| e.kind == "provider.retry")
        .collect();
    assert_eq!(waits.len(), 1, "one wait");
    assert_eq!(waits[0].payload["wait_ms"], 10_000);
    // The ledger (pages read) never goes down within the turn.
    let mut read_so_far = 0;
    let mut counts = Vec::new();
    for e in &events {
        if e.kind == "tool.executed" {
            read_so_far += 1;
        }
        counts.push(read_so_far);
    }
    assert!(counts.windows(2).all(|w| w[0] <= w[1]));
    assert_eq!(read_so_far, 7);
    assert!(events.iter().all(|e| e.turn_id == Some(out.turn_id)));
}

/// A-T3: a daily limit is not retried; the turn pauses with what it kept,
/// and Continue on another model finishes the same turn id.
#[tokio::test]
async fn a_daily_limit_pauses_and_another_model_continues_the_same_turn() {
    let (tools, reads) = reader();
    let sink = MemorySink::default();
    let saved = MemoryCheckpoints::default();
    let mut ctx = Ctx::new(&tools, &sink);
    ctx.hooks.checkpoints = Some(&saved);
    let model = Script::new(vec![
        read("https://a.org/flare"),
        read("https://a.org/selfrag"),
        Step::Fail(ProviderError::RateLimited {
            provider: "Mistral",
            retry_after: None,
            detail: "Rate limit reached on requests per day (RPD): Limit 1000, Used 1000".into(),
        }),
    ]);
    let out = ctx.run(&model, "Compare FLARE and Self-RAG").await;
    assert_eq!(out.state, TurnState::Paused, "{:?}", out.reason);
    assert!(
        out.reason
            .as_deref()
            .unwrap_or_default()
            .contains("daily limit")
    );
    let events = sink.events();
    assert!(
        !kinds(&events).contains(&"provider.retry".to_owned()),
        "never retried"
    );
    assert_eq!(model.requests().len(), 3);
    let r = receipt(&events);
    assert_eq!(r.state, tern_core::RunState::Paused);
    assert_eq!(r.paused_by, Some(tern_core::PausedBy::DailyQuota));
    assert_eq!(r.error, Some(tern_core::ErrorClass::RateLimitDaily));
    assert_eq!(
        r.kept.sources,
        ["https://a.org/flare", "https://a.org/selfrag"]
    );
    assert_eq!(r.kept.tool_results, 2);
    assert!(r.resumable);
    assert!(r.resets_at.is_some());

    let ck = saved.latest().expect("a checkpoint");
    let other = Script::new(vec![Step::Reply("Both retrieve adaptively.".into())]);
    let sink2 = MemorySink::default();
    let mut ctx2 = Ctx::new(&tools, &sink2);
    ctx2.session = ctx.session;
    ctx2.hooks.checkpoints = Some(&saved);
    ctx2.hooks.resume = Some(&ck);
    let done = ctx2.run(&other, "Compare FLARE and Self-RAG").await;
    assert_eq!(done.state, TurnState::Completed, "{:?}", done.reason);
    assert_eq!(done.turn_id, out.turn_id, "the same turn");
    assert!(
        sink2
            .events()
            .iter()
            .all(|e| e.turn_id == Some(out.turn_id))
    );
    assert_eq!(reads.0.load(Ordering::SeqCst), 2, "no read ran again");
}

/// A-T4: a stream dropped after 400 characters keeps them; Continue sends
/// them back as the start of the answer and runs no tool again.
#[tokio::test]
async fn a_dropped_stream_resumes_with_its_prefix_and_no_tool_reruns() {
    let (tools, reads) = reader();
    let sink = MemorySink::default();
    let saved = MemoryCheckpoints::default();
    let mut ctx = Ctx::new(&tools, &sink);
    ctx.hooks.checkpoints = Some(&saved);
    let partial = "x".repeat(400);
    let model = Script::new(vec![
        read("https://a.org/flare"),
        Step::DropAfter(partial.clone()),
    ]);
    let out = ctx.run(&model, "Explain FLARE").await;
    assert_eq!(out.state, TurnState::Paused, "{:?}", out.reason);
    let events = sink.events();
    let end = events.iter().find(|e| e.kind == "turn.finished").unwrap();
    assert_eq!(
        end.payload["partial"],
        partial.as_str(),
        "partial persisted"
    );
    let r = receipt(&events);
    assert_eq!(r.error, Some(tern_core::ErrorClass::StreamCut));
    assert_eq!(r.kept.answer_chars, 400);
    let turns = tern_agent::transcript::past_turns(&events);
    assert_eq!(turns[0].answer.as_deref(), Some(partial.as_str()));

    let ck = saved.latest().expect("a checkpoint");
    assert_eq!(ck.quick.as_ref().unwrap().partial, partial);
    assert_eq!(ck.quick.as_ref().unwrap().journal.len(), 1);
    // The resumed model asks for the same read again, then finishes.
    let resumed = Script::new(vec![
        read("https://a.org/flare"),
        Step::Reply(" and the rest.".into()),
    ]);
    let sink2 = MemorySink::default();
    let mut ctx2 = Ctx::new(&tools, &sink2);
    ctx2.session = ctx.session;
    ctx2.hooks.checkpoints = Some(&saved);
    ctx2.hooks.resume = Some(&ck);
    let done = ctx2.run(&resumed, "Explain FLARE").await;
    assert_eq!(done.state, TurnState::Completed, "{:?}", done.reason);
    assert_eq!(done.answer.unwrap(), format!("{partial} and the rest."));
    let first = &resumed.requests()[0];
    let n = first.messages.len();
    assert_eq!(first.messages[n - 2].content, partial, "prefix re-sent");
    assert_eq!(first.messages[n - 1].content, RESUME_PROMPT);
    // Journal hits equal the tools the stopped turn ran: nothing re-ran.
    let replayed = sink2
        .events()
        .iter()
        .filter(|e| e.kind == "tool.replayed")
        .count();
    assert_eq!(replayed, 1);
    assert_eq!(reads.0.load(Ordering::SeqCst), 1);
}

/// A-T9: the budget runs out and the reserved call still answers, with
/// the gaps listed; a failed call is not counted.
#[tokio::test]
async fn a_spent_budget_still_yields_an_answer_with_gaps() {
    let (tools, reads) = reader();
    let sink = MemorySink::default();
    let mut ctx = Ctx::new(&tools, &sink);
    ctx.hooks.wrap_up = true;
    ctx.budget = Budget {
        max_model_calls: 8,
        max_tool_calls: 2,
        max_tokens: 200_000,
    };
    let model = Script::new(vec![
        read("https://a.org/broken"),
        read("https://a.org/1"),
        read("https://a.org/2"),
        read("https://a.org/3"),
        Step::Reply("FLARE answer.\n\nNot established: its cost.".into()),
    ]);
    let out = ctx.run(&model, "How does FLARE work?").await;
    assert_eq!(out.state, TurnState::CompletedWithGaps, "{:?}", out.reason);
    assert!(out.answer.unwrap().contains("Not established"));
    // The 403 did not count: two reads ran after it within a budget of two.
    assert_eq!(reads.0.load(Ordering::SeqCst), 3);
    assert_eq!(out.tool_calls, 2);
    let events = sink.events();
    assert!(kinds(&events).contains(&"budget.wrap_up".to_owned()));
    let failed = events.iter().find(|e| e.kind == "tool.failed").unwrap();
    assert_eq!(failed.payload["counted"], false);
    let last = model.requests().pop().unwrap();
    assert!(last.tools.is_empty(), "the wrap-up offers no tools");
    assert!(
        last.messages
            .last()
            .unwrap()
            .content
            .contains(WRAP_UP_PROMPT)
    );
    let end = events.iter().find(|e| e.kind == "turn.finished").unwrap();
    assert_eq!(end.payload["failed_not_counted"], 1);
}

/// Without the reserve (the default for bare loops), the old stop holds.
#[tokio::test]
async fn without_a_reserve_the_budget_still_stops_the_turn() {
    let (tools, _) = reader();
    let sink = MemorySink::default();
    let mut ctx = Ctx::new(&tools, &sink);
    ctx.budget.max_tool_calls = 1;
    let model = Script::new(vec![read("https://a.org/1"), read("https://a.org/2")]);
    let out = ctx.run(&model, "go").await;
    assert_eq!(out.state, TurnState::BudgetExceeded);
    let r = receipt(&sink.events());
    assert_eq!(r.paused_by, Some(tern_core::PausedBy::Budget));
}

/// A-T13: Stop changes the state within 2 s on a silent stream, and on a
/// tool that never returns.
#[tokio::test]
async fn stop_lands_within_two_seconds_on_a_silent_stream_and_a_stuck_tool() {
    let (tools, _) = reader();
    for steps in [
        vec![Step::Hang],
        vec![Step::Reply(
            "```tool\n{\"tool\": \"stuck\", \"args\": {}}\n```".into(),
        )],
    ] {
        let sink = MemorySink::default();
        let ctx = Ctx::new(&tools, &sink);
        let model = Script::new(steps);
        let cancel = ctx.cancel.clone();
        let started = Instant::now();
        let (out, ()) = tokio::join!(ctx.run(&model, "go"), async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            cancel.cancel();
        });
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(out.state, TurnState::Cancelled);
        let r = receipt(&sink.events());
        assert_eq!(r.paused_by, Some(tern_core::PausedBy::User));
        assert_eq!(r.state, tern_core::RunState::Paused);
    }
}

/// A3: the governor makes a call wait for its window (one visible
/// `governor.wait` row) and shrinks a request bigger than the window
/// before sending it.
#[tokio::test(start_paused = true)]
async fn the_governor_waits_for_the_window_and_prefits_big_requests() {
    let (tools, _) = reader();
    let sink = MemorySink::default();
    let gov = Governor::new();
    gov.observe(
        "script",
        "test-model",
        [
            ("x-ratelimit-limit-requests", "1"),
            ("x-ratelimit-remaining-requests", "0"),
        ],
    );
    let mut ctx = Ctx::new(&tools, &sink);
    ctx.hooks.governor = Some(&gov);
    let model = Script::new(vec![Step::Reply("Done.".into())]);
    let out = ctx.run(&model, "go").await;
    assert_eq!(out.state, TurnState::Completed, "{:?}", out.reason);
    let events = sink.events();
    let wait = events
        .iter()
        .find(|e| e.kind == "governor.wait")
        .expect("a wait");
    assert_eq!(wait.payload["run_state"], "waiting");
    assert!(wait.payload["wait_ms"].as_u64().unwrap() > 0);

    let big = Governor::new();
    big.observe(
        "script",
        "test-model",
        [("x-ratelimit-limit-tokens", "3000")],
    );
    let sink = MemorySink::default();
    let mut ctx = Ctx::new(&tools, &sink);
    ctx.hooks.governor = Some(&big);
    let model = Script::new(vec![Step::Reply("Done.".into())]);
    let long = "word ".repeat(4_000);
    let out = ctx.run(&model, &long).await;
    assert_eq!(out.state, TurnState::Completed, "{:?}", out.reason);
    let trim = sink
        .events()
        .into_iter()
        .find(|e| e.kind == "context.trimmed")
        .expect("a pre-fit");
    assert_eq!(trim.payload["tier"], "prefit");
    let sent = &model.requests()[0];
    let chars: usize = sent.messages.iter().map(|m| m.content.len()).sum();
    assert!(chars < long.len() / 2, "sent {chars} chars");
}

/// A4 / A-T8 / consistency S2: a daily limit hands the rest of this one
/// turn to the within-provider fallback, with a `model.switched` row; the
/// next turn starts on the chosen model again (never sticky).
#[tokio::test]
async fn a_fallback_lasts_one_turn_and_the_next_turn_uses_the_chosen_model() {
    let (tools, _) = reader();
    let sink = MemorySink::default();
    let mut ctx = Ctx::new(&tools, &sink);
    ctx.hooks.fallback_model = Some("small-model");
    let model = Script::new(vec![
        Step::Fail(ProviderError::RateLimited {
            provider: "Mistral",
            retry_after: None,
            detail: "requests per day (RPD) exceeded".into(),
        }),
        Step::Reply("Answer from the small model.".into()),
        Step::Reply("Next turn.".into()),
    ]);
    let out = ctx.run(&model, "first").await;
    assert_eq!(out.state, TurnState::Completed, "{:?}", out.reason);
    let reqs = model.requests();
    assert_eq!(reqs[0].model, "test-model");
    assert_eq!(reqs[1].model, "small-model");
    let events = sink.events();
    let sw = events
        .iter()
        .find(|e| e.kind == "model.switched")
        .expect("a row");
    let sw: tern_core::ModelSwitched = serde_json::from_value(sw.payload.clone()).unwrap();
    assert_eq!(
        (sw.from_model.as_str(), sw.to_model.as_str()),
        ("test-model", "small-model")
    );
    assert!(!sw.chosen_by_user);
    assert_eq!(out.model_calls, 1, "the failed call is not counted");
    // A new turn (or a new chat) starts on the chosen model.
    let next = ctx.run(&model, "second").await;
    assert_eq!(next.state, TurnState::Completed);
    assert_eq!(model.requests()[2].model, "test-model");
}

/// A7 / A-T8 / consistency S3: two chats with the same tools and mode get
/// the same fingerprint (tools hash, prompt hash) and the same pinned
/// sampling on every request.
#[tokio::test]
async fn the_same_tools_and_mode_give_the_same_fingerprint_and_sampling() {
    let (tools, _) = reader();
    let mut prints = Vec::new();
    for _ in 0..2 {
        let sink = MemorySink::default();
        let mut ctx = Ctx::new(&tools, &sink);
        ctx.hooks.temperature = Some(0.3);
        let model = Script::new(vec![Step::Reply("Done.".into())]);
        ctx.run(&model, "go").await;
        assert!(model.requests().iter().all(|r| r.temperature == Some(0.3)));
        let started = sink
            .events()
            .into_iter()
            .find(|e| e.kind == "turn.started")
            .unwrap();
        prints.push(started.payload["fingerprint"].clone());
    }
    assert_eq!(prints[0], prints[1]);
    assert!(
        prints[0]["tools_hash"]
            .as_str()
            .is_some_and(|h| h.len() == 16)
    );
    // Registration order does not change the hash.
    let mut other = ToolRegistry::new();
    other.register(Stuck).register(Read::default());
    assert_eq!(other.tools_hash(), tools.tools_hash());
}

/// A-T16: with native tools the system text carries no input schemas.
#[tokio::test]
async fn native_tool_requests_do_not_repeat_the_schemas_in_the_system_text() {
    let (tools, _) = reader();
    let sink = MemorySink::default();
    let mut ctx = Ctx::new(&tools, &sink);
    ctx.hooks.native_tools = true;
    let model = Script::new(vec![Step::Reply("Done.".into())]);
    ctx.run(&model, "go").await;
    let req = &model.requests()[0];
    assert!(!req.tools.is_empty());
    let system = req.system.as_deref().unwrap();
    assert!(system.contains("You can use these tools:"));
    assert!(!system.contains("input schema:"), "{system}");
    // Without native tools the schemas stay in the text.
    let sink = MemorySink::default();
    let ctx = Ctx::new(&tools, &sink);
    let model = Script::new(vec![Step::Reply("Done.".into())]);
    ctx.run(&model, "go").await;
    assert!(
        model.requests()[0]
            .system
            .as_deref()
            .unwrap()
            .contains("input schema:")
    );
}

/// A paper reader that counts its reads and returns a long paper.
#[derive(Debug, Default)]
struct Papers(AtomicUsize);

impl Tool for Papers {
    fn name(&self) -> &'static str {
        "paper_read"
    }
    fn summary(&self) -> &'static str {
        "Read a paper"
    }
    fn effect(&self) -> EffectClass {
        EffectClass::Network
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object", "required": ["paper"],
               "properties": {"paper": {"type": "string"}, "question": {"type": "string"}}})
    }
    fn call<'a>(&'a self, args: Value) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            let id = args["paper"].as_str().unwrap_or_default().to_owned();
            Ok(ToolOutput {
                content: format!("[p. 1 · Abstract] {id} ") + &"retrieval passage ".repeat(1_800),
                meta: json!({"page": {"title": id, "url": format!("https://arxiv.org/abs/{id}"), "site": "arxiv.org"}}),
                evidence_refs: vec![id],
            })
        })
    }
}

fn calls(blocks: &[Value]) -> Step {
    Step::Reply(
        blocks
            .iter()
            .map(|b| format!("```tool\n{b}\n```"))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

fn paper(id: &str) -> Value {
    json!({"tool": "paper_read", "args": {"paper": id, "question": "how does FLARE decide when to retrieve"}})
}

/// A-T1: the FLARE turn that failed at 22:34 (five long papers asked for
/// in one reply on a 20k-token-a-minute key), replayed against a fake
/// provider with that window: papers are read a few per step, the turn
/// is paced and ends with an answer, and no request is larger than the
/// envelope.
#[tokio::test(start_paused = true)]
async fn the_flare_turn_is_paced_on_a_small_token_window() {
    let papers = std::sync::Arc::new(Papers::default());
    let mut tools = ToolRegistry::new();
    tools.register(ArcPapers(papers.clone()));
    let sink = MemorySink::default();
    let gov = Governor::new();
    gov.observe(
        "script",
        "test-model",
        [("x-ratelimit-limit-tokens", "20000")],
    );
    let envelope = gov.envelope("script", "test-model").expect("an envelope");
    let mut ctx = Ctx::new(&tools, &sink);
    ctx.hooks.governor = Some(&gov);
    ctx.hooks.wrap_up = true;
    ctx.budget = Budget {
        max_model_calls: 12,
        max_tool_calls: 12,
        max_tokens: 400_000,
    };
    let ids = [
        "2305.06983",
        "2310.11511",
        "2005.11401",
        "2212.10496",
        "2401.15884",
    ];
    let model = Script::new(vec![
        calls(&ids.iter().copied().map(paper).collect::<Vec<_>>()),
        calls(&ids[2..].iter().copied().map(paper).collect::<Vec<_>>()),
        calls(&ids[4..].iter().copied().map(paper).collect::<Vec<_>>()),
        Step::Reply("FLARE retrieves when a sentence has low-confidence tokens [1].".into()),
    ]);
    let out = ctx
        .run(&model, "How does FLARE decide when to retrieve?")
        .await;
    assert!(
        matches!(
            out.state,
            TurnState::Completed | TurnState::CompletedWithGaps
        ),
        "{:?} {:?}",
        out.state,
        out.reason
    );
    // Each paper read once, two per step on a 20k window.
    assert_eq!(papers.0.load(Ordering::SeqCst), 5);
    let events = sink.events();
    let deferred = events
        .iter()
        .filter(|e| e.kind == "tool.skipped" && e.payload["reason"] == "next_batch")
        .count();
    assert_eq!(deferred, 3 + 1, "3 waited in step one, 1 in step two");
    // No request went over the envelope (about four characters a token).
    for r in model.requests() {
        let tokens = (sent_chars(&r) as u64).div_ceil(4);
        assert!(tokens <= envelope, "a request of {tokens} > {envelope}");
    }
}

#[derive(Debug)]
struct ArcPapers(std::sync::Arc<Papers>);

impl Tool for ArcPapers {
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

/// A6: a paper asked for twice in one turn is read once.
#[tokio::test]
async fn a_paper_is_read_once_per_turn() {
    let papers = std::sync::Arc::new(Papers::default());
    let mut tools = ToolRegistry::new();
    tools.register(ArcPapers(papers.clone()));
    let sink = MemorySink::default();
    let ctx = Ctx::new(&tools, &sink);
    let model = Script::new(vec![
        calls(&[paper("arXiv:2305.06983")]),
        calls(&[paper("https://arxiv.org/abs/2305.06983")]),
        Step::Reply("Done.".into()),
    ]);
    let out = ctx.run(&model, "Read FLARE").await;
    assert_eq!(out.state, TurnState::Completed, "{:?}", out.reason);
    assert_eq!(papers.0.load(Ordering::SeqCst), 1);
    assert_eq!(out.tool_calls, 1);
    assert!(sink.events().iter().any(|e| e.kind == "tool.skipped"
        && e.payload["reason"] == "already_read"
        && e.payload["counted"] == false));
}

/// A-T6: arguments sent as a JSON string or inside a `{tool, args}`
/// envelope run; a call with bad arguments is refused and not counted.
#[tokio::test]
async fn string_and_envelope_arguments_run_and_bad_ones_are_not_counted() {
    let (tools, reads) = reader();
    let sink = MemorySink::default();
    let mut ctx = Ctx::new(&tools, &sink);
    ctx.budget.max_tool_calls = 2;
    let model = Script::new(vec![
        calls(&[json!({"tool": "read_url", "args": {}})]),
        calls(&[json!({"tool": "read_url", "args": "{\"url\": \"https://a.org/1\"}"})]),
        calls(&[
            json!({"tool": "read_url", "args": {"tool": "read_url", "args": {"url": "https://a.org/2"}}}),
        ]),
        Step::Reply("Both pages read.".into()),
    ]);
    let out = ctx.run(&model, "read two pages").await;
    assert_eq!(out.state, TurnState::Completed, "{:?}", out.reason);
    assert_eq!(reads.0.load(Ordering::SeqCst), 2);
    assert_eq!(out.tool_calls, 2);
    let events = sink.events();
    assert_eq!(
        events.iter().filter(|e| e.kind == "tool.rejected").count(),
        1
    );
    let urls: Vec<_> = events
        .iter()
        .filter(|e| e.kind == "tool.executed")
        .map(|e| {
            e.payload["args"]["url"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
        .collect();
    assert_eq!(urls, ["https://a.org/1", "https://a.org/2"]);
}

/// A-T7: the same address failing with a 403 is fetched once in a turn,
/// however the model writes it again (a trailing slash, a fragment).
#[tokio::test]
async fn a_failed_address_is_fetched_once_per_turn() {
    let (tools, reads) = reader();
    let sink = MemorySink::default();
    let ctx = Ctx::new(&tools, &sink);
    let model = Script::new(vec![
        read("https://x.org/broken"),
        read("https://x.org/broken/"),
        read("https://x.org/broken#top"),
        Step::Reply("That page refused Tern; answered from others.".into()),
    ]);
    let out = ctx.run(&model, "read it").await;
    assert_eq!(out.state, TurnState::Completed, "{:?}", out.reason);
    assert_eq!(reads.0.load(Ordering::SeqCst), 1);
    let events = sink.events();
    let skipped: Vec<_> = events.iter().filter(|e| e.kind == "tool.skipped").collect();
    assert_eq!(skipped.len(), 2);
    assert!(
        skipped
            .iter()
            .all(|e| e.payload["reason"] == "failed_before" && e.payload["counted"] == false)
    );
    assert_eq!(
        out.tool_calls, 0,
        "failed and skipped calls are not counted"
    );
}

/// A17: chaos switches sit in front of the model call: `bad-args` sends
/// the next call's arguments as a JSON string (read back by A6), `429`
/// fails the call after it once with a short rate limit, retried.
#[tokio::test(start_paused = true)]
async fn chaos_faults_reach_the_model_call() {
    let (tools, reads) = reader();
    let sink = MemorySink::default();
    let chaos = tern_agent::chaos::Chaos::new("bad-args,429");
    let mut ctx = Ctx::new(&tools, &sink);
    ctx.hooks.chaos = Some(&chaos);
    let model = Script::new(vec![
        read("https://a.org/1"),
        Step::Reply("Read it.".into()),
    ]);
    let out = ctx.run(&model, "read it").await;
    assert_eq!(out.state, TurnState::Completed, "{:?}", out.reason);
    assert_eq!(reads.0.load(Ordering::SeqCst), 1);
    assert_eq!(chaos.remaining(), 0);
    assert!(
        sink.events().iter().any(|e| e.kind == "provider.retry"),
        "the simulated 429 was waited out"
    );
}
