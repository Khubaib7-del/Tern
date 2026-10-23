use serde_json::{Value, json};

use super::{str_at, u64_at};
use crate::model::{Capability, Lifecycle, Source};
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
    // Most servers wrap the list in `data`; Together sends a bare array.
    let data = v
        .get("data")
        .and_then(Value::as_array)
        .or_else(|| v.as_array())
        .ok_or("no `data` list in the models reply")?;
    Ok(data.iter().filter_map(model_card).collect())
}

/// One model card. Reads every capability field the OpenAI-compatible
/// providers send: Mistral `capabilities`, OpenRouter `architecture` and
/// `supported_parameters`, xAI `input_modalities`/`output_modalities`,
/// Together and LM Studio `type`.
fn model_card(m: &Value) -> Option<ModelInfo> {
    let id = m.get("id")?.as_str()?;
    let (caps, source) = card_capabilities(m);
    let mut info = match caps {
        Some(caps) => ModelInfo::with_listing(id, caps, source),
        None => ModelInfo::new(id),
    };
    info.display_name = str_at(m, &["name"])
        .or_else(|| str_at(m, &["display_name"]))
        .filter(|n| !n.is_empty() && *n != id)
        .map(str::to_owned);
    // OpenRouter and LM Studio say `context_length`, Mistral
    // `max_context_length`, Groq `context_window`, vLLM `max_model_len`.
    info.context_window = u64_at(m, &["context_length"])
        .or_else(|| u64_at(m, &["max_context_length"]))
        .or_else(|| u64_at(m, &["context_window"]))
        .or_else(|| u64_at(m, &["max_model_len"]));
    if let Some(a) = m.get("aliases").and_then(Value::as_array) {
        info.aliases = a
            .iter()
            .filter_map(Value::as_str)
            .filter(|a| *a != id)
            .map(str::to_owned)
            .collect();
    }
    let flag = |k: &str| m.get(k).and_then(Value::as_bool);
    let set = |k: &str| m.get(k).is_some_and(|x| !x.is_null());
    info.lifecycle = if flag("active") == Some(false) || flag("archived") == Some(true) {
        Lifecycle::Retired
    } else if set("deprecation") || set("expiration_date") {
        Lifecycle::Deprecated
    } else {
        Lifecycle::Active
    };
    Some(info)
}

fn card_capabilities(m: &Value) -> (Option<Vec<Capability>>, Source) {
    use Capability::*;
    // Mistral: a flag per capability.
    if let Some(c) = m.get("capabilities").and_then(Value::as_object) {
        let on = |k: &str| c.get(k).and_then(Value::as_bool) == Some(true);
        let mut caps = Vec::new();
        for (k, cap) in [
            ("completion_chat", Chat),
            ("vision", Vision),
            ("function_calling", Tools),
            ("reasoning", Reasoning),
            ("ocr", Ocr),
            ("audio_transcription", SpeechToText),
            ("audio_speech", TextToSpeech),
            ("moderation", Moderation),
        ] {
            if on(k) {
                caps.push(cap);
            }
        }
        // Classifier-only models (`classification`) screen text.
        if caps.is_empty() && on("classification") {
            caps.push(Moderation);
        }
        // An OCR or transcription model that also says chat is listed by
        // its job: it does not answer chat messages well.
        if caps.contains(&Ocr) || (caps.contains(&SpeechToText) && !on("audio")) {
            caps.retain(|c| *c != Chat && *c != Vision && *c != Tools);
        }
        return (Some(caps), Source::Listing);
    }
    // OpenRouter nests modalities in `architecture`; xAI puts them on top.
    let arch = m.get("architecture").unwrap_or(m);
    let list = |k: &str| -> Option<Vec<String>> {
        arch.get(k).and_then(Value::as_array).map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_ascii_lowercase)
                .collect()
        })
    };
    if let (Some(input), Some(output)) = (list("input_modalities"), list("output_modalities")) {
        let mut caps = Vec::new();
        let out = |k: &str| output.iter().any(|o| o == k);
        if out("text") {
            caps.push(Chat);
            if input.iter().any(|i| i == "image") {
                caps.push(Vision);
            }
        }
        if out("image") {
            caps.push(ImageGen);
        }
        if out("embeddings") || out("embedding") {
            caps.push(Embedding);
        }
        if out("audio") || out("speech") {
            caps.push(TextToSpeech);
        }
        if out("transcription") {
            caps.push(SpeechToText);
        }
        if out("rerank") {
            caps.push(Rerank);
        }
        // A model that makes pictures or sound alongside text is listed
        // by what it makes.
        if caps.contains(&ImageGen) || caps.contains(&TextToSpeech) {
            caps.retain(|c| *c != Chat && *c != Vision);
        }
        if let Some(p) = m.get("supported_parameters").and_then(Value::as_array) {
            let param = |k: &str| p.iter().any(|x| x.as_str() == Some(k));
            if caps.contains(&Chat) && param("tools") {
                caps.push(Tools);
            }
            if caps.contains(&Chat) && (param("reasoning") || param("include_reasoning")) {
                caps.push(Reasoning);
            }
        }
        return (Some(caps), Source::Listing);
    }
    // Together and LM Studio's own API: a `type` word.
    if let Some(t) = str_at(m, &["type"]) {
        let caps = match t.to_ascii_lowercase().as_str() {
            "chat" | "language" | "code" | "llm" => vec![Chat],
            "vlm" => vec![Chat, Vision],
            "image" => vec![ImageGen],
            "embedding" | "embeddings" => vec![Embedding],
            "moderation" => vec![Moderation],
            "rerank" => vec![Rerank],
            "audio" | "tts" => vec![TextToSpeech],
            "transcribe" | "stt" => vec![SpeechToText],
            // Mistral's `base`/`fine-tuned`, OpenAI's `model`: not a role.
            _ => return (None, Source::Table),
        };
        return (Some(caps), Source::Partial);
    }
    (None, Source::Table)
}

/// The cursor for the next page, for servers that page their list the
/// OpenAI way (`has_more` and `last_id`).
pub(crate) fn next_cursor(v: &Value) -> Option<String> {
    if v.get("has_more").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    str_at(v, &["last_id"]).map(str::to_owned)
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

    #[test]
    fn parses_mistral_max_context_length() {
        let v = json!({"object": "list", "data": [{"id": "ministral-8b-latest", "max_context_length": 131072}]});
        assert_eq!(parse_models(&v).unwrap()[0].context_window, Some(131_072));
    }
}
