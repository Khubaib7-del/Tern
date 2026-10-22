use serde_json::{Value, json};

use super::{str_at, u64_at};
use crate::{ChatRequest, ChatResponse, ModelInfo, Usage};

pub(crate) const VERSION: &str = "2023-06-01";

/// Plain text, or pictures then text as content blocks (Anthropic reads
/// images best ahead of the question).
fn content(m: &crate::ChatMessage) -> Value {
    if m.images.is_empty() {
        return json!(m.content);
    }
    let mut parts: Vec<Value> = m
        .images
        .iter()
        .map(|i| {
            json!({"type": "image", "source": {"type": "base64", "media_type": i.media_type, "data": i.data}})
        })
        .collect();
    parts.push(json!({"type": "text", "text": m.content}));
    Value::Array(parts)
}

pub(crate) fn chat_body(req: &ChatRequest) -> Value {
    let messages: Vec<Value> = req
        .messages
        .iter()
        .map(|m| json!({"role": m.role, "content": content(m)}))
        .collect();
    let mut body = json!({"model": req.model, "max_tokens": req.max_tokens, "messages": messages});
    if let Some(s) = &req.system {
        body["system"] = json!(s);
    }
    if let Some(t) = req.temperature {
        body["temperature"] = json!(t);
    }
    body
}

pub(crate) fn parse_chat(v: &Value, req: &ChatRequest) -> Result<ChatResponse, String> {
    let blocks = v
        .get("content")
        .and_then(Value::as_array)
        .ok_or("no `content` in the reply")?;
    let text: String = blocks
        .iter()
        .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|b| b.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("");
    let usage = match (
        u64_at(v, &["usage", "input_tokens"]),
        u64_at(v, &["usage", "output_tokens"]),
    ) {
        (Some(i), Some(o)) => Usage::measured(i, o),
        _ => Usage::estimated(req.prompt_chars(), text.len()),
    };
    Ok(ChatResponse {
        text,
        usage,
        finish_reason: str_at(v, &["stop_reason"]).map(str::to_owned),
        model: str_at(v, &["model"]).map(str::to_owned),
    })
}

pub(crate) fn parse_models(v: &Value) -> Result<Vec<ModelInfo>, String> {
    let data = v
        .get("data")
        .and_then(Value::as_array)
        .ok_or("no `data` list in the models reply")?;
    Ok(data
        .iter()
        .filter_map(|m| {
            let mut info = ModelInfo::new(m.get("id")?.as_str()?);
            info.display_name = str_at(m, &["display_name"]).map(str::to_owned);
            Some(info)
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ChatMessage;

    #[test]
    fn body_uses_top_level_system() {
        let r = ChatRequest::new("claude-sonnet-4-5", vec![ChatMessage::user("hi")])
            .with_system("cite sources");
        let b = chat_body(&r);
        assert_eq!(b["system"], "cite sources");
        assert_eq!(b["messages"].as_array().unwrap().len(), 1);
        assert_eq!(b["max_tokens"], 1024);
    }

    #[test]
    fn joins_text_blocks_and_reads_usage() {
        let r = ChatRequest::new("m", vec![ChatMessage::user("hi")]);
        let v = json!({"model": "claude-sonnet-4-5", "stop_reason": "end_turn",
                       "content": [{"type": "text", "text": "Hel"}, {"type": "tool_use"}, {"type": "text", "text": "lo"}],
                       "usage": {"input_tokens": 9, "output_tokens": 2}});
        let out = parse_chat(&v, &r).unwrap();
        assert_eq!(out.text, "Hello");
        assert_eq!(out.usage, Usage::measured(9, 2));
        assert_eq!(out.finish_reason.as_deref(), Some("end_turn"));
    }

    #[test]
    fn parses_models_with_display_names() {
        let v = json!({"data": [{"id": "claude-opus-4-1", "display_name": "Claude Opus 4.1", "type": "model"}]});
        let m = parse_models(&v).unwrap();
        assert_eq!(m[0].display_name.as_deref(), Some("Claude Opus 4.1"));
    }
}
