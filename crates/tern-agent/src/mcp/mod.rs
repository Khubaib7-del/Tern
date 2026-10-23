//! The MCP client: servers in `<data>/mcp.json` ([`config`]), a JSON-RPC
//! client over stdio and Streamable HTTP ([`client`]), the trust preview a
//! user reads before any server's tools reach a chat ([`trust`]), MCP
//! tools in the typed registry with their output treated as untrusted
//! data ([`tool`]), and health checks.
//!
//! Connections are shared across chats in one process ([`hub`]): a stdio
//! server starts once, on first use, and is restarted by a health check
//! or by the user.

pub mod client;
pub mod config;
pub mod tool;
pub mod trust;

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use serde::Serialize;

pub use client::{CallResult, LATEST, LEGACY, McpClient, McpError, ServerInfo, ToolInfo};
pub use config::{McpConfig, ServerConfig, ToolSetting, Transport};
pub use tool::McpTool;
pub use trust::{Flag, Severity, ToolPreview, TrustPreview};

use crate::policy::PolicyGate;
use crate::tools::ToolRegistry;

/// A server's state as the health dot shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthState {
    /// Started, answered a ping, and listed its tools.
    Ok,
    /// Working, but a round trip took over a second.
    Slow,
    /// Would not start, or stopped answering.
    Down,
    /// Switched off.
    Off,
    /// Not reviewed yet, or its tools changed since the review.
    NeedsReview,
}

impl HealthState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Slow => "slow",
            Self::Down => "down",
            Self::Off => "off",
            Self::NeedsReview => "needs review",
        }
    }
}

/// One health check's result.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Health {
    pub server: String,
    pub state: HealthState,
    /// How long starting (or reaching) and agreeing a version took.
    pub start_ms: Option<u64>,
    /// One ping's round trip.
    pub ping_ms: Option<u64>,
    /// How long listing the tools took.
    pub list_ms: Option<u64>,
    pub tools: usize,
    pub protocol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

const SLOW: Duration = Duration::from_secs(1);

/// Starts the server (or reuses the shared connection), pings it, and
/// lists its tools, timing each step. `restart` stops the shared
/// connection first.
pub async fn check(name: &str, cfg: &ServerConfig, restart: bool) -> Health {
    let mut h = Health {
        server: name.to_owned(),
        state: HealthState::Down,
        start_ms: None,
        ping_ms: None,
        list_ms: None,
        tools: 0,
        protocol: None,
        error: None,
    };
    if restart {
        hub().stop(name).await;
    }
    let client = match hub().client(name, cfg).await {
        Ok(c) => c,
        Err(e) => {
            h.error = Some(e.to_string());
            return h;
        }
    };
    h.start_ms = Some(client.started.as_millis() as u64);
    h.protocol = Some(client.info().protocol.clone());
    let ping = match client.ping().await {
        Ok(d) => d,
        Err(e) => {
            // A dead connection is dropped so the next use starts fresh.
            hub().stop(name).await;
            h.error = Some(e.to_string());
            return h;
        }
    };
    h.ping_ms = Some(ping.as_millis() as u64);
    let t0 = std::time::Instant::now();
    match client.list_tools().await {
        Ok(tools) => {
            let list = t0.elapsed();
            h.list_ms = Some(list.as_millis() as u64);
            h.tools = tools.len();
            let fp = trust::fingerprint(&tools);
            h.state = if cfg.disabled || !cfg.tern.enabled {
                HealthState::Off
            } else if cfg.tern.trusted.as_deref() != Some(fp.as_str()) {
                HealthState::NeedsReview
            } else if ping > SLOW || list > SLOW {
                HealthState::Slow
            } else {
                HealthState::Ok
            };
        }
        Err(e) => h.error = Some(e.to_string()),
    }
    h
}

/// Shared connections, one per server name.
#[derive(Default)]
pub struct McpHub {
    clients: tokio::sync::Mutex<HashMap<String, (ServerConfig, Arc<McpClient>)>>,
}

impl std::fmt::Debug for McpHub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("McpHub")
    }
}

/// The process's connections.
pub fn hub() -> &'static McpHub {
    static HUB: OnceLock<McpHub> = OnceLock::new();
    HUB.get_or_init(McpHub::default)
}

impl McpHub {
    /// The server's connection, started if it is not running or its
    /// config changed.
    pub async fn client(&self, name: &str, cfg: &ServerConfig) -> Result<Arc<McpClient>, McpError> {
        let stale = {
            let mut map = self.clients.lock().await;
            match map.get(name) {
                Some((c, client)) if same_launch(c, cfg) => return Ok(client.clone()),
                Some(_) => map.remove(name).map(|(_, c)| c),
                None => None,
            }
        };
        if let Some(c) = stale {
            c.shutdown().await;
        }
        // Started without holding the lock, so servers start in parallel.
        let client = Arc::new(
            tokio::time::timeout(Duration::from_secs(30), McpClient::connect(cfg))
                .await
                .map_err(|_| McpError::Timeout("the server to start"))??,
        );
        let mut map = self.clients.lock().await;
        if let Some((c, other)) = map.get(name)
            && same_launch(c, cfg)
        {
            let other = other.clone();
            drop(map);
            client.shutdown().await;
            return Ok(other);
        }
        map.insert(name.to_owned(), (cfg.clone(), client.clone()));
        Ok(client)
    }

    /// Stops a server's shared connection.
    pub async fn stop(&self, name: &str) {
        if let Some((_, c)) = self.clients.lock().await.remove(name) {
            c.shutdown().await;
        }
    }

    /// Stops every server.
    pub async fn stop_all(&self) {
        let all: Vec<_> = self.clients.lock().await.drain().collect();
        for (_, (_, c)) in all {
            c.shutdown().await;
        }
    }

    /// Names of servers running now.
    pub async fn running(&self) -> Vec<String> {
        self.clients.lock().await.keys().cloned().collect()
    }
}

fn same_launch(a: &ServerConfig, b: &ServerConfig) -> bool {
    a.command == b.command
        && a.args == b.args
        && a.env == b.env
        && a.url == b.url
        && a.headers == b.headers
        && a.cwd == b.cwd
}

/// What adding a config's tools to a chat did.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Attached {
    /// `(server, tool names as the model sees them)`.
    pub servers: Vec<(String, Vec<String>)>,
    /// Lines for the user: servers that would not start or need review.
    pub notes: Vec<String>,
}

/// Registers the tools of every active server, and each tool's approval
/// mode in the gate. Servers start in parallel; one that will not start,
/// or whose tools changed since the user trusted them, is left out with a
/// note.
pub async fn attach(cfg: &McpConfig, tools: &mut ToolRegistry, gate: &mut PolicyGate) -> Attached {
    let active: Vec<(&String, &ServerConfig)> =
        cfg.servers.iter().filter(|(_, s)| s.active()).collect();
    let lists = futures::future::join_all(active.iter().map(|(name, s)| async move {
        let client = hub().client(name, s).await?;
        let list = client.list_tools().await?;
        Ok::<_, McpError>((client, list))
    }))
    .await;
    let mut out = Attached::default();
    for ((name, s), r) in active.into_iter().zip(lists) {
        match r {
            Err(e) => out
                .notes
                .push(format!("MCP server “{name}” is not available: {e}")),
            Ok((client, list)) => {
                if s.tern.trusted.as_deref() != Some(trust::fingerprint(&list).as_str()) {
                    out.notes.push(format!(
                        "MCP server “{name}” changed its tools since you reviewed them, so they are off until you review them again (`tern mcp preview {name}`)."
                    ));
                    continue;
                }
                let mut names = Vec::new();
                for t in &list {
                    let setting = s.tool(&t.name);
                    if !setting.enabled {
                        continue;
                    }
                    let tool = McpTool::new(name, t, client.clone(), s.timeout());
                    gate.set_tool_mode(crate::tools::Tool::name(&tool), setting.approval);
                    names.push(crate::tools::Tool::name(&tool).to_owned());
                    tools.register(tool);
                }
                out.servers.push((name.to_string(), names));
            }
        }
    }
    out
}
