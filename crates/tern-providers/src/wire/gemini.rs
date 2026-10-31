use serde_json::{Value, json};

use super::{str_at, u64_at};
use crate::{ChatRequest, ChatResponse, ModelInfo, Role, Usage};

pub(crate) fn chat_body(req: &ChatRequest) -> Value {
    let contents: Vec<Value> = req
        .messages
        .iter()
        .map(|m| {
            let role = match m.role {
                Role::User => "user",
                Role::Assistant => "model",
            };
            let mut parts = vec![json!({"text": m.content})];
            parts.extend(
                m.images
                    .iter()
                    .map(|i| json!({"inline_data": {"mime_type": i.media_type, "data": i.data}})),
            );
            json!({"role": role, "parts": parts})
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
    if !req.tools.is_empty() {
        let decls: Vec<Value> = req
            .tools
            .iter()
            .map(|t| {
                let mut d = json!({"name": t.name, "description": t.description});
                let p = schema(&t.parameters);
                if p.get("properties")
                    .and_then(Value::as_object)
                    .is_some_and(|o| !o.is_empty())
                {
                    d["parameters"] = p;
                }
                d
            })
            .collect();
        body["tools"] = json!([{"functionDeclarations": decls}]);
        body["toolConfig"] = json!({"functionCallingConfig": {"mode": "AUTO"}});
    }
    body
}

/// A JSON Schema cut down to the OpenAPI subset Gemini accepts (it
/// refuses `additionalProperties`, `$schema`, `default` and the like).
pub(crate) fn schema(v: &Value) -> Value {
    let Some(o) = v.as_object() else {
        return v.clone();
    };
    let mut out = serde_json::Map::new();
    for (k, val) in o {
        match k.as_str() {
            "type" => {
                // `["string", "null"]` becomes `string`, nullable.
                if let Some(a) = val.as_array() {
                    let t = a.iter().find(|t| t.as_str() != Some("null"));
                    out.insert("type".into(), t.cloned().unwrap_or(json!("string")));
                    if a.iter().any(|t| t.as_str() == Some("null")) {
                        out.insert("nullable".into(), json!(true));
                    }
                } else {
                    out.insert("type".into(), val.clone());
                }
            }
            "description" | "enum" | "format" | "nullable" | "required" | "minimum" | "maximum"
            | "minItems" | "maxItems" => {
                out.insert(k.clone(), val.clone());
            }
            "items" => {
                out.insert(k.clone(), schema(val));
            }
            "properties" => {
                let props = val
                    .as_object()
                    .map(|p| p.iter().map(|(n, s)| (n.clone(), schema(s))).collect())
                    .unwrap_or_default();
                out.insert(k.clone(), Value::Object(props));
            }
            _ => {}
        }
    }
    Value::Object(out)
}

/// The native function calls in a candidate's parts.
pub(crate) fn function_calls(parts: Option<&Vec<Value>>) -> Vec<(String, Value)> {
    parts
        .into_iter()
        .flatten()
        .filter_map(|p| {
            let f = p.get("functionCall")?;
            Some((
                f.get("name")?.as_str()?.to_owned(),
                f.get("args").cloned().unwrap_or(Value::Null),
            ))
        })
        .collect()
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
    let text = crate::chat::with_tool_calls(
        text,
        &function_calls(cand.pointer("/content/parts").and_then(Value::as_array)),
    );
    let usage = match (
        u64_at(v, &["usageMetadata", "promptTokenCount"]),
        u64_at(v, &["usageMetadata", "candidatesTokenCount"]),
    ) {
        (Some(i), Some(o)) => {
            // Thoughts are billed output Gemini leaves out of
            // `candidatesTokenCount` (#85, A19).
            let (cached, thoughts) = crate::usage::gemini_details(v.get("usageMetadata"));
            Usage::measured(i, o + thoughts).with_details(cached, 0, thoughts)
        }
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
    use crate::model::{Capability::*, Source};
    let list = v
        .get("models")
        .and_then(Value::as_array)
        .ok_or("no `models` list in the reply")?;
    Ok(list
        .iter()
        .filter_map(|m| {
            let name = m.get("name")?.as_str()?;
            let id = name.strip_prefix("models/").unwrap_or(name);
            let mut info = match m
                .get("supportedGenerationMethods")
                .and_then(Value::as_array)
            {
                Some(methods) => {
                    let has = |x: &str| methods.iter().any(|v| v.as_str() == Some(x));
                    let mut caps = Vec::new();
                    if has("generateContent") {
                        caps.push(Chat);
                    }
                    if has("embedContent") || has("embedText") || has("batchEmbedContents") {
                        caps.push(Embedding);
                    }
                    if has("predict") && !has("generateContent") {
                        caps.push(ImageGen);
                    }
                    if m.get("thinking").and_then(Value::as_bool) == Some(true) {
                        caps.push(Reasoning);
                    }
                    ModelInfo::with_listing(id, caps, Source::Partial)
                }
                None => ModelInfo::new(id),
            };
            info.display_name = str_at(m, &["displayName"]).map(str::to_owned);
            info.context_window = u64_at(m, &["inputTokenLimit"]);
            Some(info)
        })
        .collect())
}

/// The `pageToken` for the next page, when there is one.
pub(crate) fn next_cursor(v: &Value) -> Option<String> {
    str_at(v, &["nextPageToken"])
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ChatMessage;
    use crate::ModelKind;

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
        let mut e = m[1].clone();
        e.finish("google");
        assert_eq!(e.kind, ModelKind::Embedding);
    }
}
