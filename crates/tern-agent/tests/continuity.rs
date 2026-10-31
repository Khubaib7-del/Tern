//! Phase 9 continuity against a mock OpenAI-compatible server: a session
//! started from a handoff packet alone, a sub-session run that reports
//! back, `@Q` mentions, and removed turns leaving the model's history.

use serde_json::json;
use tern_agent::{CancelFlag, ChatOptions, ChatSession, DenyAll, handoff, range, sub};
use tern_core::{EffectClass, EventEnvelope, SessionId};
use tern_store::{Harness, Store};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn server(answer: &str) -> MockServer {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{"id": "llama3.2"}]
        })))
        .mount(&s)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{"message": {"role": "assistant", "content": answer}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5}
        })))
        .mount(&s)
        .await;
    s
}

fn opts(s: &MockServer) -> ChatOptions {
    ChatOptions {
        provider: Some("ollama".into()),
        base_url: Some(format!("{}/v1", s.uri())),
        search: Some("off".into()),
        ..Default::default()
    }
}

fn turn(store: &mut Store, s: SessionId, q: &str, a: &str) {
    for (kind, payload) in [
        ("turn.started", json!({ "message": q })),
        ("turn.finished", json!({"state": "completed", "answer": a})),
    ] {
        store
            .append(&EventEnvelope::new(
                s,
                kind,
                EffectClass::Analysis,
                kind,
                payload,
            ))
            .unwrap();
    }
}

/// The non-system messages of the last request.
async fn last_messages(s: &MockServer) -> Vec<String> {
    let reqs = s.received_requests().await.unwrap();
    let body: serde_json::Value = reqs.last().unwrap().body_json().unwrap();
    body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] != "system")
        .map(|m| m["content"].as_str().unwrap_or_default().to_owned())
        .collect()
}

#[tokio::test]
async fn a_new_session_starts_from_the_packet_alone() {
    let mut old = Store::open_in_memory().unwrap();
    let p = old.ensure_project("Inbox").unwrap();
    let s = old
        .create_session(p, Harness::Research, "Vector DBs")
        .unwrap();
    turn(
        &mut old,
        s,
        "Which vector database should my app use?",
        "I recommend pgvector. SECRET-OLD-DETAIL lives only in the old log.",
    );
    turn(
        &mut old,
        s,
        "We decided to use pgvector.",
        "Next step: benchmark recall at 10.",
    );
    let packet = handoff::for_session(&old, s, 1).unwrap();
    let md = handoff::to_markdown(&packet);
    drop(old);

    // Only the Markdown file travels; the old database is gone.
    let packet = handoff::parse(&md).unwrap();
    let mut fresh = Store::open_in_memory().unwrap();
    let id = handoff::start_from(&mut fresh, &packet).unwrap();

    let server = server("Let's benchmark.").await;
    let (mut chat, turns) = ChatSession::resume_with(opts(&server), None, fresh, id)
        .await
        .unwrap();
    assert!(turns.is_empty());
    chat.send(
        "What should I do next?",
        &DenyAll,
        CancelFlag::default(),
        &|_| {},
    )
    .await;
    let msgs = last_messages(&server).await;
    assert_eq!(
        msgs.len(),
        3,
        "the packet, its acknowledgement, the question"
    );
    assert!(msgs[0].contains("Which vector database should my app use?"));
    assert!(msgs[0].contains("benchmark recall at 10"));
    assert!(msgs[0].contains("## Decisions"));
    assert!(!msgs.iter().any(|m| m.contains("SECRET-OLD-DETAIL")));
    assert_eq!(msgs[2], "What should I do next?");
}

#[tokio::test]
async fn a_sub_session_runs_and_reports_back_and_removed_turns_leave_history() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("t.sqlite3");
    let (top, planned) = {
        let mut store = Store::open(&db).unwrap();
        let p = store.ensure_project("Inbox").unwrap();
        let top = store.create_session(p, Harness::Research, "RAG").unwrap();
        turn(
            &mut store,
            top,
            "What is RAG?",
            "Retrieval plus generation.",
        );
        turn(&mut store, top, "Off-topic tangent", "A tangent.");
        turn(&mut store, top, "Costs?", "It depends on the database.");
        let planned = sub::create(
            &mut store,
            top,
            3,
            None,
            Some("Compare vector database prices"),
        )
        .unwrap();
        (top, planned.id)
    };

    let server = server("Pinecone costs more than pgvector.").await;
    let (chat, _) =
        ChatSession::resume_with(opts(&server), None, Store::open(&db).unwrap(), planned)
            .await
            .unwrap();
    let run = sub::run_in(chat, None, &DenyAll, CancelFlag::default(), &|_| {})
        .await
        .unwrap();
    assert_eq!(run.question, "Compare vector database prices");
    assert_eq!(
        run.summary.as_deref(),
        Some("Pinecone costs more than pgvector.")
    );
    // The sub-session read the parent's context through Q3.
    let msgs = last_messages(&server).await;
    assert!(msgs[0].contains("Costs?"));

    let mut store = Store::open(&db).unwrap();
    let rows = sub::list(&store, top).unwrap();
    assert!(rows[0].reported && !rows[0].planned);
    assert_eq!(rows[0].refs, ["Q3.1"]);

    // Remove the tangent; the parent's model no longer reads it, but it
    // does read the sub-session's report.
    range::remove(&mut store, top, &[2]).unwrap();
    drop(store);
    let (mut parent, turns) =
        ChatSession::resume_with(opts(&server), None, Store::open(&db).unwrap(), top)
            .await
            .unwrap();
    assert_eq!(turns.iter().map(|t| t.number).collect::<Vec<_>>(), [1, 3]);
    assert_eq!(turns[1].reports.len(), 1);
    parent
        .send(
            "Summarise @Q3.1 for me",
            &DenyAll,
            CancelFlag::default(),
            &|_| {},
        )
        .await;
    let msgs = last_messages(&server).await;
    assert!(!msgs.iter().any(|m| m.contains("tangent")));
    assert!(
        msgs.iter()
            .any(|m| m.contains("Sub-session Q3.1 reported back"))
    );
    let last = msgs.last().unwrap();
    assert!(last.starts_with("Summarise @Q3.1 for me"));
    assert!(last.contains("<attachment name=\"Q3.1\""));
    assert!(last.contains("Pinecone costs more"));
}
