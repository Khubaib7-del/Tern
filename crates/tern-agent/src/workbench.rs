//! The research agent's workbench tools (#75): `run_command` (a program
//! under sandbox v1, in the chat's folder: conversions, a script over a
//! data file, a citation tool) and `show_in_panel` (asks the window to
//! show a tab of the right panel). `run_python` is in [`crate::python`].

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde_json::{Value, json};
use tern_core::EffectClass;

use crate::sandbox::{self, SandboxPolicy};
use crate::tools::{BoxFuture, Tool, ToolError, ToolOutput, ToolRegistry};
use crate::workspace::Workspace;

/// What the user set for the workbench in Settings › Permissions.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorkbenchSettings {
    /// Programs added to the default list.
    pub extra_programs: Vec<String>,
    /// Network for spawned commands (off by default).
    pub network: bool,
}

fn settings() -> &'static Mutex<WorkbenchSettings> {
    static S: OnceLock<Mutex<WorkbenchSettings>> = OnceLock::new();
    S.get_or_init(Mutex::default)
}

/// Applies the user's workbench settings to commands from now on.
pub fn set_settings(s: WorkbenchSettings) {
    *settings().lock().unwrap_or_else(|e| e.into_inner()) = s;
}

/// The sandbox policy for a command in `folder`, with the user's settings.
pub fn policy_for(folder: &std::path::Path) -> SandboxPolicy {
    let s = settings().lock().unwrap_or_else(|e| e.into_inner()).clone();
    let mut p = SandboxPolicy::for_folder(folder);
    p.network = s.network;
    for x in s.extra_programs {
        let x = x.trim().to_owned();
        if !x.is_empty() && !p.programs.iter().any(|q| q.eq_ignore_ascii_case(&x)) {
            p.programs.push(x);
        }
    }
    p
}

/// Registers the workbench tools for a chat with a folder.
pub fn register(ws: &Arc<Workspace>, tools: &mut ToolRegistry) {
    tools.register(RunCommand(ws.clone()));
}

/// Registers the tools every chat has (no folder needed).
pub fn register_always(tools: &mut ToolRegistry) {
    tools.register(ShowInPanel);
    tools.register(crate::python::RunPython);
    // #77: the visible plan and the task graph.
    tools.register(crate::planner::PlanTool);
    tools.register(crate::planner::RunTasksTool);
}

#[derive(Debug, Clone)]
struct RunCommand(Arc<Workspace>);

impl RunCommand {
    fn prepared(&self, args: &Value) -> Result<sandbox::Prepared, sandbox::Denial> {
        let policy = policy_for(self.0.root());
        sandbox::prepare(
            &policy,
            args["command"].as_str().unwrap_or_default(),
            &[],
            args["cwd"].as_str(),
        )
    }
}

impl Tool for RunCommand {
    fn name(&self) -> &'static str {
        "run_command"
    }
    fn summary(&self) -> &'static str {
        "Run one program in the chat's folder on Tern's sandboxed workbench, for conversions and data glue (pandoc, typst, python scripts, ffmpeg, git status...). One program per call, no shell, pipes or redirection. Network is off unless the user turned it on. Returns the exit code, stdout and stderr."
    }
    fn effect(&self) -> EffectClass {
        EffectClass::ExternalSideEffect
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object", "required": ["command"], "additionalProperties": false,
        "properties": {
            "command": {"type": "string", "description": "The program and its arguments, e.g. `pandoc notes.md -o notes.docx`"},
            "cwd": {"type": "string", "description": "A folder inside the chat's folder to run in (relative); empty for the folder itself"},
            "timeout_s": {"type": "integer", "description": "Seconds before it is stopped (at most 600; default 120)"}
        }})
    }
    fn target(&self, args: &Value) -> Option<String> {
        args["command"].as_str().map(str::to_owned)
    }
    fn refuses(&self, args: &Value) -> Option<String> {
        self.prepared(args).err().map(|d| d.reason)
    }
    fn asks_because(&self, args: &Value) -> Option<String> {
        let p = self.prepared(args).ok()?;
        p.outside.first().map(|o| {
            format!(
                "it names {}, outside the chat's folder{}",
                crate::projects::plain_path(o).display(),
                if p.outside.len() > 1 {
                    format!(" (and {} more)", p.outside.len() - 1)
                } else {
                    String::new()
                }
            )
        })
    }
    fn confined(&self, args: &Value) -> bool {
        self.prepared(args).is_ok_and(|p| p.outside.is_empty())
    }
    fn rule_paths(&self, args: &Value) -> Vec<PathBuf> {
        match self.prepared(args) {
            Ok(p) => std::iter::once(p.cwd).chain(p.outside).collect(),
            Err(_) => Vec::new(),
        }
    }
    fn timeout(&self) -> Duration {
        // The sandbox's own limit stops the command first.
        Duration::from_secs(660)
    }
    fn call<'a>(&'a self, args: Value) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let mut policy = policy_for(self.0.root());
            if let Some(t) = args["timeout_s"].as_u64() {
                policy.limits.wall = Duration::from_secs(t.clamp(1, 600));
            }
            let command = args["command"].as_str().unwrap_or_default();
            let prepared = match sandbox::prepare(&policy, command, &[], args["cwd"].as_str()) {
                Ok(p) => p,
                Err(d) => {
                    sandbox::note_denied(command, &policy.folder, &d.reason);
                    return Err(ToolError::Failed(format!(
                        "the sandbox refused it: {}",
                        d.reason
                    )));
                }
            };
            let o = sandbox::run(&policy, &prepared).await;
            let mut text = match o.state {
                crate::live::RunState::Exited => format!(
                    "Exit code {} in {:.1} s.",
                    o.exit
                        .map(|c| c.to_string())
                        .unwrap_or_else(|| "unknown".into()),
                    o.took.as_secs_f32()
                ),
                // #76: the real cause, stated plainly, so the model reports
                // it instead of guessing ("a stricter limit", "the
                // environment").
                crate::live::RunState::Killed => format!(
                    "The command was killed after {:.1} s: the user stopped it (Kill in the Terminal tab, or Stop). It did not finish. That is the only reason it ended; tell the user exactly that and do not suggest other causes.",
                    o.took.as_secs_f32()
                ),
                crate::live::RunState::TimedOut => format!(
                    "The command was stopped after {:.1} s because it reached Tern's time limit of {} s for one command. It did not finish. That is the only reason it ended; tell the user exactly that (a longer timeout_s, up to 600, may let it finish).",
                    o.took.as_secs_f32(),
                    policy.limits.wall.as_secs()
                ),
                s => format!(
                    "The command {} after {:.1} s.",
                    s.label(),
                    o.took.as_secs_f32()
                ),
            };
            if !o.stdout.trim().is_empty() {
                text.push_str(&format!("\nstdout:\n{}", o.stdout.trim_end()));
            }
            if !o.stderr.trim().is_empty() {
                text.push_str(&format!("\nstderr:\n{}", o.stderr.trim_end()));
            }
            if o.stdout.trim().is_empty() && o.stderr.trim().is_empty() {
                text.push_str(" No output.");
            }
            let out = ToolOutput {
                content: text,
                evidence_refs: Vec::new(),
                meta: json!({
                    "run": o.run, "exit": o.exit, "state": o.state.label(),
                    "took_ms": o.took.as_millis() as u64, "command": prepared.display,
                    "sandbox": policy.summary(),
                }),
            };
            if o.state == crate::live::RunState::Failed {
                return Err(ToolError::Failed(out.content));
            }
            Ok(out)
        })
    }
}

/// The right panel's tabs `show_in_panel` may open.
pub const PANEL_TABS: [&str; 5] = ["files", "terminal", "outputs", "sources", "report"];

#[derive(Debug, Clone, Copy)]
struct ShowInPanel;

impl Tool for ShowInPanel {
    fn name(&self) -> &'static str {
        "show_in_panel"
    }
    fn summary(&self) -> &'static str {
        "Show the user a tab of the right panel: `files` (optionally with `path`, a file in the chat's folder, previewed), `terminal`, `outputs`, `sources` or `report`. Use it after writing a file or running a command the user should look at."
    }
    fn effect(&self) -> EffectClass {
        EffectClass::Analysis
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object", "required": ["tab"], "additionalProperties": false,
               "properties": {"tab": {"type": "string", "enum": PANEL_TABS},
                              "path": {"type": "string"}}})
    }
    fn call<'a>(&'a self, args: Value) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let tab = args["tab"]
                .as_str()
                .unwrap_or_default()
                .to_ascii_lowercase();
            if !PANEL_TABS.contains(&tab.as_str()) {
                return Err(ToolError::BadInput(format!(
                    "`tab` must be one of {}",
                    PANEL_TABS.join(", ")
                )));
            }
            let path = args["path"].as_str().filter(|p| !p.trim().is_empty());
            crate::live::request_panel(&tab, path);
            Ok(ToolOutput::text(match path {
                Some(p) => format!("Showing {p} in the {tab} tab."),
                None => format!("Showing the {tab} tab."),
            }))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn show_in_panel_posts_a_request() {
        let before = crate::live::panel_request(0).map(|r| r.seq).unwrap_or(0);
        let out = ShowInPanel.call(json!({"tab": "terminal"})).await.unwrap();
        assert!(out.content.contains("terminal"));
        let r = crate::live::panel_request(before).unwrap();
        assert_eq!(r.tab, "terminal");
        assert!(ShowInPanel.call(json!({"tab": "kitchen"})).await.is_err());
    }

    #[test]
    fn run_command_refuses_shells_and_confines_to_the_folder() {
        let d = tempfile::tempdir().unwrap();
        let ws = Arc::new(Workspace::open(d.path()).unwrap());
        let t = RunCommand(ws);
        assert!(
            t.refuses(&json!({"command": "bash -c 'rm -rf /'"}))
                .is_some()
        );
        assert!(
            t.refuses(&json!({"command": "pandoc a.md | cat"}))
                .is_some()
        );
        set_settings(WorkbenchSettings {
            extra_programs: vec!["ls".into()],
            network: false,
        });
        if t.refuses(&json!({"command": "ls"})).is_none() {
            assert!(t.confined(&json!({"command": "ls ."})));
            assert!(t.asks_because(&json!({"command": "ls /"})).is_some());
        }
        set_settings(WorkbenchSettings::default());
    }
}
