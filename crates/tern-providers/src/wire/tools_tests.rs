//! Native tool calling per wire format: the request carries the tools
//! (snapshots of the exact JSON), and calls in replies, whole or streamed,
//! come back as Tern `tool` blocks.

use serde_json::{Value, json};

use crate::stream::{Assembler, SseParser, decode};
use crate::{ChatMessage, ChatRequest, ToolSpec, Wire};

fn file_read() -> ToolSpec {
    ToolSpec {
        name: "file_read".into(),
        description: "Read a text file by path.".into(),
        parameters: json!({
            "type": "object",
            "properties": {"path": {"type": "string", "description": "The file's path"}},
            "required": ["path"],
            "additionalProperties": false
        }),
    }
}

fn req() -> ChatRequest {
    ChatRequest::new(
        "codestral-2508",
        vec![ChatMessage::user("read C:\\Users\\HP\\notes.txt")],
    )
    .with_system("You are Tern.")
    .with_max_tokens(512)
    .with_tools(vec![file_read()])
}

#[test]
fn openai_compatible_and_mistral_body_offers_the_tools() {
    let body = super::openai::chat_body(&req());
    assert_eq!(
        body,
        json!({
            "model": "codestral-2508",
            "messages": [
                {"role": "system", "content": "You are Tern."},
                {"role": "user", "content": "read C:\\Users\\HP\\notes.txt"}
            ],
            "max_tokens": 512,
            "stream": false,
            "tools": [{"type": "function", "function": {
                "name": "file_read",
                "description": "Read a text file by path.",
                "parameters": {
                    "type": "object",
                    "properties": {"path": {"type": "string", "description": "The file's path"}},
                    "required": ["path"],
                    "additionalProperties": false
                }
            }}],
            "tool_choice": "auto"
        })
    );
    // No tools, no keys: servers that know nothing of tools see the old body.
    let plain = super::openai::chat_body(&ChatRequest::new("m", vec![ChatMessage::user("hi")]));
    assert!(plain.get("tools").is_none() && plain.get("tool_choice").is_none());
}

#[test]
fn anthropic_body_offers_the_tools() {
    let body = super::anthropic::chat_body(&req());
    assert_eq!(
        body["tools"],
        json!([{
            "name": "file_read",
            "description": "Read a text file by path.",
            "input_schema": file_read().parameters
        }])
    );
    assert_eq!(body["tool_choice"], json!({"type": "auto"}));
}

#[test]
fn gemini_body_offers_the_tools_in_its_schema_subset() {
    let body = super::gemini::chat_body(&req());
    assert_eq!(
        body["tools"],
        json!([{"functionDeclarations": [{
            "name": "file_read",
            "description": "Read a text file by path.",
            "parameters": {
                "type": "object",
                "properties": {"path": {"type": "string", "description": "The file's path"}},
                "required": ["path"]
            }
        }]}])
    );
    assert_eq!(
        body["toolConfig"],
        json!({"functionCallingConfig": {"mode": "AUTO"}})
    );
}

fn block(name: &str, args: Value) -> String {
    crate::tool_block(name, &args)
}

#[test]
fn mistral_reply_with_tool_calls_becomes_a_tool_block() {
    // Mistral's documented shape: arguments as a JSON string.
    let v = json!({
        "id": "x", "model": "codestral-2508",
        "choices": [{"index": 0, "finish_reason": "tool_calls", "message": {
            "role": "assistant", "content": "",
            "tool_calls": [{"id": "D681PevKs", "type": "function", "function": {
                "name": "file_read", "arguments": "{\"path\": \"C:\\\\Users\\\\HP\\\\notes.txt\"}"
            }}]
        }}],
        "usage": {"prompt_tokens": 90, "completion_tokens": 20}
    });
    let r = super::openai::parse_chat(&v, &req()).unwrap();
    assert_eq!(
        r.text,
        block("file_read", json!({"path": "C:\\Users\\HP\\notes.txt"}))
    );
    // Some servers send the arguments as an object.
    let v = json!({"choices": [{"message": {"content": null, "tool_calls": [
        {"function": {"name": "web_search", "arguments": {"query": "tern bird"}}}
    ]}}]});
    let r = super::openai::parse_chat(&v, &req()).unwrap();
    assert_eq!(r.text, block("web_search", json!({"query": "tern bird"})));
}

#[test]
fn anthropic_and_gemini_replies_with_calls_become_tool_blocks() {
    let v = json!({"content": [
        {"type": "text", "text": "Reading it."},
        {"type": "tool_use", "id": "t1", "name": "file_read", "input": {"path": "/tmp/a.txt"}}
    ], "stop_reason": "tool_use"});
    let r = super::anthropic::parse_chat(&v, &req()).unwrap();
    assert_eq!(
        r.text,
        format!(
            "Reading it.\n{}",
            block("file_read", json!({"path": "/tmp/a.txt"}))
        )
    );
    let v = json!({"candidates": [{"content": {"parts": [
        {"functionCall": {"name": "file_read", "args": {"path": "/tmp/a.txt"}}}
    ]}, "finishReason": "STOP"}]});
    let r = super::gemini::parse_chat(&v, &req()).unwrap();
    assert_eq!(r.text, block("file_read", json!({"path": "/tmp/a.txt"})));
}

fn streamed(wire: Wire, raw: &str) -> String {
    let mut parser = SseParser::default();
    let mut events = parser.push(raw.as_bytes());
    events.extend(parser.finish());
    let mut asm = Assembler::default();
    for e in &events {
        asm.add(&decode(wire, e));
    }
    asm.finish(&req(), false).text
}

#[test]
fn streamed_tool_calls_are_assembled_for_every_wire() {
    let want = block("file_read", json!({"path": "/tmp/a.txt"}));
    // Mistral: the whole call in one delta.
    let mistral = concat!(
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"\"}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"id\":\"a\",\"function\":{\"name\":\"file_read\",\"arguments\":\"{\\\"path\\\": \\\"/tmp/a.txt\\\"}\"},\"index\":0}]},\"finish_reason\":\"tool_calls\"}]}\n\n",
        "data: [DONE]\n\n"
    );
    assert_eq!(streamed(Wire::OpenAiCompatible, mistral), want);
    // OpenAI: the name, then the arguments in fragments.
    let openai = concat!(
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"c\",\"type\":\"function\",\"function\":{\"name\":\"file_read\",\"arguments\":\"\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"path\\\": \"}}]}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"/tmp/a.txt\\\"}\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
        "data: [DONE]\n\n"
    );
    assert_eq!(streamed(Wire::OpenAiCompatible, openai), want);
    // Anthropic: a tool_use block start, then input_json_delta pieces.
    let anthropic = concat!(
        "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"t\",\"name\":\"file_read\",\"input\":{}}}\n\n",
        "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\\\": \\\"/tmp\"}}\n\n",
        "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"/a.txt\\\"}\"}}\n\n",
        "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"
    );
    assert_eq!(streamed(Wire::Anthropic, anthropic), want);
    // Gemini: the call whole in one chunk.
    let gemini = "data: {\"candidates\":[{\"content\":{\"parts\":[{\"functionCall\":{\"name\":\"file_read\",\"args\":{\"path\":\"/tmp/a.txt\"}}}]}}]}\n\n";
    assert_eq!(streamed(Wire::Gemini, gemini), want);
}

/// The exact streaming body Tern sends Mistral for a Quick turn with web
/// search on, after the message asked for the web: the web tool offered
/// natively with `tool_choice: auto`, and the search Tern ran first in
/// the conversation as its call and the live results (#66).
#[test]
fn mistral_quick_turn_body_with_the_web_tool_snapshot() {
    let web = ToolSpec {
        name: "web_search".into(),
        description:
            "Search the web. Returns titles, links, and snippets; read a link before citing it."
                .into(),
        parameters: json!({"type": "object", "required": ["query"], "additionalProperties": false,
            "properties": {"query": {"type": "string"}, "limit": {"type": "integer"}}}),
    };
    let call = crate::tool_block("web_search", &json!({"query": "who created LangChain"}));
    let results = "Result of `web_search`:\nWeb search results for \"who created LangChain\" from Wikipedia, retrieved just now.";
    let req = ChatRequest::new(
        "codestral-2508",
        vec![
            ChatMessage::user("who created LangChain? use the web tool"),
            ChatMessage::assistant(call.clone()),
            ChatMessage::user(results),
        ],
    )
    .with_system("You are Tern.")
    .with_max_tokens(4096)
    .with_tools(vec![web]);
    let mut body = super::openai::chat_body(&req);
    body["stream"] = json!(true);
    let spec = crate::catalog::find("mistral").expect("mistral is built in");
    crate::effort::apply(&mut body, spec, &req);
    assert_eq!(
        body,
        json!({
            "model": "codestral-2508",
            "messages": [
                {"role": "system", "content": "You are Tern."},
                {"role": "user", "content": "who created LangChain? use the web tool"},
                {"role": "assistant", "content": call},
                {"role": "user", "content": results}
            ],
            "max_tokens": 4096,
            "stream": true,
            "tools": [{"type": "function", "function": {
                "name": "web_search",
                "description": "Search the web. Returns titles, links, and snippets; read a link before citing it.",
                "parameters": {"type": "object", "required": ["query"], "additionalProperties": false,
                    "properties": {"query": {"type": "string"}, "limit": {"type": "integer"}}}
            }}],
            "tool_choice": "auto"
        })
    );
}

/// Mistral streams parallel calls each whole, every one under `index` 0
/// with its own id; they come back as separate calls, not glued together.
#[test]
fn mistral_parallel_calls_under_one_index_stay_apart() {
    let raw = concat!(
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"id\":\"a1\",\"function\":{\"name\":\"web_search\",\"arguments\":\"{\\\"query\\\": \\\"langchain\\\"}\"},\"index\":0}]}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"id\":\"b2\",\"function\":{\"name\":\"read_url\",\"arguments\":\"{\\\"url\\\": \\\"https://x.org\\\"}\"},\"index\":0}]},\"finish_reason\":\"tool_calls\"}]}\n\n",
        "data: [DONE]\n\n"
    );
    assert_eq!(
        streamed(Wire::OpenAiCompatible, raw),
        format!(
            "{}\n{}",
            block("web_search", json!({"query": "langchain"})),
            block("read_url", json!({"url": "https://x.org"}))
        )
    );
    // OpenAI's parallel calls use distinct indexes and fragments.
    let openai = concat!(
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"c\",\"function\":{\"name\":\"web_search\",\"arguments\":\"{\\\"query\\\":\"}},{\"index\":1,\"id\":\"d\",\"function\":{\"name\":\"web_search\",\"arguments\":\"{\\\"query\\\":\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"a\\\"}\"}},{\"index\":1,\"function\":{\"arguments\":\"\\\"b\\\"}\"}}]}}]}\n\n",
        "data: [DONE]\n\n"
    );
    assert_eq!(
        streamed(Wire::OpenAiCompatible, openai),
        format!(
            "{}\n{}",
            block("web_search", json!({"query": "a"})),
            block("web_search", json!({"query": "b"}))
        )
    );
}
