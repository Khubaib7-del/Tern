//! A small MCP client over JSON-RPC 2.0: stdio and Streamable HTTP.
//!
//! Tern speaks the current revision, `2026-07-28` (stateless: no
//! `initialize` handshake; every request carries its protocol version,
//! client info, and capabilities in `_meta`; `server/discover` describes the
//! server), and falls back to the `initialize` era (`2025-11-25` back to
//! `2024-11-05`) for servers that have not moved yet, as the spec's
//! backward-compatibility rules describe: `server/discover` is tried
//! first, and a reply that is not a modern JSON-RPC error means a legacy
//! server.
//!
//! Tern uses tools, prompts, and resources. It offers no client
//! capabilities (no roots, sampling, or elicitation), so a server request
//! for one is answered with "method not found", and a multi round-trip
//! result asking for input is reported as an error.
//!
//! List changes: a legacy server sends `notifications/*/list_changed` on
//! its own; a modern stdio server sends them on a `subscriptions/listen`
//! stream Tern opens when the server says its lists change. Either way the
//! cached catalog ([`McpClient::catalog`]) is read again on next use. Over
//! HTTP Tern opens no long-lived stream, so the catalog is simply re-read
//! after a short while.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{Value, json};
use tern_providers::rpc_http::{RpcHttp, RpcHttpError};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{Mutex, oneshot};

use super::config::{ServerConfig, Transport};
use crate::tools::BoxFuture;

/// The revision Tern implements.
pub const LATEST: &str = "2026-07-28";
/// Initialize-era revisions Tern accepts, newest first.
pub const LEGACY: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

const META_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
const META_CLIENT: &str = "io.modelcontextprotocol/clientInfo";
const META_CAPS: &str = "io.modelcontextprotocol/clientCapabilities";
const META_SERVER: &str = "io.modelcontextprotocol/serverInfo";

/// Error codes the 2026-07-28 revision reserves; a server that sends one
/// speaks the modern protocol.
const MODERN_CODES: [i64; 3] = [-32020, -32021, -32022];

/// What went wrong talking to a server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum McpError {
    /// The process would not start, or the address could not be reached.
    Start(String),
    /// The server did not answer in time.
    Timeout(&'static str),
    /// The server answered with a JSON-RPC error.
    Rpc { code: i64, message: String },
    /// The server's answer was not what MCP says it should be.
    Protocol(String),
    /// The server process exited or the connection closed.
    Closed(String),
}

impl std::fmt::Display for McpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Start(m) => write!(f, "could not start: {m}"),
            Self::Timeout(what) => write!(f, "timed out waiting for {what}"),
            Self::Rpc { code, message } => write!(f, "server error {code}: {message}"),
            Self::Protocol(m) => write!(f, "unexpected reply: {m}"),
            Self::Closed(m) => write!(f, "connection closed: {m}"),
        }
    }
}

/// Who the server says it is. Self-reported: for display only.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
    /// The protocol revision in use.
    pub protocol: String,
    /// The server's own guidance for models. Untrusted, like tool
    /// descriptions: shown in the trust preview, never put in the prompt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    /// The server offers prompts (`capabilities.prompts`).
    pub prompts: bool,
    /// The server offers resources (`capabilities.resources`).
    pub resources: bool,
    /// The server says its prompt list changes (`prompts.listChanged`).
    pub prompts_change: bool,
    /// The server says its resource list changes (`resources.listChanged`).
    pub resources_change: bool,
    /// The server says its tool list changes (`tools.listChanged`).
    pub tools_change: bool,
}

impl ServerInfo {
    fn read_caps(&mut self, caps: &Value) {
        self.prompts = caps.get("prompts").is_some_and(|v| !v.is_null());
        self.resources = caps.get("resources").is_some_and(|v| !v.is_null());
        self.prompts_change = caps["prompts"]["listChanged"].as_bool().unwrap_or(false);
        self.resources_change = caps["resources"]["listChanged"].as_bool().unwrap_or(false);
        self.tools_change = caps["tools"]["listChanged"].as_bool().unwrap_or(false);
    }
}

/// One argument a prompt takes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PromptArg {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub description: String,
    pub required: bool,
}

/// One prompt as the server lists it. Its text is the server's: shown to
/// the user, never sent on its own.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PromptInfo {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub description: String,
    pub arguments: Vec<PromptArg>,
}

/// A prompt filled in by the server (`prompts/get`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PromptText {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// `(role, text)` per message, in order.
    pub messages: Vec<(String, String)>,
}

impl PromptText {
    /// The text that goes into the composer: one user message as is;
    /// several messages each under its role.
    pub fn to_composer(&self) -> String {
        match self.messages.as_slice() {
            [] => String::new(),
            [(role, text)] if role == "user" => text.trim().to_owned(),
            all => all
                .iter()
                .map(|(r, t)| format!("[{r}]\n{}", t.trim()))
                .collect::<Vec<_>>()
                .join("\n\n"),
        }
    }
}

/// One resource as the server lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ResourceInfo {
    pub uri: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
}

/// A resource template (`file:///{path}`): an address with parts the
/// user fills in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TemplateInfo {
    pub uri_template: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime: Option<String>,
}

/// One part of a read resource.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResourcePart {
    Text {
        uri: String,
        mime: Option<String>,
        text: String,
    },
    Blob {
        uri: String,
        mime: Option<String>,
        bytes: Vec<u8>,
    },
}

/// Everything a server offers: tools, prompts, resources, templates.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Catalog {
    pub tools: Vec<ToolInfo>,
    pub prompts: Vec<PromptInfo>,
    pub resources: Vec<ResourceInfo>,
    pub templates: Vec<TemplateInfo>,
}

/// Counters bumped by list-change notifications, so cached lists know
/// they are stale.
#[derive(Debug, Default)]
pub struct Changes {
    tools: AtomicU64,
    prompts: AtomicU64,
    resources: AtomicU64,
    /// URIs a server said changed (`notifications/resources/updated`).
    updated: std::sync::Mutex<Vec<String>>,
}

impl Changes {
    fn note(&self, method: &str, params: &Value) {
        match method {
            "notifications/tools/list_changed" => {
                self.tools.fetch_add(1, Ordering::Relaxed);
            }
            "notifications/prompts/list_changed" => {
                self.prompts.fetch_add(1, Ordering::Relaxed);
            }
            "notifications/resources/list_changed" => {
                self.resources.fetch_add(1, Ordering::Relaxed);
            }
            "notifications/resources/updated" => {
                if let Some(u) = params["uri"].as_str() {
                    let mut v = self.updated.lock().expect("updated");
                    if !v.iter().any(|x| x == u) {
                        v.push(u.to_owned());
                    }
                    if v.len() > 100 {
                        v.remove(0);
                    }
                }
            }
            _ => {}
        }
    }

    /// One number that moves whenever any list changes.
    pub fn generation(&self) -> u64 {
        self.tools.load(Ordering::Relaxed)
            + self.prompts.load(Ordering::Relaxed)
            + self.resources.load(Ordering::Relaxed)
    }
}

/// One tool as the server lists it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ToolInfo {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub description: String,
    pub input_schema: Value,
    /// Hints the server gives (`readOnlyHint`, `destructiveHint`, …).
    /// Untrusted: they only make Tern more careful, never less.
    pub read_only: bool,
    pub destructive: bool,
}

/// A tool call's result as text, with whether the server flagged it as
/// an error.
#[derive(Clone, Debug, PartialEq)]
pub struct CallResult {
    pub text: String,
    pub is_error: bool,
}

/// Moves JSON-RPC messages.
trait Wire: Send + Sync {
    /// Sends a request and waits for the response object.
    fn request<'a>(
        &'a self,
        msg: Value,
        timeout: Duration,
    ) -> BoxFuture<'a, Result<Value, McpError>>;
    fn notify<'a>(&'a self, msg: Value) -> BoxFuture<'a, Result<(), McpError>>;
    /// Uses a protocol version for later HTTP headers.
    fn set_version(&self, _v: &str) {}
    fn close<'a>(&'a self) -> BoxFuture<'a, ()>;
}

type Pending = Arc<std::sync::Mutex<HashMap<String, oneshot::Sender<Value>>>>;

/// A server started as a child process, speaking newline-delimited JSON.
struct StdioWire {
    child: Mutex<Option<tokio::process::Child>>,
    stdin: Arc<Mutex<tokio::process::ChildStdin>>,
    pending: Pending,
    /// The last lines the server wrote to stderr, for error messages.
    stderr: Arc<std::sync::Mutex<Vec<String>>>,
}

impl StdioWire {
    fn spawn(cfg: &ServerConfig, changes: Arc<Changes>) -> Result<Self, McpError> {
        let command = cfg
            .command
            .as_deref()
            .filter(|c| !c.trim().is_empty())
            .ok_or_else(|| McpError::Start("no command".into()))?;
        let mut cmd = tokio::process::Command::new(expand_home(command));
        cmd.args(cfg.args.iter().map(|a| expand_home(a)))
            .envs(&cfg.env)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        if let Some(dir) = &cfg.cwd {
            cmd.current_dir(expand_home(dir));
        }
        #[cfg(windows)]
        {
            // No console window flashes up for the server.
            cmd.creation_flags(0x0800_0000);
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| McpError::Start(format!("{command}: {e}")))?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr_pipe = child.stderr.take().expect("piped stderr");
        let stdin = Arc::new(Mutex::new(stdin));
        let pending: Pending = Arc::default();
        let stderr: Arc<std::sync::Mutex<Vec<String>>> = Arc::default();
        {
            let pending = pending.clone();
            let stdin = stdin.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    // Servers sometimes print logs to stdout; skip what is
                    // not JSON.
                    let Ok(msg) = serde_json::from_str::<Value>(line.trim()) else {
                        continue;
                    };
                    route(&msg, &pending, &changes, |reply| {
                        let stdin = stdin.clone();
                        tokio::spawn(async move {
                            let mut w = stdin.lock().await;
                            let _ = w.write_all(format!("{reply}\n").as_bytes()).await;
                            let _ = w.flush().await;
                        });
                    });
                }
                // Closed: every waiting request fails.
                pending.lock().expect("pending").clear();
            });
        }
        {
            let stderr = stderr.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr_pipe).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let mut v = stderr.lock().expect("stderr");
                    v.push(line.chars().take(400).collect());
                    if v.len() > 20 {
                        v.remove(0);
                    }
                }
            });
        }
        Ok(Self {
            child: Mutex::new(Some(child)),
            stdin,
            pending,
            stderr,
        })
    }

    fn last_stderr(&self) -> String {
        let v = self.stderr.lock().expect("stderr");
        v.iter()
            .rev()
            .find(|l| !l.trim().is_empty())
            .cloned()
            .unwrap_or_default()
    }
}

/// Hands a response to its waiting request, or answers a server request
/// Tern does not support.
fn route(msg: &Value, pending: &Pending, changes: &Changes, reply: impl Fn(Value)) {
    let id = msg.get("id").filter(|v| !v.is_null());
    let is_response = msg.get("result").is_some() || msg.get("error").is_some();
    match (id, is_response, msg.get("method")) {
        (Some(id), true, _) => {
            if let Some(tx) = pending.lock().expect("pending").remove(&id.to_string()) {
                let _ = tx.send(msg.clone());
            }
        }
        (Some(id), false, Some(_)) => reply(json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {"code": -32601, "message": "Tern does not offer this to MCP servers"}
        })),
        // List changes mark the cached catalog stale; progress and logs
        // are not used.
        (None, false, Some(m)) => changes.note(m.as_str().unwrap_or_default(), &msg["params"]),
        _ => {}
    }
}

impl Wire for StdioWire {
    fn request<'a>(
        &'a self,
        msg: Value,
        timeout: Duration,
    ) -> BoxFuture<'a, Result<Value, McpError>> {
        Box::pin(async move {
            let id = msg["id"].to_string();
            let (tx, rx) = oneshot::channel();
            self.pending.lock().expect("pending").insert(id.clone(), tx);
            {
                let mut w = self.stdin.lock().await;
                let line = format!("{msg}\n");
                if let Err(e) = w.write_all(line.as_bytes()).await {
                    self.pending.lock().expect("pending").remove(&id);
                    return Err(McpError::Closed(format!("{e}. {}", self.last_stderr())));
                }
                let _ = w.flush().await;
            }
            match tokio::time::timeout(timeout, rx).await {
                Ok(Ok(v)) => Ok(v),
                Ok(Err(_)) => {
                    let why = self.last_stderr();
                    Err(McpError::Closed(if why.is_empty() {
                        "the server exited".into()
                    } else {
                        format!("the server exited: {why}")
                    }))
                }
                Err(_) => {
                    self.pending.lock().expect("pending").remove(&id);
                    // Tell the server to stop working on it.
                    let _ = self
                        .notify(
                            json!({"jsonrpc": "2.0", "method": "notifications/cancelled",
                                        "params": {"requestId": msg["id"], "reason": "timed out"}}),
                        )
                        .await;
                    Err(McpError::Timeout("the server"))
                }
            }
        })
    }

    fn notify<'a>(&'a self, msg: Value) -> BoxFuture<'a, Result<(), McpError>> {
        Box::pin(async move {
            let mut w = self.stdin.lock().await;
            w.write_all(format!("{msg}\n").as_bytes())
                .await
                .map_err(|e| McpError::Closed(e.to_string()))?;
            let _ = w.flush().await;
            Ok(())
        })
    }

    fn close<'a>(&'a self) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            if let Some(mut c) = self.child.lock().await.take() {
                let _ = c.start_kill();
                let _ = tokio::time::timeout(Duration::from_secs(2), c.wait()).await;
            }
        })
    }
}

/// A server reached over Streamable HTTP: one POST per message. The
/// network itself lives in `tern-providers`.
struct HttpWire {
    http: RpcHttp,
    headers: Vec<(String, String)>,
    /// The protocol version for the `MCP-Protocol-Version` header.
    version: std::sync::Mutex<Option<String>>,
    /// The legacy era's session, when the server assigned one.
    session: std::sync::Mutex<Option<String>>,
    changes: Arc<Changes>,
}

impl HttpWire {
    fn new(cfg: &ServerConfig, changes: Arc<Changes>) -> Result<Self, McpError> {
        let url = cfg
            .url
            .clone()
            .filter(|u| u.starts_with("http://") || u.starts_with("https://"))
            .ok_or_else(|| McpError::Start("an http server needs an http(s) url".into()))?;
        let http = RpcHttp::new(&url).map_err(|e| McpError::Start(e.to_string()))?;
        Ok(Self {
            http,
            headers: cfg
                .headers
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            version: std::sync::Mutex::new(Some(LATEST.into())),
            session: std::sync::Mutex::new(None),
            changes,
        })
    }

    async fn post(&self, msg: &Value, timeout: Duration) -> Result<Option<Value>, McpError> {
        let method = msg["method"].as_str().unwrap_or_default();
        let mut headers: Vec<(String, String)> = vec![
            (
                "Accept".into(),
                "application/json, text/event-stream".into(),
            ),
            ("Mcp-Method".into(), header_value(method)),
        ];
        if let Some(v) = self.version.lock().expect("version").clone() {
            headers.push(("MCP-Protocol-Version".into(), v));
        }
        if let Some(name) = msg["params"]["name"]
            .as_str()
            .or_else(|| msg["params"]["uri"].as_str())
        {
            headers.push(("Mcp-Name".into(), header_value(name)));
        }
        if let Some(s) = self.session.lock().expect("session").clone() {
            headers.push(("Mcp-Session-Id".into(), s));
        }
        headers.extend(self.headers.iter().cloned());
        let resp = self
            .http
            .post(&headers, msg, timeout)
            .await
            .map_err(|e| match e {
                RpcHttpError::Timeout => McpError::Timeout("the server"),
                RpcHttpError::Connect(e) => McpError::Start(e),
                RpcHttpError::Read(e) => McpError::Closed(e),
            })?;
        if let Some(s) = &resp.session {
            *self.session.lock().expect("session") = Some(s.clone());
        }
        let status = resp.status;
        let ok = resp.is_success();
        let sse = resp
            .content_type
            .as_deref()
            .is_some_and(|c| c.starts_with("text/event-stream"));
        let body = resp.body;
        if msg.get("id").is_none() {
            return if ok {
                Ok(None)
            } else {
                Err(McpError::Protocol(format!(
                    "notification refused (HTTP {status})"
                )))
            };
        }
        let id = &msg["id"];
        let found = if sse {
            let all = sse_messages(&body);
            for m in all.iter().filter(|m| m.get("id").is_none()) {
                self.changes
                    .note(m["method"].as_str().unwrap_or_default(), &m["params"]);
            }
            all.into_iter()
                .find(|m| &m["id"] == id && (m.get("result").is_some() || m.get("error").is_some()))
        } else {
            serde_json::from_str::<Value>(&body).ok()
        };
        match found {
            // Modern servers answer errors with 4xx and a JSON-RPC body.
            Some(v) if v.get("result").is_some() || v.get("error").is_some() => Ok(Some(v)),
            _ if !ok => Err(McpError::Protocol(format!(
                "HTTP {status}{}",
                if body.trim().is_empty() {
                    String::new()
                } else {
                    format!(": {}", body.chars().take(200).collect::<String>())
                }
            ))),
            _ => Err(McpError::Protocol(
                "no JSON-RPC response in the reply".into(),
            )),
        }
    }
}

impl Wire for HttpWire {
    fn request<'a>(
        &'a self,
        msg: Value,
        timeout: Duration,
    ) -> BoxFuture<'a, Result<Value, McpError>> {
        Box::pin(async move {
            self.post(&msg, timeout)
                .await?
                .ok_or_else(|| McpError::Protocol("empty reply".into()))
        })
    }

    fn notify<'a>(&'a self, msg: Value) -> BoxFuture<'a, Result<(), McpError>> {
        Box::pin(async move { self.post(&msg, Duration::from_secs(10)).await.map(|_| ()) })
    }

    fn set_version(&self, v: &str) {
        *self.version.lock().expect("version") = Some(v.to_owned());
    }

    fn close<'a>(&'a self) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            // A legacy session is ended with DELETE; modern servers have none.
            let session = self.session.lock().expect("session").clone();
            if let Some(s) = session {
                self.http
                    .delete(&[("Mcp-Session-Id".into(), s)], Duration::from_secs(3))
                    .await;
            }
        })
    }
}

/// The JSON messages in an SSE body (`data:` lines, joined per event).
pub(crate) fn sse_messages(body: &str) -> Vec<Value> {
    let mut out = Vec::new();
    let mut data = String::new();
    for line in body.lines().chain(std::iter::once("")) {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            if !data.is_empty() {
                if let Ok(v) = serde_json::from_str(&data) {
                    out.push(v);
                }
                data.clear();
            }
        } else if let Some(d) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(d.strip_prefix(' ').unwrap_or(d));
        }
    }
    out
}

/// A header-safe value, or the spec's Base64 sentinel form.
pub(crate) fn header_value(v: &str) -> String {
    use base64::Engine as _;
    let plain = v.bytes().all(|b| (0x20..=0x7e).contains(&b))
        && v.trim() == v
        && !(v.starts_with("=?base64?") && v.ends_with("?="));
    if plain {
        v.to_owned()
    } else {
        format!(
            "=?base64?{}?=",
            base64::engine::general_purpose::STANDARD.encode(v.as_bytes())
        )
    }
}

fn expand_home(s: &str) -> String {
    if let Some(rest) = s.strip_prefix("~/").or_else(|| s.strip_prefix("~\\")) {
        if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
            return std::path::Path::new(&home)
                .join(rest)
                .to_string_lossy()
                .into_owned();
        }
    }
    s.to_owned()
}

/// Which generation of MCP a server speaks.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Era {
    /// Stateless, with `_meta` on every request.
    Modern,
    /// After an `initialize` handshake, at this version.
    Legacy(String),
}

/// One connected MCP server.
pub struct McpClient {
    wire: Box<dyn Wire>,
    era: Era,
    info: ServerInfo,
    next: AtomicU64,
    timeout: Duration,
    /// When the connection was made, for health checks.
    pub started: Duration,
    changes: Arc<Changes>,
    /// List changes reach Tern (stdio), so the catalog cache can be kept
    /// until one arrives.
    pushed: bool,
    cache: std::sync::Mutex<Option<(u64, Instant, Catalog)>>,
}

impl std::fmt::Debug for McpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpClient")
            .field("server", &self.info.name)
            .field("protocol", &self.info.protocol)
            .finish()
    }
}

fn client_info() -> Value {
    json!({"name": "tern", "title": "Tern", "version": env!("CARGO_PKG_VERSION")})
}

fn modern_meta() -> Value {
    json!({
        META_VERSION: LATEST,
        META_CLIENT: client_info(),
        META_CAPS: {},
    })
}

/// The `result`, or the JSON-RPC error as [`McpError::Rpc`].
fn result_of(resp: Value) -> Result<Value, McpError> {
    if let Some(e) = resp.get("error") {
        return Err(McpError::Rpc {
            code: e["code"].as_i64().unwrap_or(0),
            message: e["message"].as_str().unwrap_or("error").to_owned(),
        });
    }
    let r = resp.get("result").cloned().unwrap_or(Value::Null);
    if r["resultType"] == "input_required" {
        return Err(McpError::Protocol(
            "the server asked for more input (sampling, elicitation, or roots), which Tern does not offer".into(),
        ));
    }
    Ok(r)
}

/// A prompt message's content block as text.
fn content_text(c: &Value) -> Option<String> {
    match c["type"].as_str()? {
        "text" => c["text"].as_str().map(str::to_owned),
        "resource" => {
            let res = &c["resource"];
            let uri = res["uri"].as_str().unwrap_or("");
            Some(match res["text"].as_str() {
                Some(t) => format!("[resource {uri}]\n{t}"),
                None => format!("[binary resource {uri} not inserted]"),
            })
        }
        "resource_link" => Some(format!(
            "[link] {} {}",
            c["name"].as_str().unwrap_or(""),
            c["uri"].as_str().unwrap_or("")
        )),
        kind @ ("image" | "audio") => Some(format!("[{kind} in the prompt; not inserted]")),
        _ => None,
    }
}

impl McpClient {
    /// Starts or reaches the server and agrees on a protocol version.
    pub async fn connect(cfg: &ServerConfig) -> Result<Self, McpError> {
        let t0 = Instant::now();
        let changes = Arc::new(Changes::default());
        let transport = cfg.transport().map_err(McpError::Start)?;
        let wire: Box<dyn Wire> = match transport {
            Transport::Stdio => Box::new(StdioWire::spawn(cfg, changes.clone())?),
            Transport::Http => Box::new(HttpWire::new(cfg, changes.clone())?),
        };
        let mut c = Self {
            wire,
            era: Era::Modern,
            info: ServerInfo::default(),
            next: AtomicU64::new(1),
            timeout: cfg.timeout(),
            started: Duration::ZERO,
            changes,
            pushed: transport == Transport::Stdio,
            cache: std::sync::Mutex::new(None),
        };
        match c.handshake().await {
            Ok(()) => {
                c.started = t0.elapsed();
                c.listen().await;
                Ok(c)
            }
            Err(e) => {
                c.wire.close().await;
                Err(e)
            }
        }
    }

    async fn handshake(&mut self) -> Result<(), McpError> {
        // Modern first: `server/discover`. A legacy stdio server may not
        // answer an unknown method before `initialize`, so wait briefly.
        let probe = self
            .raw(
                "server/discover",
                json!({"_meta": modern_meta()}),
                Duration::from_secs(5),
            )
            .await;
        let fallback = match probe {
            Ok(resp) => match result_of(resp) {
                Ok(r) => {
                    let versions: Vec<&str> = r["supportedVersions"]
                        .as_array()
                        .map(|a| a.iter().filter_map(Value::as_str).collect())
                        .unwrap_or_default();
                    if versions.contains(&LATEST) {
                        self.era = Era::Modern;
                        let si = &r["_meta"][META_SERVER];
                        self.info = ServerInfo {
                            name: si["name"].as_str().unwrap_or_default().to_owned(),
                            version: si["version"].as_str().unwrap_or_default().to_owned(),
                            protocol: LATEST.into(),
                            instructions: r["instructions"].as_str().map(str::to_owned),
                            ..Default::default()
                        };
                        self.info.read_caps(&r["capabilities"]);
                        return Ok(());
                    }
                    if versions.iter().any(|v| LEGACY.contains(v)) {
                        true
                    } else {
                        return Err(McpError::Protocol(format!(
                            "the server supports {} and Tern supports {LATEST} and {}",
                            versions.join(", "),
                            LEGACY.join(", ")
                        )));
                    }
                }
                Err(McpError::Rpc { code, message }) if MODERN_CODES.contains(&code) => {
                    return Err(McpError::Rpc { code, message });
                }
                Err(_) => true,
            },
            Err(McpError::Timeout(_)) | Err(McpError::Protocol(_)) | Err(McpError::Rpc { .. }) => {
                true
            }
            Err(e) => return Err(e),
        };
        if fallback {
            self.legacy_init().await?;
        }
        Ok(())
    }

    async fn legacy_init(&mut self) -> Result<(), McpError> {
        self.wire.set_version(LEGACY[0]);
        let r = result_of(
            self.raw(
                "initialize",
                json!({
                    "protocolVersion": LEGACY[0],
                    "capabilities": {},
                    "clientInfo": client_info(),
                }),
                self.timeout.min(Duration::from_secs(30)),
            )
            .await?,
        )?;
        let v = r["protocolVersion"].as_str().unwrap_or_default().to_owned();
        if !LEGACY.contains(&v.as_str()) {
            return Err(McpError::Protocol(format!(
                "the server wants protocol {v}, which Tern does not speak"
            )));
        }
        self.wire.set_version(&v);
        self.era = Era::Legacy(v.clone());
        self.info = ServerInfo {
            name: r["serverInfo"]["name"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            version: r["serverInfo"]["version"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            protocol: v,
            instructions: r["instructions"].as_str().map(str::to_owned),
            ..Default::default()
        };
        self.info.read_caps(&r["capabilities"]);
        self.wire
            .notify(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
            .await
    }

    async fn raw(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, McpError> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        self.wire
            .request(
                json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}),
                timeout,
            )
            .await
    }

    /// One request in the agreed protocol, its `result` or the error.
    async fn rpc(&self, method: &str, mut params: Value) -> Result<Value, McpError> {
        if self.era == Era::Modern {
            if !params.is_object() {
                params = json!({});
            }
            params["_meta"] = modern_meta();
        }
        result_of(self.raw(method, params, self.timeout).await?)
    }

    pub fn info(&self) -> &ServerInfo {
        &self.info
    }

    /// Every item of a list method, following `nextCursor` (at most 50
    /// pages).
    async fn list_all(&self, method: &str, key: &str) -> Result<Vec<Value>, McpError> {
        let mut out = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..50 {
            let params = match &cursor {
                Some(c) => json!({"cursor": c}),
                None => json!({}),
            };
            let r = self.rpc(method, params).await?;
            let items = r[key]
                .as_array()
                .ok_or_else(|| McpError::Protocol(format!("{method} has no `{key}`")))?;
            out.extend(items.iter().cloned());
            cursor = r["nextCursor"].as_str().map(str::to_owned);
            if cursor.is_none() {
                break;
            }
        }
        Ok(out)
    }

    /// Every tool.
    pub async fn list_tools(&self) -> Result<Vec<ToolInfo>, McpError> {
        Ok(self
            .list_all("tools/list", "tools")
            .await?
            .iter()
            .filter_map(|t| {
                let name = t["name"].as_str()?;
                let ann = &t["annotations"];
                Some(ToolInfo {
                    name: name.to_owned(),
                    title: t["title"]
                        .as_str()
                        .or_else(|| ann["title"].as_str())
                        .map(str::to_owned),
                    description: t["description"].as_str().unwrap_or_default().to_owned(),
                    input_schema: t
                        .get("inputSchema")
                        .cloned()
                        .unwrap_or_else(|| json!({"type": "object"})),
                    read_only: ann["readOnlyHint"].as_bool().unwrap_or(false),
                    destructive: ann["destructiveHint"].as_bool().unwrap_or(false),
                })
            })
            .collect())
    }

    /// A list the server may not offer: empty when it did not declare the
    /// capability or answers "method not found".
    async fn optional_list(
        &self,
        offered: bool,
        method: &str,
        key: &str,
    ) -> Result<Vec<Value>, McpError> {
        if !offered {
            return Ok(Vec::new());
        }
        match self.list_all(method, key).await {
            Err(McpError::Rpc { code: -32601, .. }) => Ok(Vec::new()),
            r => r,
        }
    }

    /// Every prompt (none when the server offers no prompts).
    pub async fn list_prompts(&self) -> Result<Vec<PromptInfo>, McpError> {
        Ok(self
            .optional_list(self.info.prompts, "prompts/list", "prompts")
            .await?
            .iter()
            .filter_map(|p| {
                Some(PromptInfo {
                    name: p["name"].as_str()?.to_owned(),
                    title: p["title"].as_str().map(str::to_owned),
                    description: p["description"].as_str().unwrap_or_default().to_owned(),
                    arguments: p["arguments"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|a| {
                            Some(PromptArg {
                                name: a["name"].as_str()?.to_owned(),
                                title: a["title"].as_str().map(str::to_owned),
                                description: a["description"]
                                    .as_str()
                                    .unwrap_or_default()
                                    .to_owned(),
                                required: a["required"].as_bool().unwrap_or(false),
                            })
                        })
                        .collect(),
                })
            })
            .collect())
    }

    /// Every resource (none when the server offers no resources).
    pub async fn list_resources(&self) -> Result<Vec<ResourceInfo>, McpError> {
        Ok(self
            .optional_list(self.info.resources, "resources/list", "resources")
            .await?
            .iter()
            .filter_map(|r| {
                let uri = r["uri"].as_str()?.to_owned();
                Some(ResourceInfo {
                    name: r["name"].as_str().unwrap_or(&uri).to_owned(),
                    title: r["title"].as_str().map(str::to_owned),
                    description: r["description"].as_str().unwrap_or_default().to_owned(),
                    mime: r["mimeType"].as_str().map(str::to_owned),
                    size: r["size"].as_u64(),
                    uri,
                })
            })
            .collect())
    }

    /// Every resource template.
    pub async fn list_templates(&self) -> Result<Vec<TemplateInfo>, McpError> {
        Ok(self
            .optional_list(
                self.info.resources,
                "resources/templates/list",
                "resourceTemplates",
            )
            .await?
            .iter()
            .filter_map(|t| {
                let tpl = t["uriTemplate"].as_str()?.to_owned();
                Some(TemplateInfo {
                    name: t["name"].as_str().unwrap_or(&tpl).to_owned(),
                    title: t["title"].as_str().map(str::to_owned),
                    description: t["description"].as_str().unwrap_or_default().to_owned(),
                    mime: t["mimeType"].as_str().map(str::to_owned),
                    uri_template: tpl,
                })
            })
            .collect())
    }

    /// Tools, prompts, resources, and templates, from the cache while no
    /// list change has arrived (stdio) or for 30 seconds (HTTP).
    pub async fn catalog(&self) -> Result<Catalog, McpError> {
        let generation = self.changes.generation();
        let ttl = if self.pushed {
            Duration::from_secs(600)
        } else {
            Duration::from_secs(30)
        };
        if let Some((g, at, c)) = self.cache.lock().expect("cache").as_ref()
            && *g == generation
            && at.elapsed() < ttl
        {
            return Ok(c.clone());
        }
        let (tools, prompts, resources, templates) = futures::join!(
            self.list_tools(),
            self.list_prompts(),
            self.list_resources(),
            self.list_templates()
        );
        let c = Catalog {
            tools: tools?,
            prompts: prompts?,
            resources: resources?,
            templates: templates?,
        };
        *self.cache.lock().expect("cache") = Some((generation, Instant::now(), c.clone()));
        Ok(c)
    }

    /// The counters list changes bump, and the URIs reported updated.
    pub fn changes(&self) -> &Changes {
        &self.changes
    }

    /// URIs the server said changed since the last call.
    pub fn take_updated(&self) -> Vec<String> {
        std::mem::take(&mut *self.changes.updated.lock().expect("updated"))
    }

    /// Fills in a prompt. Text and embedded text resources come back as
    /// text; pictures and audio are named, not passed on.
    pub async fn get_prompt(
        &self,
        name: &str,
        args: &std::collections::BTreeMap<String, String>,
    ) -> Result<PromptText, McpError> {
        let r = self
            .rpc("prompts/get", json!({"name": name, "arguments": args}))
            .await?;
        let messages = r["messages"]
            .as_array()
            .ok_or_else(|| McpError::Protocol("prompts/get has no `messages`".into()))?
            .iter()
            .map(|m| {
                let role = m["role"].as_str().unwrap_or("user").to_owned();
                // One content block per message; some servers send a list.
                let blocks: Vec<&Value> = match &m["content"] {
                    Value::Array(a) => a.iter().collect(),
                    v => vec![v],
                };
                let text = blocks
                    .into_iter()
                    .filter_map(content_text)
                    .collect::<Vec<_>>()
                    .join("\n\n");
                (role, text)
            })
            .collect();
        Ok(PromptText {
            description: r["description"].as_str().map(str::to_owned),
            messages,
        })
    }

    /// Reads a resource: text parts as text, binary parts decoded.
    pub async fn read_resource(&self, uri: &str) -> Result<Vec<ResourcePart>, McpError> {
        use base64::Engine as _;
        let r = self.rpc("resources/read", json!({"uri": uri})).await?;
        let contents = r["contents"]
            .as_array()
            .ok_or_else(|| McpError::Protocol("resources/read has no `contents`".into()))?;
        let mut out = Vec::new();
        for c in contents {
            let uri = c["uri"].as_str().unwrap_or(uri).to_owned();
            let mime = c["mimeType"].as_str().map(str::to_owned);
            if let Some(t) = c["text"].as_str() {
                out.push(ResourcePart::Text {
                    uri,
                    mime,
                    text: t.to_owned(),
                });
            } else if let Some(b) = c["blob"].as_str() {
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(b.trim())
                    .map_err(|e| McpError::Protocol(format!("a blob is not base64: {e}")))?;
                out.push(ResourcePart::Blob { uri, mime, bytes });
            }
        }
        Ok(out)
    }

    /// Opens the modern era's change stream on stdio, for the lists the
    /// server says change. Its notifications arrive like any other; the
    /// request itself is never answered while the stream is open.
    async fn listen(&self) {
        if self.era != Era::Modern || !self.pushed {
            return;
        }
        let i = &self.info;
        if !(i.tools_change || i.prompts_change || i.resources_change) {
            return;
        }
        let id = format!("listen-{}", self.next.fetch_add(1, Ordering::Relaxed));
        let _ = self
            .wire
            .notify(json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "subscriptions/listen",
                "params": {
                    "_meta": modern_meta(),
                    "notifications": {
                        "toolsListChanged": i.tools_change,
                        "promptsListChanged": i.prompts_change,
                        "resourcesListChanged": i.resources_change,
                    }
                }
            }))
            .await;
    }

    /// Calls a tool. Text, resource text, and structured content come back
    /// as text; pictures and audio are named, not passed on.
    pub async fn call_tool(&self, name: &str, args: Value) -> Result<CallResult, McpError> {
        let r = self
            .rpc("tools/call", json!({"name": name, "arguments": args}))
            .await?;
        let mut parts = Vec::new();
        for c in r["content"].as_array().into_iter().flatten() {
            match c["type"].as_str() {
                Some("text") => parts.push(c["text"].as_str().unwrap_or_default().to_owned()),
                Some("resource") => {
                    let res = &c["resource"];
                    match res["text"].as_str() {
                        Some(t) => parts.push(format!(
                            "[resource {}]\n{t}",
                            res["uri"].as_str().unwrap_or("")
                        )),
                        None => parts.push(format!(
                            "[binary resource {} not shown]",
                            res["uri"].as_str().unwrap_or("")
                        )),
                    }
                }
                Some("resource_link") => parts.push(format!(
                    "[link] {} {}",
                    c["name"].as_str().unwrap_or(""),
                    c["uri"].as_str().unwrap_or("")
                )),
                Some(kind @ ("image" | "audio")) => {
                    parts.push(format!("[{kind} returned; not shown to the model]"))
                }
                _ => {}
            }
        }
        if parts.is_empty()
            && let Some(s) = r.get("structuredContent").filter(|v| !v.is_null())
        {
            parts.push(s.to_string());
        }
        Ok(CallResult {
            text: parts.join("\n\n"),
            is_error: r["isError"].as_bool().unwrap_or(false),
        })
    }

    /// A liveness round trip: `server/discover` on modern servers (which
    /// dropped `ping`), `ping` on legacy ones. Returns how long it took.
    pub async fn ping(&self) -> Result<Duration, McpError> {
        let t0 = Instant::now();
        match self.era {
            Era::Modern => self.rpc("server/discover", json!({})).await?,
            Era::Legacy(_) => self.rpc("ping", json!({})).await?,
        };
        Ok(t0.elapsed())
    }

    /// Stops the process or ends the session.
    pub async fn shutdown(&self) {
        self.wire.close().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_sse_bodies() {
        let body = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\"}\n\n: keep-alive\n\ndata: {\"jsonrpc\":\"2.0\",\"id\":3,\ndata: \"result\":{}}\n\n";
        let m = sse_messages(body);
        assert_eq!(m.len(), 2);
        assert_eq!(m[1]["id"], 3);
    }

    #[test]
    fn header_values_follow_the_base64_rule() {
        assert_eq!(header_value("get_weather"), "get_weather");
        assert_eq!(
            header_value("Hello, 世界"),
            "=?base64?SGVsbG8sIOS4lueVjA==?="
        );
        assert_eq!(header_value(" padded "), "=?base64?IHBhZGRlZCA=?=");
        assert_eq!(
            header_value("=?base64?literal?="),
            "=?base64?PT9iYXNlNjQ/bGl0ZXJhbD89?="
        );
    }

    #[test]
    fn input_required_results_are_errors() {
        let r = result_of(json!({"result": {"resultType": "input_required"}}));
        assert!(matches!(r, Err(McpError::Protocol(_))));
        let e = result_of(json!({"error": {"code": -32601, "message": "nope"}}));
        assert_eq!(
            e,
            Err(McpError::Rpc {
                code: -32601,
                message: "nope".into()
            })
        );
    }
}
