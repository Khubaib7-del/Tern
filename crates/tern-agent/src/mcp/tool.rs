//! An MCP tool in Tern's typed tool registry.
//!
//! The model sees it as `mcp__<server>__<tool>`. Its effect class comes
//! from the server's hints, read only to be more careful: a tool hinted
//! destructive is destructive, every other tool acts outside Tern, and
//! one hinted read-only still reaches the network. Each tool's approval
//! (ask, allow, refuse) is a per-tool mode in the policy gate. What it
//! returns is wrapped as untrusted data (see [`crate::untrusted`]).

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde_json::Value;
use tern_core::EffectClass;

use super::client::{McpClient, ToolInfo};
use crate::tools::{BoxFuture, Tool, ToolError, ToolOutput};

/// Leaks each distinct string once, for the registry's `&'static str`
/// names. Tool lists are small and repeat, so this stays bounded.
pub(crate) fn intern(s: &str) -> &'static str {
    static SET: OnceLock<Mutex<BTreeSet<&'static str>>> = OnceLock::new();
    let mut set = SET.get_or_init(Default::default).lock().expect("intern");
    if let Some(v) = set.get(s) {
        return v;
    }
    let v: &'static str = Box::leak(s.to_owned().into_boxed_str());
    set.insert(v);
    v
}

/// The name the model calls: `mcp__<server>__<tool>`, at most 64
/// characters of letters, digits, `_` and `-`.
pub fn tern_name(server: &str, tool: &str) -> String {
    let clean = |s: &str| {
        s.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect::<String>()
    };
    let mut n = format!("mcp__{}__{}", clean(server), clean(tool));
    n.truncate(64);
    n
}

/// The effect class for a tool, from its hints.
pub fn effect_of(t: &ToolInfo) -> EffectClass {
    if t.destructive {
        EffectClass::Destructive
    } else if t.read_only {
        EffectClass::Network
    } else {
        EffectClass::ExternalSideEffect
    }
}

/// One MCP tool, bound to its server's connection.
pub struct McpTool {
    name: &'static str,
    summary: &'static str,
    server: String,
    tool: String,
    schema: Value,
    effect: EffectClass,
    timeout: Duration,
    client: Arc<McpClient>,
}

impl std::fmt::Debug for McpTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpTool").field("name", &self.name).finish()
    }
}

impl McpTool {
    pub fn new(server: &str, info: &ToolInfo, client: Arc<McpClient>, timeout: Duration) -> Self {
        // The description is the server's text: shortened, and cleaned of
        // invisible characters, it is shown as the summary. It reaches the
        // model only for tools the user trusted.
        let d: String = crate::untrusted::clean(&info.description)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let d: String = d.chars().take(300).collect();
        let summary = format!(
            "[MCP server “{server}”] {}",
            if d.is_empty() { info.name.as_str() } else { &d }
        );
        Self {
            name: intern(&tern_name(server, &info.name)),
            summary: intern(&summary),
            server: server.to_owned(),
            tool: info.name.clone(),
            schema: info.input_schema.clone(),
            effect: effect_of(info),
            timeout,
            client,
        }
    }

    fn source(&self) -> String {
        format!("mcp:{}/{}", self.server, self.tool)
    }
}

impl Tool for McpTool {
    fn name(&self) -> &'static str {
        self.name
    }

    fn summary(&self) -> &'static str {
        self.summary
    }

    fn effect(&self) -> EffectClass {
        self.effect
    }

    fn input_schema(&self) -> Value {
        self.schema.clone()
    }

    fn target(&self, _args: &Value) -> Option<String> {
        Some(format!("mcp:{}", self.server))
    }

    fn timeout(&self) -> Duration {
        self.timeout
    }

    fn idempotent(&self) -> bool {
        false
    }

    fn call<'a>(&'a self, args: Value) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let r = self
                .client
                .call_tool(&self.tool, args)
                .await
                .map_err(|e| ToolError::Failed(format!("MCP server “{}” {e}", self.server)))?;
            let body = if r.text.trim().is_empty() {
                "(no content)".to_owned()
            } else {
                r.text
            };
            let wrapped = crate::untrusted::wrap(&self.source(), &body);
            if r.is_error {
                return Err(ToolError::Failed(format!(
                    "the MCP tool reported an error:\n{wrapped}"
                )));
            }
            let mut out = ToolOutput::text(wrapped);
            out.meta = serde_json::json!({"mcp": {"server": self.server, "tool": self.tool, "untrusted": true}});
            Ok(out)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_clean_and_short() {
        assert_eq!(tern_name("notes", "search.v2"), "mcp__notes__search_v2");
        assert!(tern_name("s", &"x".repeat(100)).len() <= 64);
        assert!(std::ptr::eq(intern("abc"), intern("abc")));
    }
}
