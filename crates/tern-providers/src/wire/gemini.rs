use serde_json::{Value, json};

use super::{str_at, u64_at};
use crate::{ChatRequest, ChatResponse, ModelInfo, ModelKind, Role, Usage};

pub(crate) fn chat_body(req: &ChatRequest) -> Value {
    let contents: Vec<Value> = req
        .messages
        .iter()
        .map(|m| {
            let role = match m.role {
                Role::User => "user",
                Role::Assistant => "model",
            };
            json!({"role": role, "parts": [{"text": m.content}]})
        })
        .collect();
    let mut config = json!({"maxOutputTokens": req.max_tokens});
    if let Some(t) = req.temperature {
        config["temperature"] = json!(t);
    }
    let mut body = json!({"contents": contents, "generationConfig": config});
    if let Some(s) = &req.system {
        body["systemInstruction"] = json!({"parts": [{"text": s}]});
    }
    body
}

pub(crate) fn parse_chat(v: &Value, req: &ChatRequest) -> Result<ChatResponse, String> {
    let cand = v.get("candidates").and_then(|c| c.get(0));
    let Some(cand) = cand else {
        let why = str_at(v, &["promptFeedback", "blockReason"]).unwrap_or("no candidates");
        return Err(format!("no answer ({why})"));
    };
    let text: String = cand
        .pointer("/content/parts")
        .and_then(Value::as_array)
        .map(|parts| {
            parts
                .iter()
                .filter_map(|p| p.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default();
    let usage = match (
        u64_at(v, &["usageMetadata", "promptTokenCount"]),
        u64_at(v, &["usageMetadata", "candidatesTokenCount"]),
    ) {
        (Some(i), Some(o)) => Usage::measured(i, o),
        _ => Usage::estimated(req.prompt_chars(), text.len()),
    };
    Ok(ChatResponse {
        text,
        usage,
        finish_reason: str_at(cand, &["finishReason"]).map(str::to_owned),
        model: str_at(v, &["modelVersion"]).map(str::to_owned),
    })
}

pub(crate) fn parse_models(v: &Value) -> Result<Vec<ModelInfo>, String> {
    let list = v
        .get("models")
        .and_then(Value::as_array)
        .ok_or("no `models` list in the reply")?;
    Ok(list
        .iter()
        .filter_map(|m| {
            let name = m.get("name")?.as_str()?;
            let mut info = ModelInfo::new(name.strip_prefix("models/").unwrap_or(name));
            info.display_name = str_at(m, &["displayName"]).map(str::to_owned);
            info.context_window = u64_at(m, &["inputTokenLimit"]);
            if let Some(methods) = m
                .get("supportedGenerationMethods")
                .and_then(Value::as_array)
            {
                let has = |x: &str| methods.iter().any(|v| v.as_str() == Some(x));
                if has("generateContent") {
                    info.kind = ModelKind::Chat;
                } else if has("embedContent") {
                    info.kind = ModelKind::Embedding;
                }
            }
            Some(info)
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ChatMessage;

    #[test]
    fn body_maps_roles_and_system() {
        let r = ChatRequest::new(
            "gemini-2.5-flash",
            vec![ChatMessage::user("q"), ChatMessage::assistant("a")],
        )
        .with_system("s");
        let b = chat_body(&r);
        assert_eq!(b["contents"][1]["role"], "model");
        assert_eq!(b["systemInstruction"]["parts"][0]["text"], "s");
        assert_eq!(b["generationConfig"]["maxOutputTokens"], 1024);
    }

    #[test]
    fn parses_reply_and_blocks() {
        let r = ChatRequest::new("m", vec![ChatMessage::user("hi")]);
        let v = json!({"candidates": [{"content": {"parts": [{"text": "Hi"}, {"text": "!"}]}, "finishReason": "STOP"}],
                       "usageMetadata": {"promptTokenCount": 4, "candidatesTokenCount": 2}, "modelVersion": "gemini-2.5-flash"});
        let out = parse_chat(&v, &r).unwrap();
        assert_eq!(
            (out.text.as_str(), out.usage),
            ("Hi!", Usage::measured(4, 2))
        );
        let blocked = json!({"promptFeedback": {"blockReason": "SAFETY"}});
        assert!(parse_chat(&blocked, &r).unwrap_err().contains("SAFETY"));
    }

    #[test]
    fn models_use_generation_methods() {
        let v = json!({"models": [
            {"name": "models/gemini-2.5-pro", "displayName": "Gemini 2.5 Pro", "inputTokenLimit": 1048576, "supportedGenerationMethods": ["generateContent"]},
            {"name": "models/text-embedding-004", "supportedGenerationMethods": ["embedContent"]}
        ]});
        let m = parse_models(&v).unwrap();
        assert_eq!(m[0].id, "gemini-2.5-pro");
        assert_eq!(m[0].context_window, Some(1_048_576));
        assert_eq!(m[1].kind, ModelKind::Embedding);
    }
}
