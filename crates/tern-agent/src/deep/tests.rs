//! Deep research with a mock model, search, and reader: plan approval,
//! the concurrency cap, budgets, and cancelling one worker or the run.

use std::sync::atomic::{AtomicUsize, Ordering};

use tern_core::Measure;
use tern_crawl::SourceIcon;
use tern_providers::{ChatResponse, ProviderResult};

use super::*;
use crate::steer::Steering;

use crate::engine::MemorySink;
use crate::policy::{Mode, PolicyConfig};
use crate::run::Run;

const PLAN: &str = r#"{"sub_questions": [
 {"question": "alpha part", "queries": ["alpha"], "sources": ["papers"], "share": 40},
 {"question": "beta part", "queries": ["beta"], "sources": ["news"], "share": 30},
 {"question": "gamma part", "queries": ["gamma"], "sources": ["docs"], "share": 30}
]}"#;

struct Mock {
    plan: String,
    /// Tokens a notes call reports using (input).
    notes_tokens: u64,
    /// Worker whose notes ask for one more search.
    again: Option<&'static str>,
    /// Worker whose notes never come back (the app is killed meanwhile).
    hang: Option<&'static str>,
    calls: AtomicUsize,
    prompts: std::sync::Mutex<Vec<String>>,
}

impl Default for Mock {
    fn default() -> Self {
        Self {
            plan: PLAN.into(),
            notes_tokens: 1_000,
            again: None,
            hang: None,
            calls: AtomicUsize::new(0),
            prompts: std::sync::Mutex::default(),
        }
    }
}

impl Mock {
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

fn reply(text: String, input: u64) -> ProviderResult<ChatResponse> {
    Ok(ChatResponse {
        text,
        usage: Usage {
            input_tokens: input,
            output_tokens: 100,
            measure: Measure::Measured,
        },
        finish_reason: None,
        model: Some("mock".into()),
    })
}

impl Model for Mock {
    fn provider_id(&self) -> &str {
        "mock"
    }
    fn complete<'a>(&'a self, req: &'a ChatRequest) -> BoxFuture<'a, ProviderResult<ChatResponse>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.prompts
            .lock()
            .unwrap()
            .push(req.messages[0].content.clone());
        Box::pin(async move {
            let system = req.system.clone().unwrap_or_default();
            let message = req.messages[0].content.clone();
            if system.contains("You plan research") {
                return reply(self.plan.clone(), 300);
            }
            if system.contains("research workers") {
                let sub = message
                    .lines()
                    .find_map(|l| l.strip_prefix("Your sub-question: "))
                    .unwrap_or_default()
                    .to_owned();
                if self.hang.is_some_and(|h| sub.starts_with(h)) {
                    std::future::pending::<()>().await;
                }
                let sources = message.lines().filter(|l| l.starts_with('[')).count();
                let mut text = format!("{sub} fact one [1].");
                if sources > 1 {
                    text.push_str(&format!(" {sub} fact two [2]."));
                }
                if self.again.is_some_and(|a| sub.starts_with(a))
                    && system.contains(NEXT_SEARCH_LINE)
                {
                    text.push_str(&format!("\nNEXT SEARCH: more {sub}"));
                }
                return reply(text, self.notes_tokens);
            }
            // The synthesis: cite the last source first, then the first,
            // then one that does not exist.
            let n = message
                .lines()
                .skip_while(|l| *l != "Sources:")
                .filter(|l| l.starts_with('['))
                .count();
            reply(format!("Overall [{n}]. Also [1]. Wrong [42]."), 2_000)
        })
    }
}

const NEXT_SEARCH_LINE: &str = tern_research::deep::NEXT_SEARCH;

struct MockSearch;

impl Searcher for MockSearch {
    fn id(&self) -> &str {
        "mock-search"
    }
    fn search<'a>(
        &'a self,
        query: &'a str,
        _limit: usize,
    ) -> BoxFuture<'a, Result<Vec<SearchHit>, String>> {
        Box::pin(async move {
            if query.contains("nothing") {
                return Ok(Vec::new());
            }
            let slug: String = query.chars().filter(char::is_ascii_alphanumeric).collect();
            Ok(["a", "b", "c", "d"]
                .iter()
                .map(|s| {
                    let site = format!("{slug}-{s}.test");
                    SearchHit {
                        title: format!("{site} page"),
                        url: format!("https://{site}/page"),
                        snippet: String::new(),
                        site,
                        icon: SourceIcon {
                            builtin: None,
                            favicon: None,
                        },
                    }
                })
                .collect())
        })
    }
}

#[derive(Default)]
struct MockReader {
    delay_ms: u64,
    reads: AtomicUsize,
    /// Cancel this worker when a URL containing the text is read.
    cancel_on: Option<(&'static str, usize, DeepControl)>,
    /// Cancel the whole run when a URL containing the text is read.
    cancel_all_on: Option<(&'static str, DeepControl)>,
}

impl Reader for MockReader {
    fn read<'a>(&'a self, url: &'a str) -> BoxFuture<'a, Result<ReadPage, String>> {
        Box::pin(async move {
            self.reads.fetch_add(1, Ordering::SeqCst);
            if let Some((needle, n, c)) = &self.cancel_on
                && url.contains(needle)
            {
                c.cancel_worker(*n);
            }
            if let Some((needle, c)) = &self.cancel_all_on
                && url.contains(needle)
            {
                c.cancel_all();
            }
            tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
            if url.contains("gamma-c.test") {
                return Err(
                    "gamma-c.test's robots.txt asks automated readers not to open this page".into(),
                );
            }
            let site = url
                .trim_start_matches("https://")
                .split('/')
                .next()
                .unwrap_or_default()
                .to_owned();
            Ok(ReadPage {
                page: PageRef {
                    title: format!("{site} page"),
                    url: url.to_owned(),
                    site,
                    builtin: None,
                    favicon: None,
                },
                text: format!("Page text for {url}."),
            })
        })
    }
}

/// Answers the plan with a function of it.
struct Answer(Box<dyn Fn(&ResearchPlan) -> PlanAnswer + Send + Sync>);

impl PlanApprover for Answer {
    fn review<'a>(&'a self, plan: &'a ResearchPlan) -> BoxFuture<'a, PlanAnswer> {
        let a = (self.0)(plan);
        Box::pin(async move { a })
    }
}

struct Fixture {
    sink: MemorySink,
    ledger: UsageLedger,
}

impl Fixture {
    fn new() -> Self {
        Self {
            sink: MemorySink::default(),
            ledger: UsageLedger::default(),
        }
    }

    async fn run(
        &self,
        model: &Mock,
        reader: &MockReader,
        opts: DeepOptions,
        approver: &dyn PlanApprover,
        control: &DeepControl,
        gate: Option<&mut PolicyGate>,
    ) -> TurnOutcome {
        let deps = DeepDeps {
            session: SessionId::new(),
            model,
            model_name: "mock",
            search: &MockSearch,
            reader,
            sink: &self.sink,
            ledger: &self.ledger,
            gate,
            on_text: None,
            checkpoints: None,
            steering: None,
            decider: None,
        };
        run_deep(
            deps,
            "How do alpha, beta, and gamma relate?",
            &opts,
            approver,
            control,
        )
        .await
    }

    fn kinds(&self) -> Vec<String> {
        self.sink.events().into_iter().map(|e| e.kind).collect()
    }

    fn finished(&self) -> Vec<Value> {
        self.sink
            .events()
            .into_iter()
            .filter(|e| e.kind == "worker.finished")
            .map(|e| e.payload)
            .collect()
    }

    fn status_of(&self, n: usize) -> String {
        self.finished()
            .iter()
            .find(|p| p["worker"] == n)
            .map(|p| p["status"].as_str().unwrap_or_default().to_owned())
            .unwrap_or_default()
    }

    /// Most workers running at once, from the order of their events.
    fn peak(&self) -> usize {
        let (mut now, mut peak) = (0usize, 0usize);
        for e in self.sink.events() {
            match e.kind.as_str() {
                "worker.started" => {
                    now += 1;
                    peak = peak.max(now);
                }
                "worker.finished" => now = now.saturating_sub(1),
                _ => {}
            }
        }
        peak
    }

    fn run_view(&self) -> Run {
        let mut run = Run::new();
        for e in self.sink.events() {
            run.push(&e);
        }
        run
    }
}

fn opts(workers: usize) -> DeepOptions {
    DeepOptions {
        workers,
        ..DeepOptions::default()
    }
}

#[tokio::test]
async fn a_plan_runs_workers_and_one_cited_answer() {
    let f = Fixture::new();
    let reader = MockReader {
        delay_ms: 20,
        ..MockReader::default()
    };
    let out = f
        .run(
            &Mock::default(),
            &reader,
            opts(2),
            &RunAsDrafted,
            &DeepControl::default(),
            None,
        )
        .await;
    assert_eq!(out.state, TurnState::Completed, "{:?}", out.reason);
    let answer = out.answer.unwrap();
    // The synthesis cited the last merged page first, then the first; the
    // made-up [42] is gone and the list follows the new numbers.
    assert!(
        answer.starts_with("Overall [1]. Also [2]. Wrong."),
        "{answer}"
    );
    let list = tern_research::claims::link_list(&answer);
    assert_eq!(list.len(), 2);
    assert_eq!(list[1].1, "https://alpha-a.test/page");
    let kinds = f.kinds();
    for k in [
        "turn.started",
        "plan.drafted",
        "plan.answered",
        "worker.started",
        "worker.finished",
        "turn.finished",
    ] {
        assert!(kinds.iter().any(|x| x == k), "no {k} in {kinds:?}");
    }
    let first_read = kinds.iter().position(|k| k == "tool.executed").unwrap();
    let answered = kinds.iter().position(|k| k == "plan.answered").unwrap();
    assert!(answered < first_read, "nothing is searched before approval");
    assert_eq!(f.finished().len(), 3);
    assert!((1..=3).all(|n| f.status_of(n) == "done"));
    assert_eq!(f.peak(), 2);
    // Each worker read three pages, except one robots.txt refused.
    let run = f.run_view();
    let deep = run.deep.clone().expect("a deep run");
    assert_eq!(deep.plan.as_ref().unwrap().sub_questions.len(), 3);
    assert_eq!(deep.workers.len(), 3);
    assert!(deep.workers.iter().all(|w| w.status == WorkerStatus::Done));
    let pages: Vec<usize> = deep.workers.iter().map(|w| w.pages.len()).collect();
    assert_eq!(pages, [3, 3, 2]);
    assert!(
        deep.workers
            .iter()
            .all(|w| w.tokens_in > 0 && w.searches == 1)
    );
    assert_eq!(deep.plan_answer, Some(crate::run::PlanState::Approved));
    assert_eq!(out.model_calls, 1 + 3 + 1);
    // The report gets the plan and a section per worker.
    let past = crate::transcript::past_turns(&f.sink.events());
    let report = crate::report::from_turn(SessionId::new(), 1, &past[0], "mock", None);
    assert_eq!(report.plan.len(), 3);
    assert_eq!(report.workers.len(), 3);
    assert_eq!(report.workers[0].pages, 3);
    assert_eq!(
        report.usage.budget.unwrap().max_tokens,
        PlanBudget::default().tokens
    );
    assert!(report.to_markdown().contains("## Workers"));
    assert!(report.sources.iter().any(|s| s.robots == "blocked"));
}

#[tokio::test]
async fn never_more_workers_at_once_than_the_cap() {
    let plan = r#"["one?", "two?", "three?", "four?", "five?"]"#;
    for cap in [1, 3] {
        let f = Fixture::new();
        let model = Mock {
            plan: plan.into(),
            ..Mock::default()
        };
        let reader = MockReader {
            delay_ms: 15,
            ..MockReader::default()
        };
        let o = DeepOptions {
            max_sub_questions: 5,
            ..opts(cap)
        };
        let out = f
            .run(
                &model,
                &reader,
                o,
                &RunAsDrafted,
                &DeepControl::default(),
                None,
            )
            .await;
        assert_eq!(out.state, TurnState::Completed);
        assert_eq!(f.finished().len(), 5);
        assert_eq!(f.peak(), cap, "cap {cap}");
    }
}

#[tokio::test]
async fn one_worker_can_be_cancelled_while_the_rest_finish() {
    let f = Fixture::new();
    let control = DeepControl::default();
    let reader = MockReader {
        delay_ms: 300,
        cancel_on: Some(("beta", 2, control.clone())),
        ..MockReader::default()
    };
    let out = f
        .run(
            &Mock::default(),
            &reader,
            opts(3),
            &RunAsDrafted,
            &control,
            None,
        )
        .await;
    assert_eq!(out.state, TurnState::Completed, "{:?}", out.reason);
    assert_eq!(f.status_of(2), "cancelled");
    assert_eq!(f.status_of(1), "done");
    assert_eq!(f.status_of(3), "done");
    let answer = out.answer.unwrap();
    assert!(
        !answer.contains("beta-"),
        "a cancelled worker's pages are not cited"
    );
    let run = f.run_view();
    let w2 = &run.deep.unwrap().workers[1];
    assert_eq!(
        (w2.status, w2.note.as_str()),
        (WorkerStatus::Cancelled, "you stopped it")
    );
}

#[tokio::test]
async fn cancelling_the_run_stops_every_worker() {
    let f = Fixture::new();
    let control = DeepControl::default();
    let reader = MockReader {
        delay_ms: 300,
        cancel_all_on: Some(("gamma", control.clone())),
        ..MockReader::default()
    };
    let out = f
        .run(
            &Mock::default(),
            &reader,
            opts(3),
            &RunAsDrafted,
            &control,
            None,
        )
        .await;
    assert_eq!(out.state, TurnState::Cancelled);
    assert!(out.answer.is_none());
    assert!((1..=3).all(|n| f.status_of(n) == "cancelled"));
    assert_eq!(f.kinds().last().unwrap(), "run.cancelled");
}

#[tokio::test]
async fn the_plan_can_be_cancelled_skipped_or_edited() {
    // Cancel: nothing is searched or read.
    let f = Fixture::new();
    let reader = MockReader::default();
    let cancel = Answer(Box::new(|_| PlanAnswer::Cancel));
    let out = f
        .run(
            &Mock::default(),
            &reader,
            opts(3),
            &cancel,
            &DeepControl::default(),
            None,
        )
        .await;
    assert_eq!(out.state, TurnState::Cancelled);
    assert!(
        !f.kinds()
            .iter()
            .any(|k| k == "tool.executed" || k == "worker.started")
    );
    assert_eq!(reader.reads.load(Ordering::SeqCst), 0);

    // Skip: one worker on the whole question.
    let f = Fixture::new();
    let skip = Answer(Box::new(|_| PlanAnswer::Skip));
    let out = f
        .run(
            &Mock::default(),
            &MockReader::default(),
            opts(3),
            &skip,
            &DeepControl::default(),
            None,
        )
        .await;
    assert_eq!(out.state, TurnState::Completed);
    assert_eq!(f.finished().len(), 1);
    assert_eq!(
        f.run_view().deep.unwrap().plan_answer,
        Some(crate::run::PlanState::Skipped)
    );

    // Edit: drop a sub-question and reword another.
    let f = Fixture::new();
    let edit = Answer(Box::new(|p| {
        let mut p = p.clone();
        p.sub_questions.remove(1);
        p.sub_questions[0].question = "alpha, reworded".into();
        PlanAnswer::Run(p)
    }));
    let out = f
        .run(
            &Mock::default(),
            &MockReader::default(),
            opts(3),
            &edit,
            &DeepControl::default(),
            None,
        )
        .await;
    assert_eq!(out.state, TurnState::Completed);
    let answered = f
        .sink
        .events()
        .into_iter()
        .find(|e| e.kind == "plan.answered")
        .unwrap();
    assert_eq!(answered.payload["edited"], true);
    let finished = f.finished();
    assert_eq!(finished.len(), 2);
    assert_eq!(
        finished.iter().find(|p| p["worker"] == 1).unwrap()["question"],
        "alpha, reworded"
    );
    assert_eq!(
        f.run_view().deep.unwrap().plan_answer,
        Some(crate::run::PlanState::Edited)
    );
}

#[tokio::test]
async fn an_unreadable_plan_falls_back_to_tern_s_split() {
    let f = Fixture::new();
    let model = Mock {
        plan: "Sure! I will look into it.".into(),
        ..Mock::default()
    };
    let out = f
        .run(
            &model,
            &MockReader::default(),
            opts(3),
            &RunAsDrafted,
            &DeepControl::default(),
            None,
        )
        .await;
    assert_eq!(out.state, TurnState::Completed);
    let drafted = f
        .sink
        .events()
        .into_iter()
        .find(|e| e.kind == "plan.drafted")
        .unwrap();
    assert_eq!(drafted.payload["plan"]["source"], "fallback");
    assert!(
        drafted.payload["note"]
            .as_str()
            .unwrap()
            .contains("could not be read")
    );
}

#[tokio::test]
async fn workers_keep_to_their_slices() {
    // Tokens: each notes call reports more than a worker's slice.
    let f = Fixture::new();
    let model = Mock {
        notes_tokens: 9_600,
        ..Mock::default()
    };
    let o = DeepOptions {
        budget: PlanBudget {
            tokens: 30_000,
            pages: 6,
            seconds: 60,
        },
        ..opts(3)
    };
    let out = f
        .run(
            &model,
            &MockReader::default(),
            o,
            &RunAsDrafted,
            &DeepControl::default(),
            None,
        )
        .await;
    assert!(
        (1..=3).all(|n| f.status_of(n) == "over_budget"),
        "{:?}",
        f.finished()
    );
    // Notes written before the slice ran out still reach the answer.
    assert_eq!(out.state, TurnState::Completed, "{:?}", out.reason);
    // Pages: a worker never reads more than its slice (6 pages, 2 each).
    let run = f.run_view();
    let pages: Vec<usize> = run
        .deep
        .unwrap()
        .workers
        .iter()
        .map(|w| w.pages.len())
        .collect();
    assert!(
        pages.iter().zip([2, 2, 2]).all(|(p, max)| *p <= max),
        "{pages:?}"
    );

    // Time: a one-second slice against slow reads.
    let f = Fixture::new();
    let reader = MockReader {
        delay_ms: 1_500,
        ..MockReader::default()
    };
    let o = DeepOptions {
        budget: PlanBudget {
            tokens: 100_000,
            pages: 9,
            seconds: 1,
        },
        ..opts(3)
    };
    let out = f
        .run(
            &Mock::default(),
            &reader,
            o,
            &RunAsDrafted,
            &DeepControl::default(),
            None,
        )
        .await;
    assert!((1..=3).all(|n| f.status_of(n) == "timed_out"));
    assert_eq!(out.state, TurnState::Failed);
    assert!(out.reason.unwrap().contains("No worker found a page"));
}

#[tokio::test]
async fn a_worker_may_search_once_more_when_it_asks() {
    let f = Fixture::new();
    let model = Mock {
        again: Some("alpha"),
        ..Mock::default()
    };
    let out = f
        .run(
            &model,
            &MockReader::default(),
            opts(3),
            &RunAsDrafted,
            &DeepControl::default(),
            None,
        )
        .await;
    assert_eq!(out.state, TurnState::Completed);
    let run = f.run_view();
    let workers = run.deep.unwrap().workers;
    assert_eq!(workers[0].searches, 2);
    assert_eq!(workers[0].pages.len(), 6);
    assert_eq!(workers[1].searches, 1);
}

#[tokio::test]
async fn the_policy_gate_still_rules_network_calls() {
    let f = Fixture::new();
    let mut gate =
        PolicyGate::new(PolicyConfig::default().with_mode(EffectClass::Network, Mode::Deny));
    let out = f
        .run(
            &Mock::default(),
            &MockReader::default(),
            opts(3),
            &RunAsDrafted,
            &DeepControl::default(),
            Some(&mut gate),
        )
        .await;
    assert_eq!(out.state, TurnState::Failed);
    assert!((1..=3).all(|n| f.status_of(n) == "failed"));
    assert!(!f.kinds().iter().any(|k| k == "tool.executed"));
}

#[test]
fn controls_and_picks() {
    let c = DeepControl::default();
    c.cancel_worker(2);
    assert!(c.worker_cancelled(2) && !c.worker_cancelled(1) && !c.is_cancelled());
    c.cancel_all();
    assert!(c.worker_cancelled(1));
    let hit = |url: &str, site: &str| SearchHit {
        title: String::new(),
        url: url.into(),
        snippet: String::new(),
        site: site.into(),
        icon: SourceIcon {
            builtin: None,
            favicon: None,
        },
    };
    let hits = [
        hit("a1", "a"),
        hit("a2", "a"),
        hit("b1", "b"),
        hit("a1", "a"),
    ];
    let seen: HashSet<String> = ["b1".to_owned()].into();
    let urls: Vec<String> = pick(&hits, 3, &seen).into_iter().map(|h| h.url).collect();
    assert_eq!(urls, ["a1", "a2"]);
    assert_eq!(question_of("  What is\nRAG? "), "What is RAG?");
    let plan = fallback_plan("Why?", PlanBudget::default(), 5);
    assert_eq!(turn_budget(&plan).max_model_calls, 2 + 3 * ROUNDS as u32);
}

/// Counts searches on top of [`MockSearch`].
#[derive(Default)]
struct CountSearch {
    queries: std::sync::Mutex<Vec<String>>,
}

impl CountSearch {
    fn queries(&self) -> Vec<String> {
        self.queries.lock().unwrap().clone()
    }
}

impl Searcher for CountSearch {
    fn id(&self) -> &str {
        "mock-search"
    }
    fn search<'a>(
        &'a self,
        query: &'a str,
        limit: usize,
    ) -> BoxFuture<'a, Result<Vec<SearchHit>, String>> {
        self.queries.lock().unwrap().push(query.to_owned());
        MockSearch.search(query, limit)
    }
}

/// One run with checkpoints and steering wired in.
#[allow(clippy::too_many_arguments)]
async fn run_with(
    session: SessionId,
    sink: &MemorySink,
    model: &Mock,
    search: &dyn Searcher,
    reader: &MockReader,
    checkpoints: &crate::checkpoint::MemoryCheckpoints,
    steering: Option<&Steering>,
    resume: Option<RunCheckpoint>,
    control: &DeepControl,
    workers: usize,
) -> TurnOutcome {
    let ledger = UsageLedger::default();
    let deps = DeepDeps {
        session,
        model,
        model_name: "mock",
        search,
        reader,
        sink,
        ledger: &ledger,
        gate: None,
        on_text: None,
        checkpoints: Some(checkpoints),
        steering,
        decider: None,
    };
    match resume {
        Some(c) => resume_deep(deps, c, &opts(workers), control).await,
        None => {
            run_deep(
                deps,
                "How do alpha, beta, and gamma relate?",
                &opts(workers),
                &RunAsDrafted,
                control,
            )
            .await
        }
    }
}

fn view(sink: &MemorySink) -> Run {
    let mut run = Run::new();
    for e in sink.events() {
        run.push(&e);
    }
    run
}

#[tokio::test]
async fn a_killed_run_resumes_without_repeating_paid_work() {
    let session = SessionId::new();
    let saved = crate::checkpoint::MemoryCheckpoints::default();
    // First run: worker 2's notes never come back, and the app is killed
    // (the run's future is dropped) while it waits.
    let model = Mock {
        hang: Some("beta"),
        ..Mock::default()
    };
    let search = CountSearch::default();
    let reader = MockReader::default();
    let sink = MemorySink::default();
    let killed = tokio::time::timeout(
        Duration::from_millis(800),
        run_with(
            session,
            &sink,
            &model,
            &search,
            &reader,
            &saved,
            None,
            None,
            &DeepControl::default(),
            3,
        ),
    )
    .await;
    assert!(killed.is_err(), "the first run never finishes");
    assert_eq!(
        search.queries().len(),
        4,
        "the preflight and one per worker"
    );
    assert_eq!(reader.reads.load(Ordering::SeqCst), 9);
    let ck = saved.latest().expect("a checkpoint");
    assert_eq!(ck.version, crate::checkpoint::CHECKPOINT_VERSION);
    assert_eq!(ck.stage, crate::checkpoint::Stage::Workers);
    let d = ck.deep.as_ref().unwrap();
    let by_n = |n: usize| d.workers.iter().find(|w| w.n == n).unwrap();
    assert_eq!(by_n(1).finished, Some(WorkerStatus::Done));
    assert_eq!(by_n(3).finished, Some(WorkerStatus::Done));
    assert_eq!(by_n(2).finished, None);
    assert!(
        by_n(2).pending_notes,
        "its pages were read; its notes were not"
    );
    assert_eq!(by_n(2).pages.len(), 3);
    assert_eq!(
        ck.progress(),
        "Deep research · 2 of 3 done · 1 not finished"
    );

    // Resume with fresh mocks that count everything.
    let model2 = Mock::default();
    let search2 = CountSearch::default();
    let reader2 = MockReader::default();
    let sink2 = MemorySink::default();
    let out = run_with(
        session,
        &sink2,
        &model2,
        &search2,
        &reader2,
        &saved,
        None,
        Some(ck),
        &DeepControl::default(),
        3,
    )
    .await;
    assert_eq!(out.state, TurnState::Completed, "{:?}", out.reason);
    assert!(search2.queries().is_empty(), "no search ran again");
    assert_eq!(
        reader2.reads.load(Ordering::SeqCst),
        0,
        "no page was read again"
    );
    // Worker 2's notes and the answer; no plan, no notes for workers 1 and 3.
    assert_eq!(model2.calls(), 2);
    assert!(
        !model2
            .prompts
            .lock()
            .unwrap()
            .iter()
            .any(|p| p.contains("alpha part") && p.contains("Your sub-question")),
        "worker 1 did not write notes again"
    );
    assert!(saved.latest().is_none(), "the answer clears the checkpoint");
    let kinds: Vec<String> = sink2.events().into_iter().map(|e| e.kind).collect();
    assert!(kinds.contains(&"run.resumed".to_owned()));
    assert!(!kinds.contains(&"plan.drafted".to_owned()));
    // The resumed run's view and report show all three workers.
    let run = view(&sink2);
    let deep = run.deep.clone().unwrap();
    assert_eq!(deep.workers.len(), 3);
    assert!(deep.workers.iter().all(|w| w.status == WorkerStatus::Done));
    assert!(deep.workers.iter().all(|w| !w.pages.is_empty()));
    assert!(run.resumed.is_some());
    let past = crate::transcript::past_turns(&sink2.events());
    let report = crate::report::from_turn(session, 1, &past[0], "mock", None);
    assert!(report.resumed_from.is_some());
    let counts = report.ledger.counts();
    assert!(
        report
            .ledger
            .sources
            .iter()
            .any(|s| s.reason == LedgerReason::Restored),
        "pages kept from the checkpoint are in the ledger"
    );
    assert!(counts.read >= 8, "{counts:?}");
    assert!(report.to_markdown().contains("## Resume and steering"));
}

#[tokio::test]
async fn a_stopped_run_resumes_its_unfinished_workers() {
    let session = SessionId::new();
    let saved = crate::checkpoint::MemoryCheckpoints::default();
    let control = DeepControl::default();
    let reader = MockReader {
        delay_ms: 200,
        cancel_all_on: Some(("gamma-a", control.clone())),
        ..MockReader::default()
    };
    let sink = MemorySink::default();
    let search = CountSearch::default();
    let out = run_with(
        session,
        &sink,
        &Mock::default(),
        &search,
        &reader,
        &saved,
        None,
        None,
        &control,
        1,
    )
    .await;
    assert_eq!(out.state, TurnState::Cancelled);
    let ck = saved.latest().expect("a stopped run keeps its checkpoint");
    let done = ck.workers_done().unwrap();
    assert_eq!(done, (2, 3), "workers 1 and 2 finished before the stop");
    let run = view(&sink);
    assert!(run.resumable);

    let search2 = CountSearch::default();
    let reader2 = MockReader::default();
    let sink2 = MemorySink::default();
    let out = run_with(
        session,
        &sink2,
        &Mock::default(),
        &search2,
        &reader2,
        &saved,
        None,
        Some(ck),
        &DeepControl::default(),
        1,
    )
    .await;
    assert_eq!(out.state, TurnState::Completed, "{:?}", out.reason);
    // Only worker 3 searched again; workers 1 and 2 were kept.
    assert_eq!(search2.queries(), ["gamma"]);
    assert!(reader2.reads.load(Ordering::SeqCst) <= 3);
}

#[tokio::test]
async fn steering_drops_adds_and_guides_workers() {
    let steer = Steering::new();
    steer.send("drop sub-question 3");
    steer.send("add: delta part");
    steer.send("focus on 2024 sources");
    steer.send("drop 9");
    let saved = crate::checkpoint::MemoryCheckpoints::default();
    let model = Mock::default();
    let sink = MemorySink::default();
    let search = CountSearch::default();
    let out = run_with(
        SessionId::new(),
        &sink,
        &model,
        &search,
        &MockReader::default(),
        &saved,
        Some(&steer),
        None,
        &DeepControl::default(),
        2,
    )
    .await;
    assert_eq!(out.state, TurnState::Completed, "{:?}", out.reason);
    let events = sink.events();
    let received = events.iter().filter(|e| e.kind == "steer.received").count();
    let applied: Vec<&EventEnvelope> = events
        .iter()
        .filter(|e| e.kind == "steer.applied")
        .collect();
    assert_eq!(received, 4);
    assert_eq!(applied.len(), 4);
    assert_eq!(
        applied[3].payload["applied"], false,
        "there is no sub-question 9"
    );
    // Steering lands before any worker starts (the first safe point).
    let first_steer = events
        .iter()
        .position(|e| e.kind == "steer.applied")
        .unwrap();
    let first_worker = events
        .iter()
        .position(|e| e.kind == "worker.started")
        .unwrap();
    assert!(first_steer < first_worker);
    // Gamma never searched; delta did, as worker 4.
    let q = search.queries();
    assert!(!q.iter().any(|x| x == "gamma"), "{q:?}");
    assert!(q.iter().any(|x| x == "delta part"), "{q:?}");
    let run = view(&sink);
    let deep = run.deep.unwrap();
    assert_eq!(deep.workers.len(), 4);
    let w3 = deep.workers.iter().find(|w| w.n == 3).unwrap();
    assert_eq!(w3.status, WorkerStatus::Cancelled);
    assert_eq!(w3.note, "dropped by your steer");
    assert!(
        deep.workers
            .iter()
            .any(|w| w.n == 4 && w.status == WorkerStatus::Done)
    );
    // The guidance reached every notes call and the answer.
    let prompts = model.prompts.lock().unwrap().clone();
    let with_guidance = prompts
        .iter()
        .filter(|p| p.contains("focus on 2024 sources"))
        .count();
    assert_eq!(with_guidance, 3 + 1, "three workers' notes and the answer");
    assert_eq!(run.steers.len(), 4);
    assert!(run.steers.iter().all(|s| s.applied.is_some()));
}

#[tokio::test]
async fn a_running_worker_takes_a_redirect_at_its_next_round() {
    let steer = Steering::new();
    // Sent while worker 1 reads its pages.
    struct SteerReader {
        inner: MockReader,
        steer: Steering,
    }
    impl Reader for SteerReader {
        fn read<'a>(&'a self, url: &'a str) -> BoxFuture<'a, Result<ReadPage, String>> {
            if url.contains("alpha-a") {
                self.steer.send("redirect 1: alpha costs");
            }
            self.inner.read(url)
        }
    }
    let reader = SteerReader {
        inner: MockReader {
            delay_ms: 120,
            ..MockReader::default()
        },
        steer: steer.clone(),
    };
    let ledger = UsageLedger::default();
    let sink = MemorySink::default();
    let search = CountSearch::default();
    let model = Mock::default();
    let deps = DeepDeps {
        session: SessionId::new(),
        model: &model,
        model_name: "mock",
        search: &search,
        reader: &reader,
        sink: &sink,
        ledger: &ledger,
        gate: None,
        on_text: None,
        checkpoints: None,
        steering: Some(&steer),
        decider: None,
    };
    let out = run_deep(
        deps,
        "alpha, beta, gamma?",
        &opts(3),
        &RunAsDrafted,
        &DeepControl::default(),
    )
    .await;
    assert_eq!(out.state, TurnState::Completed, "{:?}", out.reason);
    assert!(
        search.queries().iter().any(|q| q == "alpha costs"),
        "{:?}",
        search.queries()
    );
    let applied: Vec<Value> = sink
        .events()
        .into_iter()
        .filter(|e| e.kind == "steer.applied")
        .map(|e| e.payload)
        .collect();
    assert_eq!(applied.len(), 1);
    assert_eq!(applied[0]["worker"], 1);
    assert_eq!(applied[0]["applied"], true);
    let prompts = model.prompts.lock().unwrap().clone();
    assert!(
        prompts
            .iter()
            .any(|p| p.contains("redirected this sub-question while it ran: alpha costs"))
    );
}

#[tokio::test]
async fn a_late_steer_is_logged_but_not_applied() {
    let steer = Steering::new();
    let ledger = UsageLedger::default();
    let sink = MemorySink::default();
    struct LateModel {
        inner: Mock,
        steer: Steering,
    }
    impl Model for LateModel {
        fn provider_id(&self) -> &str {
            "mock"
        }
        fn complete<'a>(
            &'a self,
            req: &'a ChatRequest,
        ) -> BoxFuture<'a, ProviderResult<ChatResponse>> {
            if req
                .system
                .as_deref()
                .is_some_and(|s| s.contains("Several workers researched"))
            {
                self.steer.send("also cover pricing");
            }
            self.inner.complete(req)
        }
    }
    let model = LateModel {
        inner: Mock::default(),
        steer: steer.clone(),
    };
    let deps = DeepDeps {
        session: SessionId::new(),
        model: &model,
        model_name: "mock",
        search: &MockSearch,
        reader: &MockReader::default(),
        sink: &sink,
        ledger: &ledger,
        gate: None,
        on_text: None,
        checkpoints: None,
        steering: Some(&steer),
        decider: None,
    };
    let out = run_deep(
        deps,
        "alpha, beta, gamma?",
        &opts(3),
        &RunAsDrafted,
        &DeepControl::default(),
    )
    .await;
    assert_eq!(out.state, TurnState::Completed);
    let e = sink
        .events()
        .into_iter()
        .find(|e| e.kind == "steer.applied")
        .expect("logged");
    assert_eq!(e.payload["applied"], false);
    assert!(e.user_summary.contains("already being written"));
}

#[tokio::test]
async fn the_ledger_lists_every_url_and_what_each_worker_used() {
    let f = Fixture::new();
    let out = f
        .run(
            &Mock::default(),
            &MockReader::default(),
            opts(3),
            &RunAsDrafted,
            &DeepControl::default(),
            None,
        )
        .await;
    assert_eq!(out.state, TurnState::Completed);
    let l = f.run_view().ledger();
    // Each search returns four results; three are read per worker.
    let c = l.counts();
    assert_eq!(c.considered, 12, "{l:#?}");
    assert_eq!(c.read, 8);
    assert!(l.sources.iter().any(|s| s.reason == LedgerReason::Robots));
    assert_eq!(c.skipped, 4, "3 not picked and 1 robots.txt");
    let lanes: Vec<Option<usize>> = l.lanes.iter().map(|x| x.worker).collect();
    assert_eq!(lanes, [None, Some(1), Some(2), Some(3)]);
    let w1 = &l.lanes[1];
    assert_eq!((w1.model_calls, w1.pages, w1.searches), (1, 3, 1));
    assert!(w1.budget.is_some());
    let run = &l.lanes[0];
    assert_eq!(run.model_calls, 2, "the plan and the answer");
}

/// A search service that is down: every query fails, except the first
/// `ok_first` (the preflight), until it is switched back on.
struct DownSearch {
    calls: AtomicUsize,
    ok_first: usize,
    down: std::sync::atomic::AtomicBool,
}

impl DownSearch {
    fn new(ok_first: usize) -> Self {
        Self {
            calls: AtomicUsize::new(0),
            ok_first,
            down: std::sync::atomic::AtomicBool::new(true),
        }
    }
}

const DOWN: &str = "SearXNG at http://localhost:8080 did not answer: Connection refused (os error 111). Check that it is running, or set its server address in Providers › Search";

impl Searcher for DownSearch {
    fn id(&self) -> &str {
        "mock-search"
    }
    fn search<'a>(
        &'a self,
        query: &'a str,
        limit: usize,
    ) -> BoxFuture<'a, Result<Vec<SearchHit>, String>> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        if n >= self.ok_first && self.down.load(Ordering::SeqCst) {
            return Box::pin(async { Err(DOWN.to_owned()) });
        }
        MockSearch.search(query, limit)
    }
}

#[tokio::test]
async fn search_that_is_down_stops_the_run_before_the_plan() {
    let session = SessionId::new();
    let saved = crate::checkpoint::MemoryCheckpoints::default();
    let model = Mock::default();
    let search = DownSearch::new(0);
    let sink = MemorySink::default();
    let out = run_with(
        session,
        &sink,
        &model,
        &search,
        &MockReader::default(),
        &saved,
        None,
        None,
        &DeepControl::default(),
        3,
    )
    .await;
    assert_eq!(out.state, TurnState::Failed);
    let why = out.reason.unwrap();
    assert!(why.starts_with("Web search isn't working"), "{why}");
    assert!(why.contains("did not answer: Connection refused"), "{why}");
    assert!(!why.contains("found nothing"), "{why}");
    assert_eq!(model.calls(), 0, "no plan call is spent");
    assert!(saved.latest().is_none(), "nothing to resume");
}

#[tokio::test]
async fn failed_workers_name_the_search_error_and_run_again_on_retry() {
    let session = SessionId::new();
    let saved = crate::checkpoint::MemoryCheckpoints::default();
    let model = Mock::default();
    // The preflight passes, then the service goes down.
    let search = DownSearch::new(1);
    let sink = MemorySink::default();
    let out = run_with(
        session,
        &sink,
        &model,
        &search,
        &MockReader::default(),
        &saved,
        None,
        None,
        &DeepControl::default(),
        3,
    )
    .await;
    assert_eq!(out.state, TurnState::Failed);
    let why = out.reason.clone().unwrap();
    assert!(
        why.starts_with("No search service answered: SearXNG at http://localhost:8080"),
        "{why}"
    );
    assert!(!why.contains("found nothing"), "{why}");
    let notes: Vec<String> = sink
        .events()
        .into_iter()
        .filter(|e| e.kind == "worker.finished")
        .map(|e| e.payload["note"].as_str().unwrap_or_default().to_owned())
        .collect();
    assert_eq!(notes.len(), 3);
    assert!(
        notes.iter().all(|n| n.starts_with(SEARCH_FAILED)),
        "{notes:?}"
    );

    let ck = saved.latest().expect("a checkpoint");
    assert_eq!(ck.progress(), "Deep research · 0 of 3 done · 3 failed");
    let offer = ck.offer(Some((out.state, out.reason.as_deref()))).unwrap();
    assert_eq!(offer.kind, crate::checkpoint::OfferKind::Retry);
    assert!(offer.about_search());

    // Search is fixed; Retry runs the failed workers again and answers.
    search.down.store(false, Ordering::SeqCst);
    let model2 = Mock::default();
    let sink2 = MemorySink::default();
    let out2 = run_with(
        session,
        &sink2,
        &model2,
        &search,
        &MockReader::default(),
        &saved,
        None,
        Some(ck),
        &DeepControl::default(),
        3,
    )
    .await;
    assert_eq!(out2.state, TurnState::Completed, "{:?}", out2.reason);
    assert!(out2.answer.is_some());
    let started = sink2
        .events()
        .into_iter()
        .filter(|e| e.kind == "worker.started")
        .count();
    assert_eq!(started, 3, "every failed worker ran again");
    assert_eq!(
        model2.calls(),
        3 + 1,
        "three notes calls and the answer, no plan"
    );
}
