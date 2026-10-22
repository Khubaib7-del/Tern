use serde_json::{Value, json};

use super::{str_at, u64_at};
use crate::{ChatRequest, ChatResponse, ModelInfo, Usage};

/// Plain text, or text and pictures as content parts.
fn content(m: &crate::ChatMessage) -> Value {
    if m.images.is_empty() {
        return json!(m.content);
    }
    let mut parts = vec![json!({"type": "text", "text": m.content})];
    parts.extend(
        m.images
            .iter()
            .map(|i| json!({"type": "image_url", "image_url": {"url": i.data_url()}})),
    );
    Value::Array(parts)
}

pub(crate) fn chat_body(req: &ChatRequest) -> Value {
    let mut messages = Vec::new();
    if let Some(s) = &req.system {
        messages.push(json!({"role": "system", "content": s}));
    }
    messages.extend(
        req.messages
            .iter()
            .map(|m| json!({"role": m.role, "content": content(m)})),
    );
    let mut body = json!({"model": req.model, "messages": messages, "max_tokens": req.max_tokens, "stream": false});
    if let Some(t) = req.temperature {
        body["temperature"] = json!(t);
    }
    body
}

pub(crate) fn parse_chat(v: &Value, req: &ChatRequest) -> Result<ChatResponse, String> {
    let choice = v
        .get("choices")
        .and_then(|c| c.get(0))
        .ok_or("no choices in the reply")?;
    let text = str_at(choice, &["message", "content"])
        .unwrap_or_default()
        .to_owned();
    let usage = match (
        u64_at(v, &["usage", "prompt_tokens"]),
        u64_at(v, &["usage", "completion_tokens"]),
    ) {
        (Some(i), Some(o)) => Usage::measured(i, o),
        _ => Usage::estimated(req.prompt_chars(), text.len()),
    };
    Ok(ChatResponse {
        text,
        usage,
        finish_reason: str_at(choice, &["finish_reason"]).map(str::to_owned),
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
            let id = m.get("id")?.as_str()?;
            let mut info = ModelInfo::new(id);
            info.display_name = str_at(m, &["name"]).map(str::to_owned);
            info.context_window =
                u64_at(m, &["context_length"]).or_else(|| u64_at(m, &["context_window"]));
            Some(info)
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ChatMessage, ModelKind};
    use tern_core::Measure;

    fn req() -> ChatRequest {
        ChatRequest::new("gpt-4.1-mini", vec![ChatMessage::user("hi")]).with_system("be brief")
    }

    #[test]
    fn body_puts_system_first() {
        let b = chat_body(&req());
        assert_eq!(b["messages"][0]["role"], "system");
        assert_eq!(b["messages"][1], json!({"role": "user", "content": "hi"}));
        assert_eq!(b["stream"], false);
        assert!(b.get("temperature").is_none());
    }

    #[test]
    fn parses_reply_with_usage() {
        let v = json!({"model": "gpt-4.1-mini-2025", "choices": [{"message": {"content": "Hello."}, "finish_reason": "stop"}],
                       "usage": {"prompt_tokens": 12, "completion_tokens": 3}});
        let r = parse_chat(&v, &req()).unwrap();
        assert_eq!(r.text, "Hello.");
        assert_eq!(r.usage, Usage::measured(12, 3));
        assert_eq!(r.finish_reason.as_deref(), Some("stop"));
    }

    #[test]
    fn estimates_when_usage_is_missing() {
        let v = json!({"choices": [{"message": {"content": "Hello there"}}]});
        assert_eq!(
            parse_chat(&v, &req()).unwrap().usage.measure,
            Measure::Estimated
        );
        assert!(parse_chat(&json!({}), &req()).is_err());
    }

    #[test]
    fn parses_openrouter_style_models() {
        let v = json!({"data": [{"id": "anthropic/claude-sonnet-4.5", "name": "Claude Sonnet 4.5", "context_length": 200000},
                                {"id": "text-embedding-3-small"}, {"no_id": true}]});
        let m = parse_models(&v).unwrap();
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].context_window, Some(200_000));
        assert_eq!(m[1].kind, ModelKind::Embedding);
    }
}
