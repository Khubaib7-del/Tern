//! The ACP server: `tern acp` runs Tern as an agent inside editors that
//! speak the Agent Client Protocol (Zed, JetBrains, and others).
//!
//! ACP v1 (protocol version `1`) over stdio: newline-delimited JSON-RPC
//! 2.0, the editor on the other end. Tern answers `initialize`,
//! `authenticate` (nothing to do: keys live in the OS keychain),
//! `session/new` (the editor's working folder becomes the project root,
//! so its `TERN.md` and `.tern/skills` apply), `session/load` (a past
//! Tern chat, replayed as `session/update` messages), `session/prompt`
//! (streamed back as `agent_message_chunk` updates, with `tool_call` and
//! `tool_call_update` for each step), and the `session/cancel`
//! notification. When the policy gate asks, Tern sends
//! `session/request_permission` to the editor and maps its answer to the
//! gate's: allow once, allow for this session, or reject.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tern_core::{EffectClass, EventEnvelope, SessionId, TurnState};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};

use crate::engine::{ApprovalRequest, Approver, CancelFlag};
use crate::policy::ApprovalAnswer;
use crate::tools::BoxFuture;
use crate::{ChatOptions, ChatSession, Store};

/// The ACP major version Tern speaks.
pub const PROTOCOL_VERSION: u64 = 1;

/// How `tern acp` opens chats.
#[derive(Clone, Debug, Default)]
pub struct AcpOptions {
    /// Provider, model, search, and budget for every session.
    pub chat: ChatOptions,
    /// The database; `None` is the app's own.
    pub db: Option<PathBuf>,
}

/// Messages to and from the editor.
struct Peer {
    out: mpsc::UnboundedSender<Value>,
    next: AtomicU64,
    pending: Mutex<HashMap<String, oneshot::Sender<Value>>>,
}

impl Peer {
    fn send(&self, v: Value) {
        let _ = self.out.send(v);
    }

    fn notify(&self, method: &str, params: Value) {
        self.send(json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    fn update(&self, session: &str, update: Value) {
        self.notify(
            "session/update",
            json!({"sessionId": session, "update": update}),
        );
    }

    /// Sends a request to the editor and waits for its result. A closed
    /// connection gives `None`.
    async fn request(&self, method: &str, params: Value) -> Option<Value> {
        let id = format!("tern-{}", self.next.fetch_add(1, Ordering::Relaxed));
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .expect("pending")
            .insert(json!(id).to_string(), tx);
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        let resp = rx.await.ok()?;
        resp.get("result").cloned()
    }
}

/// One editor session: its chat, and the running turn's cancel flag.
struct Slot {
    chat: tokio::sync::Mutex<Option<ChatSession>>,
    cancel: Mutex<Option<CancelFlag>>,
}

struct Server {
    peer: Arc<Peer>,
    opts: AcpOptions,
    sessions: Mutex<HashMap<String, Arc<Slot>>>,
}

fn rpc_error(id: &Value, code: i64, message: impl Into<String>) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message.into()}})
}

fn ok(id: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

/// Serves ACP on the process's stdin and stdout until the editor closes
/// them. Nothing else may write to stdout.
pub async fn serve_stdio(opts: AcpOptions) -> Result<(), String> {
    serve(tokio::io::stdin(), tokio::io::stdout(), opts).await
}

/// Serves ACP on any pair of streams (the tests use an in-memory pipe).
pub async fn serve<R, W>(input: R, output: W, opts: AcpOptions) -> Result<(), String>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (tx, mut rx) = mpsc::unbounded_channel::<Value>();
    let writer = tokio::spawn(async move {
        let mut output = output;
        while let Some(v) = rx.recv().await {
            if output.write_all(format!("{v}\n").as_bytes()).await.is_err() {
                break;
            }
            let _ = output.flush().await;
        }
    });
    let server = Arc::new(Server {
        peer: Arc::new(Peer {
            out: tx,
            next: AtomicU64::new(1),
            pending: Mutex::default(),
        }),
        opts,
        sessions: Mutex::default(),
    });
    let mut lines = BufReader::new(input).lines();
    while let Some(line) = lines.next_line().await.map_err(|e| e.to_string())? {
        let Ok(msg) = serde_json::from_str::<Value>(line.trim()) else {
            if !line.trim().is_empty() {
                server.peer.send(json!({"jsonrpc": "2.0", "id": null,
                    "error": {"code": -32700, "message": "parse error"}}));
            }
            continue;
        };
        let method = msg.get("method").and_then(Value::as_str).map(str::to_owned);
        let id = msg.get("id").cloned().filter(|v| !v.is_null());
        match (method, id) {
            // A response to one of our requests.
            (None, Some(id)) => {
                if let Some(tx) = server
                    .peer
                    .pending
                    .lock()
                    .expect("pending")
                    .remove(&id.to_string())
                {
                    let _ = tx.send(msg);
                }
            }
            (Some(m), None) => server.notification(&m, &msg["params"]),
            (Some(m), Some(id)) => {
                // Requests run beside each other, so a cancel or a
                // permission answer gets through while a prompt runs.
                let s = server.clone();
                tokio::spawn(async move {
                    let reply = s.request(&m, &id, msg["params"].clone()).await;
                    s.peer.send(reply);
                });
            }
            (None, None) => {}
        }
    }
    // The editor closed the pipe: stop running turns and finish writing.
    for slot in server.sessions.lock().expect("sessions").values() {
        if let Some(c) = slot.cancel.lock().expect("cancel").as_ref() {
            c.cancel();
        }
    }
    drop(server);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), writer).await;
    Ok(())
}

impl Server {
    fn notification(&self, method: &str, params: &Value) {
        if method == "session/cancel"
            && let Some(id) = params["sessionId"].as_str()
            && let Some(slot) = self.sessions.lock().expect("sessions").get(id)
            && let Some(c) = slot.cancel.lock().expect("cancel").as_ref()
        {
            c.cancel();
        }
    }

    async fn request(self: &Arc<Self>, method: &str, id: &Value, params: Value) -> Value {
        match method {
            "initialize" => ok(
                id,
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "agentCapabilities": {
                        "loadSession": true,
                        "promptCapabilities": {"image": false, "audio": false, "embeddedContext": true},
                        "mcpCapabilities": {"http": false, "sse": false}
                    },
                    "agentInfo": {"name": "tern", "title": "Tern", "version": env!("CARGO_PKG_VERSION")},
                    "authMethods": []
                }),
            ),
            "authenticate" => ok(id, json!({})),
            "session/new" => match self.open(None, &params).await {
                Ok((sid, _)) => ok(id, json!({"sessionId": sid})),
                Err(e) => rpc_error(id, -32603, e),
            },
            "session/load" => {
                let Some(sid) = params["sessionId"].as_str() else {
                    return rpc_error(id, -32602, "session/load needs a sessionId");
                };
                let Ok(session) = sid.parse::<SessionId>() else {
                    return rpc_error(id, -32602, "That is not a Tern session id.");
                };
                match self.open(Some(session), &params).await {
                    Ok((sid, turns)) => {
                        for t in &turns {
                            if let Some(m) = &t.message {
                                let (_, text) = crate::reply::split(m);
                                self.peer.update(
                                    &sid,
                                    json!({"sessionUpdate": "user_message_chunk",
                                           "content": {"type": "text", "text": text}}),
                                );
                            }
                            if let Some(a) = &t.answer {
                                self.peer.update(
                                    &sid,
                                    json!({"sessionUpdate": "agent_message_chunk",
                                           "content": {"type": "text", "text": a}}),
                                );
                            }
                        }
                        ok(id, Value::Null)
                    }
                    Err(e) => rpc_error(id, -32603, e),
                }
            }
            "session/prompt" => self.prompt(id, &params).await,
            "session/set_mode" => ok(id, json!({})),
            _ => rpc_error(id, -32601, format!("Tern does not support `{method}`")),
        }
    }

    async fn open(
        &self,
        resume: Option<SessionId>,
        params: &Value,
    ) -> Result<(String, Vec<crate::PastTurn>), String> {
        let mut opts = self.opts.chat.clone();
        if let Some(cwd) = params["cwd"].as_str().filter(|c| !c.is_empty()) {
            opts.project_root = Some(PathBuf::from(cwd));
        }
        let store = match &self.opts.db {
            Some(p) => Store::open(p).map_err(|e| e.to_string())?,
            None => crate::open_store()?,
        };
        let keychain = crate::providers::keychain();
        let secrets: Option<&dyn crate::providers::SecretStore> =
            keychain.as_ref().ok().map(|b| b.as_ref());
        let (chat, turns) = match resume {
            Some(s) => ChatSession::resume_with(opts, secrets, store, s).await?,
            None => (
                ChatSession::open_with(opts, secrets, store).await?,
                Vec::new(),
            ),
        };
        let sid = chat.session_id().to_string();
        self.sessions.lock().expect("sessions").insert(
            sid.clone(),
            Arc::new(Slot {
                chat: tokio::sync::Mutex::new(Some(chat)),
                cancel: Mutex::new(None),
            }),
        );
        Ok((sid, turns))
    }

    async fn prompt(self: &Arc<Self>, id: &Value, params: &Value) -> Value {
        let Some(sid) = params["sessionId"].as_str().map(str::to_owned) else {
            return rpc_error(id, -32602, "session/prompt needs a sessionId");
        };
        let Some(slot) = self.sessions.lock().expect("sessions").get(&sid).cloned() else {
            return rpc_error(
                id,
                -32602,
                "Unknown session. Call session/new or session/load first.",
            );
        };
        let text = prompt_text(&params["prompt"]);
        if text.trim().is_empty() {
            return rpc_error(id, -32602, "The prompt is empty.");
        }
        let mut guard = slot.chat.lock().await;
        let Some(chat) = guard.as_mut() else {
            return rpc_error(id, -32603, "The session is closed.");
        };
        let cancel = CancelFlag::default();
        *slot.cancel.lock().expect("cancel") = Some(cancel.clone());

        let stream = Arc::new(Stream::new(self.peer.clone(), sid.clone()));
        let observer = {
            let stream = stream.clone();
            move |e: &EventEnvelope| stream.event(e)
        };
        let on_text = {
            let stream = stream.clone();
            move |call: u32, t: &str| stream.text(call, t)
        };
        let approver = AcpApprover {
            stream: stream.clone(),
        };
        let out = chat
            .send_streaming(&text, &approver, cancel.clone(), &observer, Some(&on_text))
            .await;
        if let Some(a) = &out.answer {
            stream.finish(a);
            // Claim checks and the run report, as in the other front ends.
            let _ = chat.check_last_turn(&observer).await;
        }
        *slot.cancel.lock().expect("cancel") = None;
        drop(guard);
        if out.answer.is_none()
            && let Some(r) = &out.reason
            && out.state != TurnState::Cancelled
        {
            stream.say(&format!("\n\n{r}"));
        }
        let reason = match out.state {
            _ if cancel.is_cancelled() => "cancelled",
            TurnState::Cancelled => "cancelled",
            TurnState::BudgetExceeded => "max_turn_requests",
            _ => "end_turn",
        };
        ok(id, json!({"stopReason": reason}))
    }
}

/// The prompt's content blocks as one message: text as is, embedded
/// resources as attachments, links by address.
pub fn prompt_text(blocks: &Value) -> String {
    let mut parts = Vec::new();
    for b in blocks.as_array().into_iter().flatten() {
        match b["type"].as_str() {
            Some("text") => parts.push(b["text"].as_str().unwrap_or_default().to_owned()),
            Some("resource") => {
                let r = &b["resource"];
                if let Some(t) = r["text"].as_str() {
                    parts.push(format!(
                        "<attachment name=\"{}\">\n{t}\n</attachment>",
                        r["uri"].as_str().unwrap_or("file")
                    ));
                }
            }
            Some("resource_link") => parts.push(format!(
                "[{}]({})",
                b["name"].as_str().unwrap_or("link"),
                b["uri"].as_str().unwrap_or_default()
            )),
            _ => {}
        }
    }
    parts.join("\n\n")
}

/// Turns one prompt's events and text into `session/update`s.
struct Stream {
    peer: Arc<Peer>,
    session: String,
    state: Mutex<StreamState>,
}

#[derive(Default)]
struct StreamState {
    /// The model call whose text is streaming, and what of it was sent.
    call: u32,
    text: String,
    sent: usize,
    /// Text stopped at a tool block; the rest of this call is held.
    held: bool,
    /// Tool calls so far, and the one in progress.
    calls: u32,
    current: Option<String>,
}

fn tool_kind(tool: &str, effect: EffectClass) -> &'static str {
    if tool == "web_search" {
        return "search";
    }
    match effect {
        EffectClass::Read => "read",
        EffectClass::Analysis => "think",
        EffectClass::Write => "edit",
        EffectClass::Network => "fetch",
        EffectClass::Destructive => "delete",
        _ => "other",
    }
}

impl Stream {
    fn new(peer: Arc<Peer>, session: String) -> Self {
        Self {
            peer,
            session,
            state: Mutex::default(),
        }
    }

    fn say(&self, text: &str) {
        if !text.is_empty() {
            self.peer.update(
                &self.session,
                json!({"sessionUpdate": "agent_message_chunk",
                       "content": {"type": "text", "text": text}}),
            );
        }
    }

    /// Streams reply text, holding back anything from a ```` ```tool ````
    /// block on (and a trailing partial fence until it is clear).
    fn text(&self, call: u32, t: &str) {
        let chunk = {
            let mut s = self.state.lock().expect("stream");
            if call != s.call {
                s.call = call;
                s.text.clear();
                s.sent = 0;
                s.held = false;
            }
            s.text.push_str(t);
            if s.held {
                return;
            }
            let safe = match s.text.find("```") {
                Some(i) => {
                    s.held = true;
                    i
                }
                None => {
                    // Keep back trailing backticks that may start a fence.
                    let trimmed = s.text.trim_end_matches('`');
                    trimmed.len()
                }
            };
            if safe <= s.sent {
                return;
            }
            let c = s.text[s.sent..safe].to_owned();
            s.sent = safe;
            c
        };
        self.say(&chunk);
    }

    /// Sends whatever of the final answer was not streamed (an answer that
    /// contains code fences, or text held back).
    fn finish(&self, answer: &str) {
        let rest = {
            let s = self.state.lock().expect("stream");
            let sent = &s.text[..s.sent];
            let trimmed = sent.trim_start();
            if !trimmed.is_empty() && answer.starts_with(trimmed) {
                answer[trimmed.len()..].to_owned()
            } else if sent.trim().is_empty() {
                answer.to_owned()
            } else {
                // The streamed text was not this answer; send it whole.
                format!("\n\n{answer}")
            }
        };
        self.say(&rest);
    }

    fn event(&self, e: &EventEnvelope) {
        let p = &e.payload;
        let tool = p["tool"].as_str().unwrap_or_default();
        match e.kind.as_str() {
            "policy.decided" => {
                let id = {
                    let mut s = self.state.lock().expect("stream");
                    s.calls += 1;
                    let id = format!("call-{}", s.calls);
                    s.current = Some(id.clone());
                    id
                };
                let asks = p["decision"]["decision"] == "ask";
                let denied = p["decision"]["decision"] == "deny";
                self.peer.update(
                    &self.session,
                    json!({
                        "sessionUpdate": "tool_call",
                        "toolCallId": id,
                        "title": match p["target"].as_str() {
                            Some(t) => format!("{tool} · {t}"),
                            None => tool.to_owned(),
                        },
                        "kind": tool_kind(tool, e.effect_class),
                        "status": if denied { "failed" } else if asks { "pending" } else { "in_progress" },
                    }),
                );
            }
            "tool.executed" | "tool.failed" | "approval.answered" => {
                let Some(id) = self.state.lock().expect("stream").current.clone() else {
                    return;
                };
                let (status, text) = match e.kind.as_str() {
                    "tool.executed" => ("completed", e.user_summary.clone()),
                    "tool.failed" => ("failed", e.user_summary.clone()),
                    _ if p["answer"] == "deny" => ("failed", "You declined it".to_owned()),
                    _ => ("in_progress", e.user_summary.clone()),
                };
                self.peer.update(
                    &self.session,
                    json!({
                        "sessionUpdate": "tool_call_update",
                        "toolCallId": id,
                        "status": status,
                        "content": [{"type": "content", "content": {"type": "text", "text": text}}],
                    }),
                );
            }
            "claims.checked" if p["checks"].as_array().is_some_and(|c| !c.is_empty()) => {
                self.say(&format!("\n\n_{}_", e.user_summary));
            }
            _ => {}
        }
    }
}

/// Asks the editor when the policy gate asks.
struct AcpApprover {
    stream: Arc<Stream>,
}

impl Approver for AcpApprover {
    fn ask<'a>(&'a self, req: &'a ApprovalRequest) -> BoxFuture<'a, ApprovalAnswer> {
        Box::pin(async move {
            let id = self
                .stream
                .state
                .lock()
                .expect("stream")
                .current
                .clone()
                .unwrap_or_else(|| "call-0".into());
            let r = self
                .stream
                .peer
                .request(
                    "session/request_permission",
                    json!({
                        "sessionId": self.stream.session,
                        "toolCall": {
                            "toolCallId": id,
                            "title": format!("{} ({}): {}", req.tool, req.effect.label(), req.summary),
                            "kind": tool_kind(&req.tool, req.effect),
                            "status": "pending",
                            "rawInput": req.args,
                        },
                        "options": [
                            {"optionId": "allow-once", "name": "Allow once", "kind": "allow_once"},
                            {"optionId": "allow-session", "name": "Allow for this session", "kind": "allow_always"},
                            {"optionId": "reject-once", "name": "Reject", "kind": "reject_once"}
                        ]
                    }),
                )
                .await;
            let outcome = r.as_ref().map(|r| &r["outcome"]);
            match outcome.and_then(|o| o["optionId"].as_str()) {
                Some("allow-once") if outcome.is_some_and(|o| o["outcome"] == "selected") => {
                    ApprovalAnswer::AllowOnce
                }
                Some("allow-session") if outcome.is_some_and(|o| o["outcome"] == "selected") => {
                    ApprovalAnswer::AllowForSession
                }
                // Rejected, cancelled, or no answer.
                _ => ApprovalAnswer::Deny,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_blocks_become_one_message() {
        let t = prompt_text(&json!([
            {"type": "text", "text": "Summarise this"},
            {"type": "resource", "resource": {"uri": "file:///a.md", "text": "# A"}},
            {"type": "resource_link", "name": "b.pdf", "uri": "file:///b.pdf"},
            {"type": "image", "data": "x"}
        ]));
        assert_eq!(
            t,
            "Summarise this\n\n<attachment name=\"file:///a.md\">\n# A\n</attachment>\n\n[b.pdf](file:///b.pdf)"
        );
    }

    #[tokio::test]
    async fn permission_answers_map_to_the_gate() {
        for (pick, want) in [
            (
                json!({"outcome": "selected", "optionId": "allow-once"}),
                ApprovalAnswer::AllowOnce,
            ),
            (
                json!({"outcome": "selected", "optionId": "allow-session"}),
                ApprovalAnswer::AllowForSession,
            ),
            (
                json!({"outcome": "selected", "optionId": "reject-once"}),
                ApprovalAnswer::Deny,
            ),
            (json!({"outcome": "cancelled"}), ApprovalAnswer::Deny),
        ] {
            let (tx, mut rx) = mpsc::unbounded_channel();
            let peer = Arc::new(Peer {
                out: tx,
                next: AtomicU64::new(1),
                pending: Mutex::default(),
            });
            let approver = AcpApprover {
                stream: Arc::new(Stream::new(peer.clone(), "s".into())),
            };
            let req = ApprovalRequest {
                tool: "mcp__notes__delete".into(),
                summary: "Deletes a note".into(),
                effect: EffectClass::Destructive,
                target: None,
                args: json!({"id": 3}),
            };
            let answering = async {
                let sent = rx.recv().await.unwrap();
                assert_eq!(sent["method"], "session/request_permission");
                assert_eq!(sent["params"]["toolCall"]["kind"], "delete");
                assert_eq!(sent["params"]["options"].as_array().unwrap().len(), 3);
                let key = sent["id"].to_string();
                let tx = peer.pending.lock().unwrap().remove(&key).unwrap();
                tx.send(json!({"id": sent["id"], "result": {"outcome": pick}}))
                    .unwrap();
            };
            let (got, ()) = tokio::join!(approver.ask(&req), answering);
            assert_eq!(got, want);
        }
    }

    #[test]
    fn tool_blocks_are_held_back_from_the_stream() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let peer = Arc::new(Peer {
            out: tx,
            next: AtomicU64::new(1),
            pending: Mutex::default(),
        });
        let s = Stream::new(peer, "s".into());
        s.text(1, "Let me look.`");
        s.text(1, "``tool\n{\"tool\": \"web_search\"}");
        s.text(2, "Paris is the capital.");
        s.finish("Paris is the capital.");
        let mut texts = Vec::new();
        while let Ok(v) = rx.try_recv() {
            texts.push(
                v["params"]["update"]["content"]["text"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
            );
        }
        assert_eq!(texts, ["Let me look.", "Paris is the capital."]);
    }
}
