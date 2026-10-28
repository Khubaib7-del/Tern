//! The typed tool registry (`docs/03-architecture.md`, section 4.3).
//!
//! A tool states its name, version, one-line summary, effect class, the
//! target a call touches, a JSON Schema for its input, a timeout, and whether
//! a retry is safe. The turn loop validates the input shape, asks the policy
//! gate, and only then calls [`Tool::call`].

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use tern_core::EffectClass;

/// A boxed, sendable future, so tools can live behind `dyn`.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// What a tool returns to the model.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ToolOutput {
    /// Text the model reads next.
    pub content: String,
    /// IDs of sources, spans, or artifacts this result rests on.
    pub evidence_refs: Vec<String>,
    /// Structured facts for front ends (the page a read returned), logged
    /// with `tool.executed`. The model never sees it.
    #[serde(skip_serializing_if = "serde_json::Value::is_null")]
    pub meta: serde_json::Value,
}

impl ToolOutput {
    pub fn text(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            evidence_refs: Vec::new(),
            meta: serde_json::Value::Null,
        }
    }
}

/// A tool failure, worded for both the model and the user.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "message", rename_all = "snake_case")]
pub enum ToolError {
    /// The arguments did not match the schema.
    BadInput(String),
    /// The tool ran and failed.
    Failed(String),
    TimedOut,
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadInput(m) => write!(f, "bad input: {m}"),
            Self::Failed(m) => write!(f, "failed: {m}"),
            Self::TimedOut => write!(f, "timed out"),
        }
    }
}

pub trait Tool: Send + Sync + std::fmt::Debug {
    fn name(&self) -> &'static str;
    fn version(&self) -> u32 {
        1
    }
    /// What it does, in one line. Shown to the model and in approvals.
    fn summary(&self) -> &'static str;
    fn effect(&self) -> EffectClass;
    /// JSON Schema for the input. Phase 3 checks `required` and property types.
    fn input_schema(&self) -> Value;
    /// What a call touches, for scoped grants (a domain, a path).
    fn target(&self, _args: &Value) -> Option<String> {
        None
    }
    /// Why this call needs the user's approval even where the tool's
    /// effect is allowed (a file outside the workspace), or `None`.
    fn asks_because(&self, _args: &Value) -> Option<String> {
        None
    }
    /// Why this call is refused before anyone is asked (the sandbox's
    /// program list, a protected path), or `None`.
    fn refuses(&self, _args: &Value) -> Option<String> {
        None
    }
    /// Whether the call stays inside the chat's folder under the sandbox
    /// (a file write there, a sandboxed command with no outside paths).
    /// The Workspace write approval mode runs such calls without asking.
    fn confined(&self, _args: &Value) -> bool {
        false
    }
    /// The full paths a call touches, for path permission rules.
    fn rule_paths(&self, _args: &Value) -> Vec<std::path::PathBuf> {
        Vec::new()
    }
    fn timeout(&self) -> Duration {
        Duration::from_secs(30)
    }
    /// Whether running the same call twice is harmless.
    fn idempotent(&self) -> bool {
        matches!(self.effect(), EffectClass::Read | EffectClass::Analysis)
    }
    fn call<'a>(&'a self, args: Value) -> BoxFuture<'a, Result<ToolOutput, ToolError>>;
}

/// The tools one turn may use.
#[derive(Debug, Default)]
pub struct ToolRegistry {
    tools: BTreeMap<&'static str, Box<dyn Tool>>,
    /// The turn's mode: tools it does not allow are hidden. `None` shows
    /// every tool.
    mode: Option<crate::workspace::TurnMode>,
    /// Tools the user turned off in Settings › Tools: never registered,
    /// whichever path tries to add them (a workspace opened later, web
    /// search switched back on).
    off: std::collections::BTreeSet<String>,
}

/// One of Tern's own tools, as Settings › Tools lists it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BuiltinTool {
    pub name: &'static str,
    /// What it does, in a few words.
    pub label: &'static str,
    pub effect: EffectClass,
}

/// Tern's own tools (MCP tools are listed per server). Settings › Tools
/// shows these with an on/off switch and an Ask / Allow / Deny choice.
pub const BUILTIN_TOOLS: &[BuiltinTool] = &[
    BuiltinTool {
        name: "web_search",
        label: "Search the web",
        effect: EffectClass::Network,
    },
    BuiltinTool {
        name: "read_url",
        label: "Read a web page (Tern Crawl)",
        effect: EffectClass::Network,
    },
    BuiltinTool {
        name: "vault_search",
        label: "Search your vault notes",
        effect: EffectClass::Read,
    },
    BuiltinTool {
        name: "vault_read",
        label: "Read a vault note",
        effect: EffectClass::Read,
    },
    BuiltinTool {
        name: "vault_write",
        label: "Write a vault note",
        effect: EffectClass::Write,
    },
    BuiltinTool {
        name: "files_list",
        label: "List workspace files",
        effect: EffectClass::Read,
    },
    BuiltinTool {
        name: "file_read",
        label: "Read a workspace file",
        effect: EffectClass::Read,
    },
    BuiltinTool {
        name: "file_search",
        label: "Search workspace files",
        effect: EffectClass::Read,
    },
    BuiltinTool {
        name: "file_write",
        label: "Write a workspace file",
        // The tool's own class (#77): Settings › Tools showed "Auto
        // (runs)" for it while it asked.
        effect: EffectClass::ExternalSideEffect,
    },
    BuiltinTool {
        name: "file_rename",
        label: "Rename or move a workspace file",
        effect: EffectClass::ExternalSideEffect,
    },
    BuiltinTool {
        name: "run_command",
        label: "Run a command on the workbench (sandboxed)",
        effect: EffectClass::ExternalSideEffect,
    },
    BuiltinTool {
        name: "run_python",
        label: "Run Python glue code (Monty, no files or network)",
        effect: EffectClass::Analysis,
    },
    BuiltinTool {
        name: "show_in_panel",
        label: "Show a tab in the right panel",
        effect: EffectClass::Analysis,
    },
    BuiltinTool {
        name: "load_skill",
        label: "Load a skill",
        effect: EffectClass::Read,
    },
];

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a tool. A second tool with the same name replaces the first.
    pub fn register(&mut self, tool: impl Tool + 'static) -> &mut Self {
        if !self.off.contains(tool.name()) {
            self.tools.insert(tool.name(), Box::new(tool));
        }
        self
    }

    /// Turns tools off: takes them out now and keeps them out of every
    /// later [`Self::register`].
    pub fn turn_off<'a>(&mut self, names: impl IntoIterator<Item = &'a str>) {
        for n in names {
            self.tools.remove(n);
            self.off.insert(n.to_owned());
        }
    }

    /// Takes a tool out (a chat whose web search was turned off).
    pub fn unregister(&mut self, name: &str) -> bool {
        self.tools.remove(name).is_some()
    }

    /// Hides the tools `mode` does not allow (see
    /// [`crate::workspace::TurnMode`]).
    pub fn set_mode(&mut self, mode: crate::workspace::TurnMode) {
        self.mode = Some(mode);
    }

    fn shown(&self, t: &dyn Tool) -> bool {
        self.mode.is_none_or(|m| m.allows(t.effect()))
    }

    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools
            .get(name)
            .map(|t| t.as_ref())
            .filter(|t| self.shown(*t))
    }

    pub fn is_empty(&self) -> bool {
        self.names().next().is_none()
    }

    pub fn names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.tools
            .iter()
            .filter(|(_, t)| self.shown(t.as_ref()))
            .map(|(n, _)| *n)
    }

    /// The offered tools for native function calling (the provider's own
    /// tool format), in the same order as [`ToolRegistry::describe`].
    pub fn specs(&self) -> Vec<tern_providers::ToolSpec> {
        let (mcp, own): (Vec<&str>, Vec<&str>) = self.names().partition(|n| n.starts_with("mcp__"));
        own.into_iter()
            .chain(mcp)
            .filter_map(|n| self.get(n))
            .map(|t| tern_providers::ToolSpec {
                name: t.name().to_owned(),
                description: t.summary().to_owned(),
                parameters: t.input_schema(),
            })
            .collect()
    }

    /// The tool list as the model sees it in the system prompt: Tern's
    /// own tools, then MCP tools.
    pub fn describe(&self) -> String {
        let (mcp, own): (Vec<&str>, Vec<&str>) = self.names().partition(|n| n.starts_with("mcp__"));
        [self.describe_only(&own), self.describe_only(&mcp)]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The listed tools' entries, in the order given.
    pub fn describe_only(&self, names: &[&str]) -> String {
        names
            .iter()
            .filter_map(|n| self.get(n))
            .map(|t| {
                format!(
                    "- {} (v{}, {}): {}\n  input schema: {}",
                    t.name(),
                    t.version(),
                    t.effect().label(),
                    t.summary(),
                    t.input_schema()
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Checks `args` against the subset of JSON Schema tools use in phase 3:
/// an object with `required` keys and typed `properties`.
pub fn validate(schema: &Value, args: &Value) -> Result<(), ToolError> {
    let obj = args
        .as_object()
        .ok_or_else(|| ToolError::BadInput("arguments must be a JSON object".into()))?;
    if let Some(req) = schema.get("required").and_then(Value::as_array) {
        for k in req.iter().filter_map(Value::as_str) {
            if !obj.contains_key(k) {
                return Err(ToolError::BadInput(format!("missing `{k}`")));
            }
        }
    }
    if let Some(props) = schema.get("properties").and_then(Value::as_object) {
        for (k, v) in obj {
            let Some(p) = props.get(k) else {
                if schema.get("additionalProperties") == Some(&Value::Bool(false)) {
                    return Err(ToolError::BadInput(format!("unknown field `{k}`")));
                }
                continue;
            };
            let Some(ty) = p.get("type").and_then(Value::as_str) else {
                continue;
            };
            let ok = match ty {
                "string" => v.is_string(),
                "integer" => v.is_i64() || v.is_u64(),
                "number" => v.is_number(),
                "boolean" => v.is_boolean(),
                "array" => v.is_array(),
                "object" => v.is_object(),
                _ => true,
            };
            if !ok {
                return Err(ToolError::BadInput(format!("`{k}` must be a {ty}")));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn schema() -> Value {
        json!({
            "type": "object",
            "properties": {"query": {"type": "string"}, "limit": {"type": "integer"}},
            "required": ["query"],
            "additionalProperties": false
        })
    }

    #[test]
    fn validation_checks_required_types_and_extras() {
        assert!(validate(&schema(), &json!({"query": "tern", "limit": 3})).is_ok());
        assert_eq!(
            validate(&schema(), &json!({"limit": 3})),
            Err(ToolError::BadInput("missing `query`".into()))
        );
        assert_eq!(
            validate(&schema(), &json!({"query": 4})),
            Err(ToolError::BadInput("`query` must be a string".into()))
        );
        assert!(validate(&schema(), &json!({"query": "x", "rm": true})).is_err());
        assert!(validate(&schema(), &json!("query")).is_err());
    }

    #[test]
    fn tools_turned_off_stay_out() {
        let mut t = ToolRegistry::new();
        t.register(crate::web::ReadUrlTool::default());
        assert!(t.get("read_url").is_some());
        t.turn_off(["read_url"]);
        assert!(t.get("read_url").is_none());
        // A later path that adds it again is ignored.
        t.register(crate::web::ReadUrlTool::default());
        assert!(t.get("read_url").is_none() && t.is_empty());
    }

    #[test]
    fn the_builtin_list_names_each_tool_once() {
        let mut names: Vec<_> = BUILTIN_TOOLS.iter().map(|t| t.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), BUILTIN_TOOLS.len());
        let read = crate::web::ReadUrlTool::default();
        let row = BUILTIN_TOOLS
            .iter()
            .find(|t| t.name == read.name())
            .unwrap();
        assert_eq!(row.effect, read.effect());
    }
}
