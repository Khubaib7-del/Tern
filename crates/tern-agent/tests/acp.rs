//! `tern acp` end to end: an editor-side client talks to the ACP server
//! over an in-memory pipe, with a mock model behind it.

use std::time::Duration;

use serde_json::{Value, json};
use tern_agent::ChatOptions;
use tern_agent::acp::{AcpOptions, serve};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, Lines};
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn sse(parts: &[&str]) -> String {
    let mut s = String::new();
    for (i, p) in parts.iter().enumerate() {
        let finish = if i + 1 == parts.len() {
            json!("stop")
        } else {
            Value::Null
        };
        s.push_str(&format!(
            "data: {}\n\n",
            json!({"choices": [{"delta": {"content": p}, "finish_reason": finish}]})
        ));
    }
    s.push_str("data: [DONE]\n\n");
    s
}

async fn model() -> MockServer {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"data": [{"id": "llama3.2"}]})),
        )
        .mount(&s)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_string_contains("take your time"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_millis(1500))
                .set_body_raw(sse(&["Too late."]), "text/event-stream"),
        )
        .with_priority(1)
        .mount(&s)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(sse(&["Hello ", "from the mock."]), "text/event-stream"),
        )
        .with_priority(5)
        .mount(&s)
        .await;
    s
}

struct Editor {
    to: DuplexStream,
    from: Lines<BufReader<DuplexStream>>,
    next: u64,
}

impl Editor {
    fn start(opts: AcpOptions) -> Self {
        let (client_out, server_in) = tokio::io::duplex(1 << 16);
        let (server_out, client_in) = tokio::io::duplex(1 << 16);
        tokio::spawn(serve(server_in, server_out, opts));
        Self {
            to: client_out,
            from: BufReader::new(client_in).lines(),
            next: 1,
        }
    }

    async fn send(&mut self, v: Value) {
        self.to
            .write_all(format!("{v}\n").as_bytes())
            .await
            .unwrap();
    }

    async fn request(&mut self, method: &str, params: Value) -> u64 {
        let id = self.next;
        self.next += 1;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await;
        id
    }

    /// Reads until the response to `id`, returning it and the updates
    /// that came before.
    async fn until(&mut self, id: u64) -> (Value, Vec<Value>) {
        let mut updates = Vec::new();
        loop {
            let line = tokio::time::timeout(Duration::from_secs(20), self.from.next_line())
                .await
                .expect("the server answers")
                .unwrap()
                .expect("open");
            let v: Value = serde_json::from_str(&line).unwrap();
            if v["id"] == id && v.get("method").is_none() {
                return (v, updates);
            }
            if v["method"] == "session/update" {
                updates.push(v["params"]["update"].clone());
            }
        }
    }

    async fn call(&mut self, method: &str, params: Value) -> (Value, Vec<Value>) {
        let id = self.request(method, params).await;
        self.until(id).await
    }
}

fn texts(updates: &[Value], kind: &str) -> String {
    updates
        .iter()
        .filter(|u| u["sessionUpdate"] == kind)
        .map(|u| u["content"]["text"].as_str().unwrap_or_default())
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_round_trips_and_loads_again() {
    let m = model().await;
    let db = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::write(
        project.path().join("TERN.md"),
        "Always answer in one sentence.",
    )
    .unwrap();
    let opts = AcpOptions {
        chat: ChatOptions {
            provider: Some("ollama".into()),
            base_url: Some(format!("{}/v1", m.uri())),
            no_mcp: true,
            search: Some("off".into()),
            check_claims: tern_agent::CheckClaims::Off,
            ..Default::default()
        },
        db: Some(db.path().join("tern.sqlite3")),
    };

    let mut ed = Editor::start(opts.clone());
    let (init, _) = ed
        .call(
            "initialize",
            json!({"protocolVersion": 1, "clientCapabilities": {"fs": {"readTextFile": true}},
                   "clientInfo": {"name": "test-editor", "version": "1"}}),
        )
        .await;
    assert_eq!(init["result"]["protocolVersion"], 1);
    assert_eq!(init["result"]["agentCapabilities"]["loadSession"], true);
    assert_eq!(init["result"]["agentInfo"]["name"], "tern");

    let (new, _) = ed
        .call(
            "session/new",
            json!({"cwd": project.path(), "mcpServers": []}),
        )
        .await;
    let sid = new["result"]["sessionId"].as_str().unwrap().to_owned();

    let (done, updates) = ed
        .call(
            "session/prompt",
            json!({"sessionId": sid, "prompt": [{"type": "text", "text": "Say hello"}]}),
        )
        .await;
    assert_eq!(done["result"]["stopReason"], "end_turn");
    assert_eq!(
        texts(&updates, "agent_message_chunk"),
        "Hello from the mock."
    );
    // The project's TERN.md reached the model.
    let reqs = m.received_requests().await.unwrap();
    let body: Value = reqs.last().unwrap().body_json().unwrap();
    assert!(body.to_string().contains("Always answer in one sentence."));

    // Cancel a slow turn.
    let id = ed
        .request(
            "session/prompt",
            json!({"sessionId": sid, "prompt": [{"type": "text", "text": "take your time"}]}),
        )
        .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    ed.send(json!({"jsonrpc": "2.0", "method": "session/cancel", "params": {"sessionId": sid}}))
        .await;
    let (stopped, _) = ed.until(id).await;
    assert_eq!(stopped["result"]["stopReason"], "cancelled");

    // Unknown methods are refused, not ignored.
    let (e, _) = ed.call("session/teleport", json!({})).await;
    assert_eq!(e["error"]["code"], -32601);
    drop(ed);

    // A new editor connection loads the session: the history replays.
    let mut ed = Editor::start(opts);
    ed.call("initialize", json!({"protocolVersion": 1})).await;
    let (loaded, updates) = ed
        .call(
            "session/load",
            json!({"sessionId": sid, "cwd": project.path(), "mcpServers": []}),
        )
        .await;
    assert!(loaded.get("error").is_none(), "{loaded}");
    assert!(texts(&updates, "user_message_chunk").starts_with("Say hello"));
    assert_eq!(
        texts(&updates, "agent_message_chunk"),
        "Hello from the mock."
    );
    let (again, updates) = ed
        .call(
            "session/prompt",
            json!({"sessionId": sid, "prompt": [{"type": "text", "text": "And again"}]}),
        )
        .await;
    assert_eq!(again["result"]["stopReason"], "end_turn");
    assert!(!texts(&updates, "agent_message_chunk").is_empty());
    let reqs = m.received_requests().await.unwrap();
    let body: Value = reqs.last().unwrap().body_json().unwrap();
    let said: Vec<_> = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == "user")
        .map(|m| m["content"].as_str().unwrap_or_default().to_owned())
        .collect();
    // The stopped turn stays in history with its digest (#85, A1), so
    // the model knows it was asked and cut off.
    assert_eq!(
        said,
        ["Say hello", "take your time", "And again"],
        "the loaded history goes to the model, the stopped turn included"
    );
}
