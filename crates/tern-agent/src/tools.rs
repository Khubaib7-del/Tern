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
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ToolOutput {
    /// Text the model reads next.
    pub content: String,
    /// IDs of sources, spans, or artifacts this result rests on.
    pub evidence_refs: Vec<String>,
}

impl ToolOutput {
    pub fn text(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            evidence_refs: Vec::new(),
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
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a tool. A second tool with the same name replaces the first.
    pub fn register(&mut self, tool: impl Tool + 'static) -> &mut Self {
        self.tools.insert(tool.name(), Box::new(tool));
        self
    }

    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.get(name).map(|t| t.as_ref())
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    pub fn names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.tools.keys().copied()
    }

    /// The tool list as the model sees it in the system prompt.
    pub fn describe(&self) -> String {
        self.tools
            .values()
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
}
