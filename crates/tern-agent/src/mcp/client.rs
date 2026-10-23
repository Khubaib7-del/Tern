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
//! Tern uses tools only. It offers no client capabilities (no roots,
//! sampling, or elicitation), so a server request for one is answered
//! with "method not found", and a multi round-trip result asking for
//! input is reported as an error.

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
    fn spawn(cfg: &ServerConfig) -> Result<Self, McpError> {
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
                    route(&msg, &pending, |reply| {
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
fn route(msg: &Value, pending: &Pending, reply: impl Fn(Value)) {
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
        // Notifications (progress, logs, list changes) are not used yet.
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
}

impl HttpWire {
    fn new(cfg: &ServerConfig) -> Result<Self, McpError> {
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
            sse_messages(&body)
                .into_iter()
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

impl McpClient {
    /// Starts or reaches the server and agrees on a protocol version.
    pub async fn connect(cfg: &ServerConfig) -> Result<Self, McpError> {
        let t0 = Instant::now();
        let wire: Box<dyn Wire> = match cfg.transport().map_err(McpError::Start)? {
            Transport::Stdio => Box::new(StdioWire::spawn(cfg)?),
            Transport::Http => Box::new(HttpWire::new(cfg)?),
        };
        let mut c = Self {
            wire,
            era: Era::Modern,
            info: ServerInfo::default(),
            next: AtomicU64::new(1),
            timeout: cfg.timeout(),
            started: Duration::ZERO,
        };
        match c.handshake().await {
            Ok(()) => {
                c.started = t0.elapsed();
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
                        };
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
        };
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

    /// Every tool, following `nextCursor` (at most 50 pages).
    pub async fn list_tools(&self) -> Result<Vec<ToolInfo>, McpError> {
        let mut out = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..50 {
            let params = match &cursor {
                Some(c) => json!({"cursor": c}),
                None => json!({}),
            };
            let r = self.rpc("tools/list", params).await?;
            let tools = r["tools"]
                .as_array()
                .ok_or_else(|| McpError::Protocol("tools/list has no `tools`".into()))?;
            for t in tools {
                let Some(name) = t["name"].as_str() else {
                    continue;
                };
                let ann = &t["annotations"];
                out.push(ToolInfo {
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
                });
            }
            cursor = r["nextCursor"].as_str().map(str::to_owned);
            if cursor.is_none() {
                break;
            }
        }
        Ok(out)
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
