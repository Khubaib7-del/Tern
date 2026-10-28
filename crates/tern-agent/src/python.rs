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
//! Code mode (typed tool stubs the program calls) is planned for the next
//! pull request; this tool only runs code the model wrote.

use std::path::Path;
use std::time::Duration;

use monty::MontyRun;
use monty_types::{CompileOptions, PrintWriter, ResourceLimits, ResourceTracker};
use serde_json::{Value, json};
use tern_core::EffectClass;

use crate::tools::{BoxFuture, Tool, ToolError, ToolOutput};

/// The flag that turns Tern's executable into a Python worker.
pub const WORKER_FLAG: &str = "--tern-python-worker";
/// Marks the worker's result line on stdout.
const RESULT_MARK: &str = "\u{1e}TERN-PYTHON-RESULT ";
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
/// otherwise `None` and the app starts as usual.
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
    let s = run_snippet(&code, RUN_TIME, true);
    if let Some(e) = &s.error {
        eprintln!("{e}");
    }
    println!(
        "{RESULT_MARK}{}",
        serde_json::to_string(&s).unwrap_or_default()
    );
    Some(if s.error.is_some() { 1 } else { 0 })
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
                text.push_str("Ran with no output.");
            }
            Ok(ToolOutput {
                content: text.trim_end().to_owned(),
                evidence_refs: Vec::new(),
                meta: json!({"engine": "monty"}),
            })
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
