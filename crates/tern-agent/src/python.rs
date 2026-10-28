//! `run_python` (#75): Python glue code on Monty (MIT), a Python-subset
//! interpreter written in Rust. Tier 0 of the sandbox plan: no files, no
//! network, no environment and no imports beyond Monty's own standard
//! modules, with time and recursion limits. For reshaping a table,
//! BibTeX and citation clean-up, sums and conversions in a chat.
//!
//! The code runs in a worker: Tern's own executable started again with
//! `--tern-python-worker <file>`, under the workbench sandbox (a Job
//! Object on Windows, a process group with rlimits elsewhere, 256 MB, a
//! scrubbed environment), so a crash or a runaway allocation ends the
//! worker, not Tern. Its prints stream to the Terminal tab.
//!
//! Code mode (#77): the chat's tools are Python functions inside the
//! program (`web_search(query: str) -> str`), with typed stubs listed in
//! the system prompt ([`stub_text`]). One script can search, read and
//! compare many things, and only what it prints comes back to the model,
//! so the intermediate results never fill the context. Every call the
//! script makes is handed to the turn ([`Script`]), which runs it through
//! the policy gate like any other call: a write still shows its approval
//! card. The worker asks on its stdout and reads the answer on its stdin.

use std::path::Path;
use std::time::Duration;

use monty::{MontyRun, RunProgress};
use monty_types::{
    CallArgs, CompileOptions, ExcType, ExtFunctionResult, MontyException, MontyObject,
    NameLookupResult, ObjectRef, PrintWriter, ResourceLimits, ResourceTracker,
};
use serde_json::{Value, json};
use tern_core::EffectClass;

use crate::tools::{BoxFuture, Tool, ToolError, ToolOutput};

/// The flag that turns Tern's executable into a Python worker.
pub const WORKER_FLAG: &str = "--tern-python-worker";
/// Marks the worker's result line on stdout.
const RESULT_MARK: &str = "\u{1e}TERN-PYTHON-RESULT ";
/// Marks a code-mode tool call on the worker's stdout (#77).
pub const CALL_MARK: &str = "\u{1e}TERN-PYTHON-CALL ";
/// The worker flag naming the file of tool stubs (code mode).
const TOOLS_FLAG: &str = "--tools";
/// Most tool calls one script makes.
pub const MAX_SCRIPT_CALLS: usize = 40;
/// How long a code-mode worker may live, approvals included (the
/// interpreter's own time excludes the waits on Tern).
const SCRIPT_WALL: Duration = Duration::from_secs(30 * 60);
/// Longest program accepted, in bytes.
const MAX_CODE: usize = 200_000;
/// Time the interpreter may run.
const RUN_TIME: Duration = Duration::from_secs(20);

/// What a snippet gave back.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Snippet {
    /// The value of the last expression (`repr`), `None` when it was None.
    pub value: Option<String>,
    /// What `print` wrote (in-process runs only; a worker streams it).
    pub printed: String,
    /// The error and its traceback, when it failed.
    pub error: Option<String>,
}

/// Runs `code` on Monty in this process with the time and recursion
/// limits. `print` collects (in-process) or streams (worker) prints.
pub fn run_snippet(code: &str, limit: Duration, stream: bool) -> Snippet {
    let mut printed = String::new();
    let runner = MontyRun::new(
        code.to_owned(),
        "glue.py",
        Vec::new(),
        CompileOptions::default(),
    );
    let mut runner = match runner {
        Ok(r) => r,
        Err(e) => {
            return Snippet {
                value: None,
                printed,
                error: Some(e.to_string()),
            };
        }
    };
    let limits = ResourceLimits {
        max_feed_duration: Some(limit),
        max_recursion_depth: 200,
        ..ResourceLimits::default()
    };
    let writer = if stream {
        PrintWriter::Stdout
    } else {
        PrintWriter::CollectString(&mut printed, Some(1024 * 1024))
    };
    let result = runner.run(Vec::new(), ResourceTracker::new(limits), writer);
    match result {
        Ok(v) => {
            let s = v.to_string();
            Snippet {
                value: (s != "None").then_some(s),
                printed,
                error: None,
            }
        }
        Err(e) => Snippet {
            value: None,
            printed,
            error: Some(e.to_string()),
        },
    }
}

/// The worker's entry: call first thing in `main`. When the process was
/// started as a Python worker it runs the file and returns the exit code;
/// otherwise `None` and the app starts as usual. With `--tools <file>` it
/// runs in code mode: the stubs' calls go out on stdout and their answers
/// come back on stdin.
pub fn worker_main() -> Option<i32> {
    let mut args = std::env::args_os().skip(1);
    if args.next()? != WORKER_FLAG {
        return None;
    }
    let path = args.next()?;
    let code = match std::fs::read_to_string(Path::new(&path)) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("could not read the program: {e}");
            return Some(2);
        }
    };
    let stubs: Option<Vec<Stub>> = match (args.next(), args.next()) {
        (Some(flag), Some(file)) if flag == TOOLS_FLAG => std::fs::read_to_string(Path::new(&file))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok()),
        _ => None,
    };
    let s = match stubs {
        Some(stubs) => {
            use std::io::{BufRead as _, Write as _};
            let stdin = std::io::stdin();
            let mut host = move |tool: &str, args: Value| -> Result<String, String> {
                let line = json!({"tool": tool, "args": args});
                {
                    let mut out = std::io::stdout().lock();
                    let _ = writeln!(out, "{CALL_MARK}{line}");
                    let _ = out.flush();
                }
                let mut answer = String::new();
                match stdin.lock().read_line(&mut answer) {
                    Ok(0) | Err(_) => Err("Tern stopped the script".into()),
                    Ok(_) => read_answer(&answer),
                }
            };
            run_script(&code, RUN_TIME, &stubs, true, &mut host)
        }
        None => run_snippet(&code, RUN_TIME, true),
    };
    if let Some(e) = &s.error {
        eprintln!("{e}");
    }
    println!(
        "{RESULT_MARK}{}",
        serde_json::to_string(&s).unwrap_or_default()
    );
    Some(if s.error.is_some() { 1 } else { 0 })
}

/// An answer line from Tern: `{"ok": "..."}` or `{"err": "..."}`.
fn read_answer(line: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(line.trim()).map_err(|e| e.to_string())?;
    match (v["ok"].as_str(), v["err"].as_str()) {
        (Some(ok), _) => Ok(ok.to_owned()),
        (_, Some(err)) => Err(err.to_owned()),
        _ => Err("Tern sent an answer the script could not read".into()),
    }
}

/// The answer line for a call's result.
pub fn answer_line(r: &Result<String, String>) -> String {
    match r {
        Ok(s) => json!({"ok": s}).to_string(),
        Err(e) => json!({"err": e}).to_string(),
    }
}

// ----- Code mode (#77) -----

/// One parameter of a tool stub.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Param {
    pub name: String,
    /// The Python type it is shown with (`str`, `int`, `list`...).
    pub ty: String,
    pub required: bool,
}

/// A tool as a Python function a script can call.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Stub {
    pub name: String,
    /// Required parameters first, then the rest, as the schema lists them.
    pub params: Vec<Param>,
    /// The tool's summary, its first sentence.
    pub summary: String,
}

/// Tools a script cannot call: code mode itself, the graph, the plan,
/// and asking the window to switch tabs.
const NOT_IN_SCRIPTS: &[&str] = &["run_python", "run_tasks", "plan", "show_in_panel"];

fn is_identifier(s: &str) -> bool {
    let mut c = s.chars();
    c.next()
        .is_some_and(|f| f.is_ascii_alphabetic() || f == '_')
        && c.all(|x| x.is_ascii_alphanumeric() || x == '_')
}

fn py_type(schema: &Value) -> &'static str {
    match schema.get("type").and_then(Value::as_str) {
        Some("string") => "str",
        Some("integer") => "int",
        Some("number") => "float",
        Some("boolean") => "bool",
        Some("array") => "list",
        Some("object") => "dict",
        _ => "object",
    }
}

/// The stubs for the tools a script may call in this turn.
pub fn stubs(tools: &crate::tools::ToolRegistry) -> Vec<Stub> {
    tools
        .names()
        .filter(|n| !NOT_IN_SCRIPTS.contains(n) && is_identifier(n))
        .filter_map(|n| tools.get(n))
        .map(|t| {
            let schema = t.input_schema();
            let required: Vec<&str> = schema
                .get("required")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            let mut params: Vec<Param> = Vec::new();
            if let Some(props) = schema.get("properties").and_then(Value::as_object) {
                for r in &required {
                    if let Some(p) = props.get(*r)
                        && is_identifier(r)
                    {
                        params.push(Param {
                            name: (*r).to_owned(),
                            ty: py_type(p).to_owned(),
                            required: true,
                        });
                    }
                }
                for (k, p) in props {
                    if !required.contains(&k.as_str()) && is_identifier(k) {
                        params.push(Param {
                            name: k.clone(),
                            ty: py_type(p).to_owned(),
                            required: false,
                        });
                    }
                }
            }
            let summary = t.summary();
            let first = summary
                .split_inclusive(". ")
                .next()
                .unwrap_or(summary)
                .trim()
                .trim_end_matches('.');
            let summary = if first.chars().count() > 90 {
                format!("{}…", first.chars().take(89).collect::<String>())
            } else {
                first.to_owned()
            };
            Stub {
                name: t.name().to_owned(),
                params,
                summary,
            }
        })
        .collect()
}

impl Stub {
    /// `def web_search(query: str, limit: int = None) -> str:  # Search the web`
    pub fn line(&self) -> String {
        let params = self
            .params
            .iter()
            .map(|p| {
                if p.required {
                    format!("{}: {}", p.name, p.ty)
                } else {
                    format!("{}: {} = None", p.name, p.ty)
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!("def {}({params}) -> str:  # {}", self.name, self.summary)
    }

    /// A call's arguments as the tool takes them: positional ones by the
    /// stub's order, keywords by name; `None` leaves a parameter out.
    pub fn args_of(&self, call: &CallArgs) -> Value {
        let mut m = serde_json::Map::new();
        for (i, a) in call.args().enumerate() {
            let v = to_json(&a);
            match self.params.get(i) {
                Some(p) if !v.is_null() => {
                    m.insert(p.name.clone(), v);
                }
                Some(_) => {}
                None => {
                    m.insert(format!("arg{}", i + 1), v);
                }
            }
        }
        for (k, v) in call.kwargs() {
            let v = to_json(&v);
            let k = k.as_str().map(str::to_owned).unwrap_or_else(|| k.py_repr());
            if !v.is_null() {
                m.insert(k, v);
            }
        }
        Value::Object(m)
    }
}

/// The stubs as the system prompt lists them.
pub fn stub_text(stubs: &[Stub]) -> String {
    stubs.iter().map(Stub::line).collect::<Vec<_>>().join("\n")
}

/// Whether a program calls any of the stubs (`name(` outside a longer
/// name), so it runs in code mode.
pub fn calls_tools(code: &str, stubs: &[Stub]) -> bool {
    stubs.iter().any(|s| {
        let pat = format!("{}(", s.name);
        code.match_indices(&pat).any(|(i, _)| {
            !code[..i]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
        })
    })
}

/// A Python value as JSON for a tool's arguments.
fn to_json(o: &ObjectRef<'_>) -> Value {
    if let Some(b) = o.as_bool() {
        return Value::Bool(b);
    }
    if let Some(i) = o.as_int() {
        return json!(i);
    }
    if let Some(s) = o.as_str() {
        return Value::String(s.to_owned());
    }
    if o.type_name() == "float"
        && let Some(f) = o.as_float()
    {
        return json!(f);
    }
    if let Some(items) = o.items() {
        return Value::Array(items.iter().map(to_json).collect());
    }
    if let Some(pairs) = o.pairs() {
        let mut m = serde_json::Map::new();
        for (k, v) in pairs {
            let k = k.as_str().map(str::to_owned).unwrap_or_else(|| k.py_repr());
            m.insert(k, to_json(&v));
        }
        return Value::Object(m);
    }
    let r = o.py_repr();
    if r == "None" {
        Value::Null
    } else {
        Value::String(r)
    }
}

/// Runs a code-mode program: names of the stubs resolve to functions,
/// and each call goes to `host`, whose answer (the tool's text, or an
/// error raised in the script) the program gets back.
pub fn run_script(
    code: &str,
    limit: Duration,
    stubs: &[Stub],
    stream: bool,
    host: &mut dyn FnMut(&str, Value) -> Result<String, String>,
) -> Snippet {
    let mut printed = String::new();
    let runner = match MontyRun::new(
        code.to_owned(),
        "glue.py",
        Vec::new(),
        CompileOptions::default(),
    ) {
        Ok(r) => r,
        Err(e) => {
            return Snippet {
                value: None,
                printed,
                error: Some(e.to_string()),
            };
        }
    };
    let limits = ResourceLimits {
        max_feed_duration: Some(limit),
        max_recursion_depth: 200,
        ..ResourceLimits::default()
    };
    macro_rules! out {
        () => {
            if stream {
                PrintWriter::Stdout
            } else {
                PrintWriter::CollectString(&mut printed, Some(1024 * 1024))
            }
        };
    }
    let mut progress = runner.start(Vec::new(), ResourceTracker::new(limits), out!());
    let mut calls = 0usize;
    loop {
        let step = match progress {
            Ok(p) => p,
            Err(e) => {
                return Snippet {
                    value: None,
                    printed,
                    error: Some(e.to_string()),
                };
            }
        };
        progress = match step {
            RunProgress::Complete(v) => {
                let s = v.to_string();
                return Snippet {
                    value: (s != "None").then_some(s),
                    printed,
                    error: None,
                };
            }
            RunProgress::NameLookup(l) => {
                if stubs.iter().any(|s| s.name == l.name) {
                    let f = MontyObject::function(l.name.clone(), None);
                    l.resume(f, out!())
                } else {
                    l.resume(NameLookupResult::Undefined, out!())
                }
            }
            RunProgress::FunctionCall(c) => {
                let result: ExtFunctionResult = match stubs
                    .iter()
                    .find(|s| s.name == c.function_name)
                {
                    None => ExtFunctionResult::NotFound(c.function_name.clone()),
                    Some(_) if calls >= MAX_SCRIPT_CALLS => MontyException::new(
                        ExcType::RuntimeError,
                        Some(format!(
                            "a script can call tools at most {MAX_SCRIPT_CALLS} times"
                        )),
                    )
                    .into(),
                    Some(stub) => {
                        calls += 1;
                        let args = stub.args_of(&c.args);
                        match host(&stub.name, args) {
                            Ok(text) => MontyObject::string(text).into(),
                            Err(e) => MontyException::new(ExcType::RuntimeError, Some(e)).into(),
                        }
                    }
                };
                c.resume(result, out!())
            }
            RunProgress::OsCall(o) => {
                let e = o.function_call.on_no_handler();
                o.resume(e, out!())
            }
            RunProgress::ResolveFutures(_) => {
                return Snippet {
                    value: None,
                    printed,
                    error: Some("async code is not supported in run_python".into()),
                };
            }
        };
    }
}

/// What a running script did next.
#[derive(Debug)]
pub enum ScriptEvent {
    /// It called a tool; answer with [`Script::answer`].
    Call { tool: String, args: Value },
    /// It ended: its output as the tool's result.
    Done(Result<ToolOutput, ToolError>),
}

/// A code-mode program running in a worker (or in-process under tests).
#[derive(Debug)]
pub struct Script {
    calls: tokio::sync::mpsc::UnboundedReceiver<String>,
    answers: Option<tokio::sync::mpsc::UnboundedSender<String>>,
    done: Option<tokio::task::JoinHandle<Result<(String, Option<Snippet>), ToolError>>>,
}

impl Script {
    /// Starts `code` with these stubs.
    pub fn start(code: String, stubs: Vec<Stub>) -> Self {
        let (call_tx, calls) = tokio::sync::mpsc::unbounded_channel::<String>();
        let (answers, mut answer_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let done = match worker_exe() {
            Some(exe) => tokio::spawn(async move {
                run_script_in_worker(&exe, &code, &stubs, call_tx, answer_rx).await
            }),
            None => tokio::task::spawn_blocking(move || {
                let mut host = |tool: &str, args: Value| -> Result<String, String> {
                    let line = json!({"tool": tool, "args": args}).to_string();
                    call_tx
                        .send(line)
                        .map_err(|_| "Tern stopped the script".to_owned())?;
                    match answer_rx.blocking_recv() {
                        Some(a) => read_answer(&a),
                        None => Err("Tern stopped the script".into()),
                    }
                };
                let s = run_script(&code, RUN_TIME, &stubs, false, &mut host);
                Ok((s.printed.clone(), Some(s)))
            }),
        };
        Self {
            calls,
            answers: Some(answers),
            done: Some(done),
        }
    }

    /// Waits for the script's next call or its end.
    pub async fn next(&mut self) -> ScriptEvent {
        let Some(done) = self.done.as_mut() else {
            return ScriptEvent::Done(Err(ToolError::Failed("the script already ended".into())));
        };
        loop {
            tokio::select! {
                line = self.calls.recv() => match line {
                    Some(line) => {
                        let v: Value = serde_json::from_str(&line).unwrap_or_default();
                        let tool = v["tool"].as_str().unwrap_or_default().to_owned();
                        if tool.is_empty() {
                            continue;
                        }
                        return ScriptEvent::Call { tool, args: v["args"].clone() };
                    }
                    // The worker closed its side; only its end is left.
                    None => {
                        let r = done.await;
                        self.done = None;
                        return ScriptEvent::Done(finish(r));
                    }
                },
                r = &mut *done => {
                    self.done = None;
                    return ScriptEvent::Done(finish(r));
                }
            }
        }
    }

    /// Answers the call the script is waiting on.
    pub fn answer(&self, r: Result<String, String>) {
        if let Some(a) = &self.answers {
            let _ = a.send(answer_line(&r));
        }
    }

    /// Stops answering: the script's next call fails and it ends.
    pub fn stop(&mut self) {
        self.answers = None;
    }
}

fn finish(
    r: Result<Result<(String, Option<Snippet>), ToolError>, tokio::task::JoinError>,
) -> Result<ToolOutput, ToolError> {
    let (printed, snippet) = r.map_err(|e| ToolError::Failed(e.to_string()))??;
    output_of(printed, snippet, true)
}

/// A program's prints and result as the tool's output.
fn output_of(
    printed: String,
    snippet: Option<Snippet>,
    code_mode: bool,
) -> Result<ToolOutput, ToolError> {
    let Some(s) = snippet else {
        return Err(ToolError::Failed(format!(
            "the Python worker ended without a result{}",
            if printed.trim().is_empty() {
                String::new()
            } else {
                format!(":\n{}", printed.trim_end())
            }
        )));
    };
    let mut text = String::new();
    if !printed.trim().is_empty() {
        text.push_str(&format!("printed:\n{}\n", printed.trim_end()));
    }
    if let Some(v) = &s.value {
        text.push_str(&format!("value: {v}\n"));
    }
    if let Some(e) = &s.error {
        crate::diag::note(&format!(
            "python: error {}",
            e.lines().last().unwrap_or_default()
        ));
        return Err(ToolError::Failed(format!("{text}error:\n{e}")));
    }
    if text.is_empty() {
        text.push_str(if code_mode {
            "Ran with no output. Only what the script prints (or its last expression) comes back: print a short summary of what the tools returned."
        } else {
            "Ran with no output."
        });
    }
    Ok(ToolOutput {
        content: text.trim_end().to_owned(),
        evidence_refs: Vec::new(),
        meta: json!({"engine": "monty", "code_mode": code_mode}),
    })
}

async fn run_script_in_worker(
    exe: &Path,
    code: &str,
    stubs: &[Stub],
    calls: tokio::sync::mpsc::UnboundedSender<String>,
    answers: tokio::sync::mpsc::UnboundedReceiver<String>,
) -> Result<(String, Option<Snippet>), ToolError> {
    let dir = std::env::temp_dir().join("tern-python");
    std::fs::create_dir_all(&dir).map_err(|e| ToolError::Failed(e.to_string()))?;
    let stamp = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    );
    let file = dir.join(format!("script-{stamp}.py"));
    let tools = dir.join(format!("script-{stamp}.tools.json"));
    std::fs::write(&file, code).map_err(|e| ToolError::Failed(e.to_string()))?;
    std::fs::write(
        &tools,
        serde_json::to_string(stubs).map_err(|e| ToolError::Failed(e.to_string()))?,
    )
    .map_err(|e| ToolError::Failed(e.to_string()))?;
    let mut policy = crate::sandbox::SandboxPolicy::for_folder(&dir);
    policy.limits.memory_mb = 256;
    policy.limits.processes = 1;
    policy.limits.wall = SCRIPT_WALL;
    policy.limits.cpu_secs = 30;
    let prepared = crate::sandbox::Prepared {
        program: exe.to_path_buf(),
        args: vec![
            WORKER_FLAG.to_owned(),
            file.display().to_string(),
            TOOLS_FLAG.to_owned(),
            tools.display().to_string(),
        ],
        cwd: policy.folder.clone(),
        display: format!("python (Monty, code mode) · {} lines", code.lines().count()),
        outside: Vec::new(),
    };
    let host = crate::sandbox::Host {
        mark: CALL_MARK,
        requests: calls,
        replies: answers,
    };
    let o = crate::sandbox::run_hosted(&policy, &prepared, Some(host)).await;
    let _ = std::fs::remove_file(&file);
    let _ = std::fs::remove_file(&tools);
    let (printed, snippet) = parse_worker(&o.stdout);
    let snippet = snippet.or_else(|| {
        (o.state != crate::live::RunState::Exited).then(|| Snippet {
            value: None,
            printed: String::new(),
            error: Some(format!(
                "the program was stopped: {} (limits: {} s of Python, 256 MB)",
                o.state.label(),
                RUN_TIME.as_secs()
            )),
        })
    });
    Ok((printed, snippet))
}

/// Reads the worker's result line out of its stdout; the rest is prints.
fn parse_worker(stdout: &str) -> (String, Option<Snippet>) {
    match stdout.rfind(RESULT_MARK) {
        Some(i) => {
            let printed = stdout[..i].to_owned();
            let line = stdout[i + RESULT_MARK.len()..].lines().next().unwrap_or("");
            (printed, serde_json::from_str(line).ok())
        }
        None => (stdout.to_owned(), None),
    }
}

/// The `run_python` tool.
#[derive(Debug, Clone, Copy)]
pub struct RunPython;

impl Tool for RunPython {
    fn name(&self) -> &'static str {
        "run_python"
    }
    fn summary(&self) -> &'static str {
        "Run a short Python program (a Python subset: no imports beyond basic standard modules such as math, re, json, datetime; no files, network or packages like pandas) for glue work: reshaping data you already have, sums, unit conversions, cleaning citations. Put the data in the code. Returns what it printed and the value of the last expression."
    }
    fn effect(&self) -> EffectClass {
        EffectClass::Analysis
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object", "required": ["code"], "additionalProperties": false,
               "properties": {"code": {"type": "string"}}})
    }
    fn timeout(&self) -> Duration {
        Duration::from_secs(60)
    }
    fn call<'a>(&'a self, args: Value) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let code = args["code"].as_str().unwrap_or_default().to_owned();
            if code.trim().is_empty() {
                return Err(ToolError::BadInput("`code` is empty".into()));
            }
            if code.len() > MAX_CODE {
                return Err(ToolError::BadInput(
                    "the program is longer than 200 KB".into(),
                ));
            }
            let (printed, snippet) = match worker_exe() {
                Some(exe) => run_in_worker(&exe, &code).await?,
                None => {
                    let s =
                        tokio::task::spawn_blocking(move || run_snippet(&code, RUN_TIME, false))
                            .await
                            .map_err(|e| ToolError::Failed(e.to_string()))?;
                    (s.printed.clone(), Some(s))
                }
            };
            output_of(printed, snippet, false)
        })
    }
}

/// The executable to start as a worker: Tern's own, when it is a Tern
/// binary (not a test harness). `TERN_PYTHON_INPROC=1` runs in-process.
fn worker_exe() -> Option<std::path::PathBuf> {
    if std::env::var_os("TERN_PYTHON_INPROC").is_some() {
        return None;
    }
    let exe = std::env::current_exe().ok()?;
    let stem = exe.file_stem()?.to_string_lossy().to_ascii_lowercase();
    matches!(stem.as_str(), "tern" | "tern-desktop").then_some(exe)
}

async fn run_in_worker(exe: &Path, code: &str) -> Result<(String, Option<Snippet>), ToolError> {
    let dir = std::env::temp_dir().join("tern-python");
    std::fs::create_dir_all(&dir).map_err(|e| ToolError::Failed(e.to_string()))?;
    let file = dir.join(format!(
        "glue-{}-{}.py",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::write(&file, code).map_err(|e| ToolError::Failed(e.to_string()))?;
    let mut policy = crate::sandbox::SandboxPolicy::for_folder(&dir);
    policy.limits.memory_mb = 256;
    policy.limits.processes = 1;
    policy.limits.wall = RUN_TIME + Duration::from_secs(5);
    policy.limits.cpu_secs = 30;
    let prepared = crate::sandbox::Prepared {
        program: exe.to_path_buf(),
        args: vec![WORKER_FLAG.to_owned(), file.display().to_string()],
        cwd: policy.folder.clone(),
        display: format!("python (Monty) · {} lines", code.lines().count()),
        outside: Vec::new(),
    };
    let o = crate::sandbox::run(&policy, &prepared).await;
    let _ = std::fs::remove_file(&file);
    let (printed, snippet) = parse_worker(&o.stdout);
    let snippet = snippet.or_else(|| {
        (o.state != crate::live::RunState::Exited).then(|| Snippet {
            value: None,
            printed: String::new(),
            error: Some(format!(
                "the program was stopped: {} (limits: {} s, 256 MB)",
                o.state.label(),
                RUN_TIME.as_secs()
            )),
        })
    });
    Ok((printed, snippet))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glue_code_runs_with_limits() {
        let s = run_snippet(
            "x = [1, 2, 3]\nprint(sum(x))\nlen(x) * 2",
            Duration::from_secs(5),
            false,
        );
        assert_eq!(s.error, None);
        assert_eq!(s.value.as_deref(), Some("6"));
        assert_eq!(s.printed.trim(), "6");
        let s = run_snippet("while True:\n    pass", Duration::from_millis(100), false);
        assert!(s.error.is_some());
        let s = run_snippet(
            "def f(n):\n    return f(n + 1)\nf(0)",
            Duration::from_secs(5),
            false,
        );
        assert!(s.error.is_some());
        let s = run_snippet("open('/etc/passwd').read()", Duration::from_secs(5), false);
        assert!(
            s.error.is_some(),
            "files must be closed to the sandbox: {s:?}"
        );
        let s = run_snippet("import socket", Duration::from_secs(5), false);
        assert!(s.error.is_some());
    }

    #[test]
    fn the_worker_result_line_is_found() {
        let (p, s) = parse_worker(&format!(
            "hello\n{RESULT_MARK}{}\n",
            serde_json::to_string(&Snippet {
                value: Some("3".into()),
                printed: String::new(),
                error: None
            })
            .unwrap()
        ));
        assert_eq!(p, "hello\n");
        assert_eq!(s.unwrap().value.as_deref(), Some("3"));
        assert!(parse_worker("no mark").1.is_none());
    }

    #[tokio::test]
    async fn the_tool_runs_in_process_under_tests() {
        let out = RunPython
            .call(json!({"code": "print('a', 1 + 1)"}))
            .await
            .unwrap();
        assert!(out.content.contains("a 2"), "{}", out.content);
        assert!(RunPython.call(json!({"code": "1/0"})).await.is_err());
    }
}
