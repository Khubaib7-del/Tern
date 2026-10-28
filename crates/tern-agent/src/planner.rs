//! The planner (#77): a visible plan the model keeps for a chat (`plan`),
//! and a task graph it hands Tern to run (`run_tasks`): a JSON DAG of tool
//! calls whose independent steps run in parallel, each still through the
//! policy gate, with every result fed to the steps that need it.
//!
//! Both are read leniently, because small local models get the shape
//! slightly wrong: steps as plain strings, `depends_on` instead of
//! `after`, numbers for ids, the whole graph as a JSON string or inside a
//! code fence. What can be repaired is repaired (and said); what cannot
//! (a cycle, an unknown tool) comes back to the model as one plain line
//! with an example.
//!
//! The engine runs the graph ([`crate::engine`]); this module only reads,
//! checks and orders it, and turns a run's `plan.updated` events into
//! what a front end shows ([`of_run`]).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tern_core::EffectClass;

use crate::tools::{BoxFuture, Tool, ToolError, ToolOutput};

/// Most steps a plan keeps.
pub const MAX_PLAN_STEPS: usize = 20;
/// Most tasks one graph runs.
pub const MAX_TASKS: usize = 12;
/// Tasks of one wave that run at the same time.
pub const PARALLEL: usize = 3;
/// Characters of a task's result put into a dependent's arguments.
pub const FEED_CHARS: usize = 4_000;

/// Tools a graph cannot call: itself, the plan, and code mode (a script
/// is its own graph).
pub const NOT_IN_GRAPH: &[&str] = &["run_tasks", "plan", "run_python"];

/// Where a step is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    #[default]
    Pending,
    InProgress,
    Done,
    /// A graph task that failed, was refused or declined.
    Failed,
    /// A graph task left out because a step it needs failed.
    Skipped,
}

impl StepStatus {
    /// Reads a status the way models write it.
    pub fn parse(s: &str) -> Self {
        let s = s.trim().to_ascii_lowercase().replace(['-', ' '], "_");
        match s.as_str() {
            "in_progress" | "inprogress" | "doing" | "active" | "running" | "started"
            | "current" | "working" | "now" => Self::InProgress,
            "done" | "complete" | "completed" | "finished" | "ok" | "x" | "success" | "✓" => {
                Self::Done
            }
            "failed" | "error" | "blocked" => Self::Failed,
            "skipped" | "cancelled" | "canceled" => Self::Skipped,
            _ => Self::Pending,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Pending => "Pending",
            Self::InProgress => "In progress",
            Self::Done => "Done",
            Self::Failed => "Failed",
            Self::Skipped => "Skipped",
        }
    }

    /// One character for a terminal.
    pub fn mark(self) -> &'static str {
        match self {
            Self::Pending => "○",
            Self::InProgress => "▸",
            Self::Done => "✓",
            Self::Failed => "✕",
            Self::Skipped => "–",
        }
    }
}

/// One step of a plan, or one task of a graph as the plan card shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PlanStep {
    #[serde(default)]
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub status: StepStatus,
    /// The tool a graph task calls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// The steps it waits for.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub after: Vec<String>,
    /// The call number of a graph task's tool call, once it was asked for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call: Option<u32>,
}

/// A plan or a graph, as logged with `plan.updated`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PlanView {
    /// `plan` (the model's own checklist) or `graph` (a task graph).
    pub kind: String,
    #[serde(default)]
    pub title: Option<String>,
    pub steps: Vec<PlanStep>,
    /// Waves a graph runs in, for "2 waves".
    #[serde(default)]
    pub waves: usize,
    /// What Tern repaired in what the model sent.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

impl PlanView {
    pub fn done(&self) -> usize {
        self.steps
            .iter()
            .filter(|s| s.status == StepStatus::Done)
            .count()
    }

    pub fn count(&self, st: StepStatus) -> usize {
        self.steps.iter().filter(|s| s.status == st).count()
    }

    /// "2 of 4 done".
    pub fn progress(&self) -> String {
        format!("{} of {} done", self.done(), self.steps.len())
    }

    /// The step in progress, if any.
    pub fn current(&self) -> Option<&PlanStep> {
        self.steps
            .iter()
            .find(|s| s.status == StepStatus::InProgress)
    }

    pub fn is_graph(&self) -> bool {
        self.kind == "graph"
    }

    /// One line for a terminal or a log: "Plan · 1 of 3 done: ✓ Find ·
    /// ▸ Read · ○ Write".
    pub fn line(&self) -> String {
        let head = if self.is_graph() {
            "Task graph"
        } else {
            "Plan"
        };
        let steps = self
            .steps
            .iter()
            .map(|s| format!("{} {}", s.status.mark(), s.title))
            .collect::<Vec<_>>()
            .join(" · ");
        format!("{head} · {}: {steps}", self.progress())
    }
}

/// Strips a code fence and reads a JSON string as JSON.
fn unwrap_json(v: &Value) -> Value {
    match v {
        Value::String(s) => {
            let t = s.trim();
            let t = t
                .strip_prefix("```json")
                .or_else(|| t.strip_prefix("```"))
                .unwrap_or(t);
            let t = t.strip_suffix("```").unwrap_or(t).trim();
            serde_json::from_str(t).unwrap_or_else(|_| v.clone())
        }
        other => other.clone(),
    }
}

fn str_of(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_owned()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn first<'v>(o: &'v serde_json::Map<String, Value>, keys: &[&str]) -> Option<&'v Value> {
    keys.iter().find_map(|k| o.get(*k).filter(|v| !v.is_null()))
}

/// Reads the `plan` tool's arguments: `steps` as objects or plain
/// strings, statuses in any common spelling, at most one step in
/// progress (the first one wins).
pub fn read_plan(args: &Value) -> Result<PlanView, String> {
    let args = unwrap_json(args);
    let list = match &args {
        Value::Array(_) => args.clone(),
        Value::Object(o) => unwrap_json(
            first(o, &["steps", "plan", "items", "todos", "tasks"]).unwrap_or(&Value::Null),
        ),
        _ => Value::Null,
    };
    let Some(items) = list.as_array() else {
        return Err("`steps` must be a list of steps, for example [{\"title\": \"Find sources\", \"status\": \"in_progress\"}, {\"title\": \"Write the summary\", \"status\": \"pending\"}]".into());
    };
    let mut notes = Vec::new();
    let mut steps = Vec::new();
    for (i, it) in items.iter().enumerate() {
        let (title, status) = match it {
            Value::String(s) => (s.trim().to_owned(), StepStatus::Pending),
            Value::Object(o) => {
                let title = first(
                    o,
                    &[
                        "title",
                        "step",
                        "name",
                        "text",
                        "content",
                        "task",
                        "description",
                    ],
                )
                .and_then(str_of)
                .unwrap_or_default();
                let status = match first(o, &["status", "state"]) {
                    Some(Value::String(s)) => StepStatus::parse(s),
                    Some(Value::Bool(true)) => StepStatus::Done,
                    _ if o.get("done") == Some(&Value::Bool(true)) => StepStatus::Done,
                    _ => StepStatus::Pending,
                };
                (title, status)
            }
            _ => (String::new(), StepStatus::Pending),
        };
        if title.is_empty() {
            notes.push(format!("step {} had no title and was left out", i + 1));
            continue;
        }
        let title: String = title.chars().take(160).collect();
        steps.push(PlanStep {
            id: format!("{}", steps.len() + 1),
            title,
            status,
            ..PlanStep::default()
        });
    }
    if steps.is_empty() {
        return Err("the plan has no steps; send every step with a title".into());
    }
    if steps.len() > MAX_PLAN_STEPS {
        notes.push(format!("kept the first {MAX_PLAN_STEPS} steps"));
        steps.truncate(MAX_PLAN_STEPS);
    }
    let mut seen = false;
    for s in steps.iter_mut() {
        if s.status == StepStatus::InProgress {
            if seen {
                s.status = StepStatus::Pending;
                notes.push(format!(
                    "only one step can be in progress; \"{}\" is pending",
                    s.title
                ));
            }
            seen = true;
        }
    }
    let title = args
        .as_object()
        .and_then(|o| first(o, &["title", "goal"]))
        .and_then(str_of);
    Ok(PlanView {
        kind: "plan".into(),
        title,
        steps,
        waves: 0,
        notes,
    })
}

/// One task of a graph, read and repaired.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Task {
    pub id: String,
    pub tool: String,
    pub args: Value,
    pub after: Vec<String>,
    /// A short label for the plan card.
    pub title: String,
}

/// A checked task graph: no cycles, every dependency known, every tool
/// registered.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Graph {
    pub goal: Option<String>,
    pub tasks: Vec<Task>,
    pub notes: Vec<String>,
}

/// An example graph, for the line a bad one gets back.
pub const GRAPH_EXAMPLE: &str = r#"{"tasks": [{"id": "a", "tool": "web_search", "args": {"query": "RAG survey 2024"}}, {"id": "b", "tool": "web_search", "args": {"query": "RAG evaluation benchmarks"}}, {"id": "c", "tool": "read_url", "args": {"url": "{{a}}"}, "after": ["a"]}]}"#;

fn task_title(tool: &str, args: &Value) -> String {
    let s = |k: &str| args.get(k).and_then(Value::as_str).map(str::trim);
    let what = s("query")
        .or_else(|| s("url"))
        .or_else(|| s("path"))
        .or_else(|| s("command"))
        .or_else(|| s("name"))
        .unwrap_or("");
    let t = if what.is_empty() {
        tool.to_owned()
    } else {
        format!("{tool}: {what}")
    };
    if t.chars().count() > 80 {
        format!("{}…", t.chars().take(79).collect::<String>())
    } else {
        t
    }
}

/// Reads a `run_tasks` call. `known` says whether a tool exists (the
/// turn's registry); the graph's own tools ([`NOT_IN_GRAPH`]) are refused.
pub fn read_graph(args: &Value, known: &dyn Fn(&str) -> bool) -> Result<Graph, String> {
    let args = unwrap_json(args);
    let (list, goal) = match &args {
        Value::Array(_) => (args.clone(), None),
        Value::Object(o) => (
            unwrap_json(first(o, &["tasks", "steps", "graph", "nodes"]).unwrap_or(&Value::Null)),
            first(o, &["goal", "title"]).and_then(str_of),
        ),
        _ => (Value::Null, None),
    };
    // `{"graph": {"tasks": [...]}}`
    let list = match list {
        Value::Object(ref o) => {
            unwrap_json(first(o, &["tasks", "steps", "nodes"]).unwrap_or(&Value::Null))
        }
        other => other,
    };
    let Some(items) = list.as_array() else {
        return Err(format!(
            "`tasks` must be a list of tasks. Example: {GRAPH_EXAMPLE}"
        ));
    };
    if items.is_empty() {
        return Err(format!("the graph has no tasks. Example: {GRAPH_EXAMPLE}"));
    }
    if items.len() > MAX_TASKS {
        return Err(format!(
            "the graph has {} tasks; at most {MAX_TASKS} run at once. Split it.",
            items.len()
        ));
    }
    let mut notes = Vec::new();
    let mut tasks: Vec<Task> = Vec::new();
    for (i, it) in items.iter().enumerate() {
        let Some(o) = it.as_object() else {
            return Err(format!(
                "task {} is not an object. Example: {GRAPH_EXAMPLE}",
                i + 1
            ));
        };
        let mut id = first(o, &["id", "name", "key"])
            .and_then(str_of)
            .unwrap_or_else(|| format!("t{}", i + 1));
        if tasks.iter().any(|t| t.id == id) {
            let fresh = format!("{id}_{}", i + 1);
            notes.push(format!(
                "two tasks were called \"{id}\"; the second is \"{fresh}\""
            ));
            id = fresh;
        }
        let Some(tool) = first(o, &["tool", "action", "function", "call"]).and_then(str_of) else {
            return Err(format!(
                "task \"{id}\" names no `tool`. Example: {GRAPH_EXAMPLE}"
            ));
        };
        if NOT_IN_GRAPH.contains(&tool.as_str()) {
            return Err(format!(
                "task \"{id}\" calls `{tool}`, which cannot run inside a task graph"
            ));
        }
        if !known(&tool) {
            return Err(format!(
                "task \"{id}\" calls `{tool}`, which this chat does not have"
            ));
        }
        let raw = first(o, &["args", "arguments", "input", "params", "parameters"])
            .map(unwrap_json)
            .unwrap_or_else(|| json!({}));
        let args = match raw {
            Value::Object(_) => raw,
            Value::Null => json!({}),
            other => {
                return Err(format!(
                    "task \"{id}\": `args` must be an object, not {other}"
                ));
            }
        };
        let after = match first(
            o,
            &[
                "after",
                "depends_on",
                "dependsOn",
                "deps",
                "needs",
                "requires",
            ],
        )
        .map(unwrap_json)
        {
            Some(Value::Array(a)) => a.iter().filter_map(str_of).collect(),
            Some(v) => str_of(&v).into_iter().collect(),
            None => Vec::new(),
        };
        let title = first(o, &["title", "label", "description"])
            .and_then(str_of)
            .unwrap_or_else(|| task_title(&tool, &args));
        tasks.push(Task {
            id,
            tool,
            args,
            after,
            title,
        });
    }
    // Dependencies: known ids only, no self-waits, no repeats. A number
    // that is not an id is read as a 1-based position.
    let ids: Vec<String> = tasks.iter().map(|t| t.id.clone()).collect();
    for t in tasks.iter_mut() {
        let mut kept: Vec<String> = Vec::new();
        for d in std::mem::take(&mut t.after) {
            let d = if ids.contains(&d) {
                d
            } else if let Ok(n) = d.parse::<usize>()
                && (1..=ids.len()).contains(&n)
            {
                ids[n - 1].clone()
            } else {
                notes.push(format!(
                    "\"{}\" waited for \"{d}\", which is not a task; dropped",
                    t.id
                ));
                continue;
            };
            if d == t.id {
                notes.push(format!("\"{}\" waited for itself; dropped", t.id));
                continue;
            }
            if !kept.contains(&d) {
                kept.push(d);
            }
        }
        t.after = kept;
    }
    // A `{{id}}` in the arguments is a dependency too.
    for t in tasks.iter_mut() {
        let text = t.args.to_string();
        for id in &ids {
            if *id != t.id && text.contains(&format!("{{{{{id}}}}}")) && !t.after.contains(id) {
                notes.push(format!(
                    "\"{}\" uses {{{{{id}}}}}, so it waits for \"{id}\"",
                    t.id
                ));
                t.after.push(id.clone());
            }
        }
    }
    let g = Graph { goal, tasks, notes };
    if let Some(cycle) = g.cycle() {
        return Err(format!(
            "the tasks wait for each other in a circle ({}); a task can only wait for tasks that do not wait for it",
            cycle.join(" → ")
        ));
    }
    Ok(g)
}

impl Graph {
    /// A cycle, if the graph has one, as the ids along it.
    fn cycle(&self) -> Option<Vec<String>> {
        let deps: BTreeMap<&str, &[String]> = self
            .tasks
            .iter()
            .map(|t| (t.id.as_str(), t.after.as_slice()))
            .collect();
        // 0 = unseen, 1 = on the stack, 2 = finished.
        let mut mark: BTreeMap<&str, u8> = BTreeMap::new();
        fn visit<'g>(
            n: &'g str,
            deps: &BTreeMap<&'g str, &'g [String]>,
            mark: &mut BTreeMap<&'g str, u8>,
            stack: &mut Vec<&'g str>,
        ) -> Option<Vec<String>> {
            match mark.get(n).copied() {
                Some(2) => return None,
                Some(1) => {
                    let at = stack.iter().position(|s| *s == n).unwrap_or(0);
                    let mut c: Vec<String> = stack[at..].iter().map(|s| s.to_string()).collect();
                    c.push(n.to_owned());
                    return Some(c);
                }
                _ => {}
            }
            mark.insert(n, 1);
            stack.push(n);
            for d in deps.get(n).copied().unwrap_or(&[]) {
                if let Some(c) = visit(d.as_str(), deps, mark, stack) {
                    return Some(c);
                }
            }
            stack.pop();
            mark.insert(n, 2);
            None
        }
        for t in &self.tasks {
            let mut stack = Vec::new();
            if let Some(c) = visit(t.id.as_str(), &deps, &mut mark, &mut stack) {
                return Some(c);
            }
        }
        None
    }

    /// The tasks in waves: each wave's tasks need only earlier waves, so
    /// they can run at the same time. Indices into `tasks`, in order.
    pub fn waves(&self) -> Vec<Vec<usize>> {
        let mut done: BTreeSet<&str> = BTreeSet::new();
        let mut left: Vec<usize> = (0..self.tasks.len()).collect();
        let mut out = Vec::new();
        while !left.is_empty() {
            let (ready, rest): (Vec<usize>, Vec<usize>) = left.iter().partition(|&&i| {
                self.tasks[i]
                    .after
                    .iter()
                    .all(|d| done.contains(d.as_str()))
            });
            if ready.is_empty() {
                // Only a cycle leaves nothing ready; read_graph refuses those.
                out.push(rest);
                break;
            }
            for &i in &ready {
                done.insert(self.tasks[i].id.as_str());
            }
            out.push(ready);
            left = rest;
        }
        out
    }

    /// The graph as the plan card shows it, every task pending.
    pub fn view(&self) -> PlanView {
        PlanView {
            kind: "graph".into(),
            title: self.goal.clone(),
            steps: self
                .tasks
                .iter()
                .map(|t| PlanStep {
                    id: t.id.clone(),
                    title: t.title.clone(),
                    status: StepStatus::Pending,
                    tool: Some(t.tool.clone()),
                    after: t.after.clone(),
                    call: None,
                })
                .collect(),
            waves: self.waves().len(),
            notes: self.notes.clone(),
        }
    }
}

/// Puts the results of finished tasks into a task's arguments: every
/// `{{id}}` in a string becomes that task's result (cut to
/// [`FEED_CHARS`]). A string that is only `{{id}}` and whose result is a
/// web address list becomes the first address, so `read_url` can follow
/// a search.
pub fn feed(args: &Value, results: &BTreeMap<String, String>) -> Value {
    match args {
        Value::String(s) => {
            let t = s.trim();
            if let Some(id) = t.strip_prefix("{{").and_then(|r| r.strip_suffix("}}"))
                && let Some(r) = results.get(id.trim())
                && s.contains("{{")
            {
                // A lone placeholder in a url field: the first link found.
                if let Some(url) = first_url(r)
                    && !r.trim().starts_with("http")
                {
                    return Value::String(url);
                }
                return Value::String(cut(r));
            }
            let mut out = s.clone();
            for (id, r) in results {
                let key = format!("{{{{{id}}}}}");
                if out.contains(&key) {
                    out = out.replace(&key, &cut(r));
                }
            }
            Value::String(out)
        }
        Value::Array(a) => Value::Array(a.iter().map(|v| feed(v, results)).collect()),
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, v)| (k.clone(), feed(v, results)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn cut(s: &str) -> String {
    if s.chars().count() <= FEED_CHARS {
        s.to_owned()
    } else {
        format!("{}…", s.chars().take(FEED_CHARS).collect::<String>())
    }
}

/// The first `http(s)://` address in a text.
pub fn first_url(s: &str) -> Option<String> {
    let i = s.find("https://").or_else(|| s.find("http://"))?;
    let rest = &s[i..];
    let end = rest
        .find(|c: char| c.is_whitespace() || matches!(c, ')' | ']' | '>' | '"' | '\'' | '<'))
        .unwrap_or(rest.len());
    let url = rest[..end].trim_end_matches(['.', ',', ';']);
    (url.len() > 8).then(|| url.to_owned())
}

/// Whether a tool can run alongside others in a wave: reads, searches
/// and pure work. Changes run one at a time, after them.
pub fn parallel_safe(effect: EffectClass) -> bool {
    matches!(
        effect,
        EffectClass::Read | EffectClass::Network | EffectClass::Analysis
    )
}

/// The latest plan and the latest task graph of a run, from its
/// `plan.updated` events.
pub fn of_run(run: &crate::Run) -> (Option<PlanView>, Option<PlanView>) {
    let mut plan = None;
    let mut graph = None;
    for s in run.steps.iter().filter(|s| s.event == "plan.updated") {
        if let Ok(v) = serde_json::from_value::<PlanView>(s.payload.clone()) {
            if v.is_graph() {
                graph = Some(v);
            } else {
                plan = Some(v);
            }
        }
    }
    // A run that stopped leaves no task running.
    if run.end.is_some()
        && let Some(g) = graph.as_mut()
    {
        for s in g.steps.iter_mut() {
            if matches!(s.status, StepStatus::InProgress | StepStatus::Pending) {
                s.status = StepStatus::Skipped;
            }
        }
    }
    (plan, graph)
}

/// The `plan` tool: the model's visible checklist for a chat. Tern logs
/// each update (`plan.updated`) and the window shows it as a card that
/// changes as the steps do.
#[derive(Debug, Clone, Copy)]
pub struct PlanTool;

impl Tool for PlanTool {
    fn name(&self) -> &'static str {
        "plan"
    }
    fn summary(&self) -> &'static str {
        "Show the user a short plan for a task with several steps, and update it as you go: send the whole list each time, each step with a title and a status (pending, in_progress or done), one step in progress. Skip it for simple questions."
    }
    fn effect(&self) -> EffectClass {
        EffectClass::Analysis
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object", "required": ["steps"], "properties": {
            "title": {"type": "string"},
            "steps": {"type": "array", "items": {"type": "object", "required": ["title", "status"], "properties": {
                "title": {"type": "string"},
                "status": {"type": "string", "enum": ["pending", "in_progress", "done"]}
            }}}
        }})
    }
    fn call<'a>(&'a self, args: Value) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let plan = read_plan(&args).map_err(ToolError::BadInput)?;
            let mut text = format!("Plan updated ({}).", plan.progress());
            if let Some(c) = plan.current() {
                text.push_str(&format!(" Now: {}.", c.title));
            }
            if !plan.notes.is_empty() {
                text.push_str(&format!(" Tern fixed: {}.", plan.notes.join("; ")));
            }
            text.push_str(" Carry on with the work; do not repeat the plan in your answer.");
            Ok(ToolOutput {
                content: text,
                evidence_refs: Vec::new(),
                meta: json!({"plan": plan}),
            })
        })
    }
}

/// The `run_tasks` tool: a task graph. Its calls run inside the turn (the
/// engine reads the graph and runs each task through the gate), so
/// [`Tool::call`] only answers when something calls it outside a turn.
#[derive(Debug, Clone, Copy)]
pub struct RunTasksTool;

impl Tool for RunTasksTool {
    fn name(&self) -> &'static str {
        "run_tasks"
    }
    fn summary(&self) -> &'static str {
        "Run several tool calls as a task graph: tasks that do not wait for each other run in parallel, and a task's \"after\" lists the ids it needs; write {{id}} in its args to get that task's result. JSON: {\"tasks\": [{\"id\": \"a\", \"tool\": \"web_search\", \"args\": {...}}, {\"id\": \"b\", \"tool\": \"read_url\", \"args\": {\"url\": \"{{a}}\"}, \"after\": [\"a\"]}]}. At most 12 tasks."
    }
    fn effect(&self) -> EffectClass {
        EffectClass::Analysis
    }
    fn input_schema(&self) -> Value {
        // Lenient on purpose: the graph is repaired before it is refused.
        json!({"type": "object", "properties": {
            "goal": {"type": "string"},
            "tasks": {"items": {"type": "object", "properties": {
                "id": {"type": "string"},
                "tool": {"type": "string"},
                "args": {"type": "object"},
                "after": {"type": "array", "items": {"type": "string"}}
            }}}
        }})
    }
    fn call<'a>(&'a self, _args: Value) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async {
            Err(ToolError::Failed(
                "a task graph runs inside a chat turn".into(),
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known(t: &str) -> bool {
        matches!(t, "web_search" | "read_url" | "file_read" | "file_write")
    }

    #[test]
    fn plans_are_read_leniently() {
        let p = read_plan(&json!({"steps": [
            {"title": "Find sources", "status": "done"},
            {"step": "Read two papers", "status": "In-Progress"},
            "Write the summary",
            {"title": "Check", "status": "doing"},
            {"status": "pending"}
        ]}))
        .unwrap();
        assert_eq!(p.steps.len(), 4);
        assert_eq!(p.steps[0].status, StepStatus::Done);
        assert_eq!(p.steps[1].status, StepStatus::InProgress);
        assert_eq!(p.steps[2].status, StepStatus::Pending);
        // Only one step is in progress.
        assert_eq!(p.steps[3].status, StepStatus::Pending);
        assert_eq!(p.progress(), "1 of 4 done");
        assert_eq!(p.current().unwrap().title, "Read two papers");
        assert!(
            p.line()
                .starts_with("Plan · 1 of 4 done: ✓ Find sources · ▸ Read two papers")
        );
        assert_eq!(p.notes.len(), 2);
        // A JSON string inside a fence, and a bare list.
        let s = read_plan(&json!({"steps": "```json\n[\"a\", \"b\"]\n```"})).unwrap();
        assert_eq!(s.steps.len(), 2);
        assert!(read_plan(&json!(["x"])).is_ok());
        assert!(read_plan(&json!({"steps": []})).is_err());
        assert!(read_plan(&json!({"steps": 4})).is_err());
    }

    #[test]
    fn graphs_are_repaired_and_ordered_in_waves() {
        let g = read_graph(
            &json!({"tasks": [
                {"id": 1, "tool": "web_search", "arguments": {"query": "a"}},
                {"id": "b", "tool": "web_search", "args": "{\"query\": \"b\"}"},
                {"id": "c", "tool": "read_url", "args": {"url": "{{1}}"}},
                {"name": "d", "action": "file_write", "args": {"path": "x.md", "text": "{{b}} {{c}}"}, "depends_on": "c"},
                {"id": "e", "tool": "file_read", "args": {"path": "y"}, "needs": ["zz", "e"]}
            ]}),
            &known,
        )
        .unwrap();
        assert_eq!(g.tasks[0].id, "1");
        assert_eq!(g.tasks[1].args, json!({"query": "b"}));
        assert_eq!(g.tasks[2].after, vec!["1"]);
        assert_eq!(g.tasks[3].after, vec!["c", "b"]);
        assert!(g.tasks[4].after.is_empty());
        assert!(g.notes.iter().any(|n| n.contains("zz")));
        let w = g.waves();
        assert_eq!(w, vec![vec![0, 1, 4], vec![2], vec![3]]);
        let v = g.view();
        assert_eq!(v.waves, 3);
        assert_eq!(v.steps[2].title, "read_url: {{1}}");
        // The whole graph as a string in a fence.
        let s = format!("```json\n{}\n```", GRAPH_EXAMPLE);
        let g = read_graph(
            &json!({"tasks": serde_json::from_str::<Value>(GRAPH_EXAMPLE).unwrap()["tasks"]}),
            &known,
        )
        .unwrap();
        assert_eq!(g.waves().len(), 2);
        assert!(read_graph(&json!(s), &known).is_ok());
    }

    #[test]
    fn bad_graphs_say_what_is_wrong() {
        let cyc = read_graph(
            &json!({"tasks": [
                {"id": "a", "tool": "web_search", "args": {"query": "x"}, "after": ["b"]},
                {"id": "b", "tool": "web_search", "args": {"query": "y"}, "after": ["a"]}
            ]}),
            &known,
        )
        .unwrap_err();
        assert!(cyc.contains("circle"), "{cyc}");
        assert!(
            read_graph(&json!({"tasks": [{"id": "a", "tool": "rm_rf"}]}), &known)
                .unwrap_err()
                .contains("does not have")
        );
        assert!(
            read_graph(
                &json!({"tasks": [{"id": "a", "tool": "run_tasks"}]}),
                &known
            )
            .unwrap_err()
            .contains("cannot run inside")
        );
        assert!(
            read_graph(&json!({"tasks": [{"id": "a"}]}), &known)
                .unwrap_err()
                .contains("Example")
        );
        let many: Vec<Value> = (0..13)
            .map(|i| json!({"id": format!("t{i}"), "tool": "web_search", "args": {"query": "q"}}))
            .collect();
        assert!(read_graph(&json!({"tasks": many}), &known).is_err());
        // Duplicate ids are renamed, not refused.
        let g = read_graph(
            &json!({"tasks": [{"id": "a", "tool": "web_search"}, {"id": "a", "tool": "web_search"}]}),
            &known,
        )
        .unwrap();
        assert_eq!(g.tasks[1].id, "a_2");
    }

    #[test]
    fn results_feed_dependents() {
        let mut r = BTreeMap::new();
        r.insert(
            "a".to_owned(),
            "[1] FLARE paper — https://arxiv.org/abs/2305.06983 (2023)\n[2] other".to_owned(),
        );
        r.insert("b".to_owned(), "short".to_owned());
        let fed = feed(
            &json!({"url": "{{a}}", "text": "B said {{b}}.", "n": 3, "list": ["{{b}}"]}),
            &r,
        );
        assert_eq!(fed["url"], "https://arxiv.org/abs/2305.06983");
        assert_eq!(fed["text"], "B said short.");
        assert_eq!(fed["n"], 3);
        assert_eq!(fed["list"][0], "short");
        assert_eq!(
            first_url("see (https://x.org/a)."),
            Some("https://x.org/a".into())
        );
        assert!(parallel_safe(EffectClass::Network));
        assert!(!parallel_safe(EffectClass::Write));
    }

    #[test]
    fn a_run_gives_its_latest_plan_and_graph() {
        use tern_core::{EventEnvelope, SessionId};
        let s = SessionId::new();
        let mut run = crate::Run::new();
        let p1 = read_plan(&json!({"steps": ["a", "b"]})).unwrap();
        let mut p2 = p1.clone();
        p2.steps[0].status = StepStatus::Done;
        for p in [&p1, &p2] {
            run.push(&EventEnvelope::new(
                s,
                "plan.updated",
                EffectClass::Analysis,
                p.line(),
                serde_json::to_value(p).unwrap(),
            ));
        }
        let g = read_graph(
            &json!({"tasks": [{"id": "a", "tool": "web_search"}]}),
            &known,
        )
        .unwrap();
        run.push(&EventEnvelope::new(
            s,
            "plan.updated",
            EffectClass::Analysis,
            "graph",
            serde_json::to_value(g.view()).unwrap(),
        ));
        let (plan, graph) = of_run(&run);
        assert_eq!(plan.unwrap().done(), 1);
        assert_eq!(graph.unwrap().steps.len(), 1);
    }

    #[tokio::test]
    async fn the_plan_tool_answers_with_its_plan_in_meta() {
        let out = PlanTool
            .call(json!({"steps": [{"title": "Find", "status": "in_progress"}, {"title": "Write", "status": "pending"}]}))
            .await
            .unwrap();
        assert!(out.content.contains("0 of 2 done"));
        assert!(out.content.contains("Now: Find."));
        assert_eq!(out.meta["plan"]["steps"][0]["status"], "in_progress");
        assert!(PlanTool.call(json!({"steps": 3})).await.is_err());
    }
}
