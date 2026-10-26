//! Streaming replies: a server-sent events parser and one decoder per wire
//! format. Pure code, tested against recorded chunk shapes.
//!
//! The decoders turn each event into a [`Piece`]: text to show now, plus any
//! usage figures or stop reason the provider sends along the way. The
//! client glues the pieces back into one [`ChatResponse`], so the engine sees
//! the same result whether a reply streamed or not.

use serde_json::Value;

use crate::wire::{str_at, u64_at};
use crate::{ChatRequest, ChatResponse, Usage, Wire};

/// One server-sent event: its `event:` name (if any) and joined `data:` lines.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}

/// Splits a byte stream into events. Chunks may cut lines, or UTF-8
/// characters, anywhere; the parser keeps the remainder for the next push.
#[derive(Debug, Default)]
pub struct SseParser {
    buf: Vec<u8>,
    event: Option<String>,
    data: Vec<String>,
}

impl SseParser {
    pub fn push(&mut self, bytes: &[u8]) -> Vec<SseEvent> {
        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        while let Some(nl) = self.buf.iter().position(|b| *b == b'\n') {
            let mut line: Vec<u8> = self.buf.drain(..=nl).collect();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            let line = String::from_utf8_lossy(&line);
            if line.is_empty() {
                if let Some(e) = self.take() {
                    out.push(e);
                }
                continue;
            }
            if line.starts_with(':') {
                continue;
            }
            let (field, value) = match line.split_once(':') {
                Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
                None => (line.as_ref(), ""),
            };
            match field {
                "event" => self.event = Some(value.to_owned()),
                "data" => self.data.push(value.to_owned()),
                _ => {}
            }
        }
        out
    }

    /// Flushes an event the stream ended without a blank line after.
    pub fn finish(&mut self) -> Option<SseEvent> {
        if !self.buf.is_empty() {
            let rest = std::mem::take(&mut self.buf);
            let mut events = self.push(&rest);
            events.extend(self.push(b"\n\n"));
            return events.into_iter().last();
        }
        self.take()
    }

    fn take(&mut self) -> Option<SseEvent> {
        if self.data.is_empty() && self.event.is_none() {
            return None;
        }
        let e = SseEvent {
            event: self.event.take(),
            data: std::mem::take(&mut self.data).join("\n"),
        };
        Some(e)
    }
}

/// What one event adds to the reply.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Piece {
    pub text: String,
    /// Reasoning to show in the Thinking section, never part of the answer
    /// (Anthropic `thinking_delta`, Gemini thought parts, `reasoning_content`
    /// / `reasoning` deltas, Mistral thinking chunks).
    pub reasoning: String,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub finish_reason: Option<String>,
    pub model: Option<String>,
    /// The stream said it is over (`[DONE]`, `message_stop`).
    pub done: bool,
    /// The provider reported an error inside the stream.
    pub error: Option<String>,
    /// Native tool call pieces.
    pub tools: Vec<ToolDelta>,
}

/// A piece of a native tool call in a stream. OpenAI-style providers send
/// the name first and the arguments in fragments, by index; Anthropic
/// sends `input_json_delta` fragments by block index; Gemini sends each
/// call whole.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToolDelta {
    /// Which call; `None` starts a new one (Gemini).
    pub index: Option<usize>,
    pub name: Option<String>,
    /// An arguments fragment (JSON text).
    pub args: String,
    /// Arguments sent whole as an object.
    pub whole: Option<Value>,
    /// The call's id, when the provider sends one. Mistral streams each
    /// parallel call whole under `index` 0 with its own id; a new id at
    /// an index already in use starts a new call.
    pub id: Option<String>,
}

/// Decodes one event for a wire format. Events that carry nothing useful
/// (pings, block starts) give an empty piece.
pub fn decode(wire: Wire, e: &SseEvent) -> Piece {
    let data = e.data.trim();
    if data == "[DONE]" {
        return Piece {
            done: true,
            ..Piece::default()
        };
    }
    let Ok(v) = serde_json::from_str::<Value>(data) else {
        return Piece::default();
    };
    match wire {
        Wire::OpenAiCompatible => openai(&v),
        Wire::Anthropic => anthropic(e.event.as_deref(), &v),
        Wire::Gemini => gemini(&v),
    }
}

fn error_text(v: &Value) -> Option<String> {
    let err = v.get("error")?;
    Some(
        str_at(err, &["message"])
            .map(str::to_owned)
            .unwrap_or_else(|| err.to_string()),
    )
}

fn openai(v: &Value) -> Piece {
    let choice = v.get("choices").and_then(|c| c.get(0));
    let delta = choice.and_then(|c| c.get("delta"));
    let (text, mut reasoning) = match delta.and_then(|d| d.get("content")) {
        Some(Value::String(t)) => (t.clone(), String::new()),
        // Mistral's reasoning models send content as chunks: `text`
        // chunks are the answer, `thinking` chunks hold the reasoning.
        Some(Value::Array(chunks)) => mistral_chunks(chunks),
        _ => (String::new(), String::new()),
    };
    if let Some(d) = delta {
        for key in ["reasoning_content", "reasoning"] {
            if let Some(r) = d.get(key).and_then(Value::as_str) {
                reasoning.push_str(r);
                break;
            }
        }
    }
    Piece {
        text,
        reasoning,
        input_tokens: u64_at(v, &["usage", "prompt_tokens"]),
        output_tokens: u64_at(v, &["usage", "completion_tokens"]),
        finish_reason: choice
            .and_then(|c| str_at(c, &["finish_reason"]))
            .map(str::to_owned),
        model: str_at(v, &["model"]).map(str::to_owned),
        done: false,
        error: error_text(v),
        tools: delta
            .and_then(|d| d.get("tool_calls"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .enumerate()
            .map(|(i, c)| {
                let f = c.get("function");
                let (args, whole) = match f.and_then(|f| f.get("arguments")) {
                    Some(Value::String(s)) => (s.clone(), None),
                    Some(Value::Null) | None => (String::new(), None),
                    Some(v) => (String::new(), Some(v.clone())),
                };
                ToolDelta {
                    index: Some(
                        c.get("index")
                            .and_then(Value::as_u64)
                            .map_or(i, |n| n as usize),
                    ),
                    name: f
                        .and_then(|f| f.get("name"))
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    args,
                    whole,
                    id: c
                        .get("id")
                        .and_then(Value::as_str)
                        .filter(|i| !i.is_empty())
                        .map(str::to_owned),
                }
            })
            .collect(),
    }
}

pub(crate) fn mistral_chunks(chunks: &[Value]) -> (String, String) {
    let mut text = String::new();
    let mut reasoning = String::new();
    for c in chunks {
        match c.get("type").and_then(Value::as_str) {
            Some("text") => {
                text.push_str(c.get("text").and_then(Value::as_str).unwrap_or_default())
            }
            Some("thinking") => match c.get("thinking") {
                Some(Value::Array(parts)) => {
                    for p in parts {
                        reasoning
                            .push_str(p.get("text").and_then(Value::as_str).unwrap_or_default());
                    }
                }
                Some(Value::String(t)) => reasoning.push_str(t),
                _ => {}
            },
            _ => {}
        }
    }
    (text, reasoning)
}

fn anthropic(event: Option<&str>, v: &Value) -> Piece {
    let kind = event.or_else(|| str_at(v, &["type"])).unwrap_or_default();
    let mut p = Piece::default();
    match kind {
        "message_start" => {
            p.input_tokens = u64_at(v, &["message", "usage", "input_tokens"]);
            p.output_tokens = u64_at(v, &["message", "usage", "output_tokens"]);
            p.model = str_at(v, &["message", "model"]).map(str::to_owned);
        }
        "content_block_start" if str_at(v, &["content_block", "type"]) == Some("tool_use") => {
            p.tools.push(ToolDelta {
                index: u64_at(v, &["index"]).map(|n| n as usize),
                name: str_at(v, &["content_block", "name"]).map(str::to_owned),
                ..ToolDelta::default()
            });
        }
        "content_block_delta" => match str_at(v, &["delta", "type"]) {
            Some("input_json_delta") => {
                p.tools.push(ToolDelta {
                    index: u64_at(v, &["index"]).map(|n| n as usize),
                    args: str_at(v, &["delta", "partial_json"])
                        .unwrap_or_default()
                        .to_owned(),
                    ..ToolDelta::default()
                });
            }
            Some("text_delta") => {
                p.text = str_at(v, &["delta", "text"]).unwrap_or_default().to_owned();
            }
            Some("thinking_delta") => {
                p.reasoning = str_at(v, &["delta", "thinking"])
                    .unwrap_or_default()
                    .to_owned();
            }
            _ => {}
        },
        "message_delta" => {
            p.output_tokens = u64_at(v, &["usage", "output_tokens"]);
            p.finish_reason = str_at(v, &["delta", "stop_reason"]).map(str::to_owned);
        }
        "message_stop" => p.done = true,
        "error" => p.error = error_text(v),
        _ => {}
    }
    p
}

fn gemini(v: &Value) -> Piece {
    let cand = v.get("candidates").and_then(|c| c.get(0));
    let parts = cand
        .and_then(|c| c.pointer("/content/parts"))
        .and_then(Value::as_array);
    let pick = |thought: bool| {
        parts
            .map(|parts| {
                parts
                    .iter()
                    // Thought summaries are not part of the answer.
                    .filter(|p| {
                        (p.get("thought").and_then(Value::as_bool) == Some(true)) == thought
                    })
                    .filter_map(|p| p.get("text").and_then(Value::as_str))
                    .collect::<String>()
            })
            .unwrap_or_default()
    };
    Piece {
        text: pick(false),
        reasoning: pick(true),
        input_tokens: u64_at(v, &["usageMetadata", "promptTokenCount"]),
        output_tokens: u64_at(v, &["usageMetadata", "candidatesTokenCount"]),
        finish_reason: cand
            .and_then(|c| str_at(c, &["finishReason"]))
            .map(str::to_owned),
        model: str_at(v, &["modelVersion"]).map(str::to_owned),
        done: false,
        error: error_text(v),
        tools: crate::wire::gemini::function_calls(parts)
            .into_iter()
            .map(|(name, args)| ToolDelta {
                index: None,
                name: Some(name),
                args: String::new(),
                whole: Some(args),
                id: None,
            })
            .collect(),
    }
}

/// Glues pieces into one response.
#[derive(Debug, Default)]
pub struct Assembler {
    text: String,
    reasoning: String,
    input: Option<u64>,
    output: Option<u64>,
    finish: Option<String>,
    model: Option<String>,
    /// Native tool calls by slot: name, argument text, whole arguments.
    tools: std::collections::BTreeMap<usize, (String, String, Option<Value>)>,
    /// The slot each wire index writes to now, and the id seen there.
    slots: std::collections::HashMap<usize, (usize, Option<String>)>,
}

impl Assembler {
    /// Adds a piece; later usage figures replace earlier ones, since
    /// providers send running totals.
    pub fn add(&mut self, p: &Piece) {
        self.text.push_str(&p.text);
        self.reasoning.push_str(&p.reasoning);
        if p.input_tokens.is_some() {
            self.input = p.input_tokens;
        }
        if p.output_tokens.is_some() {
            self.output = p.output_tokens;
        }
        if p.finish_reason.is_some() {
            self.finish.clone_from(&p.finish_reason);
        }
        if p.model.is_some() {
            self.model.clone_from(&p.model);
        }
        for t in &p.tools {
            let next = self.tools.keys().next_back().map_or(0, |k| k + 1);
            let i = match t.index {
                None => next,
                Some(wire) => match self.slots.get(&wire) {
                    // Another call under the same index (Mistral's
                    // parallel calls all say index 0): a new slot.
                    Some((_, Some(seen))) if t.id.as_ref().is_some_and(|id| id != seen) => {
                        self.slots.insert(wire, (next, t.id.clone()));
                        next
                    }
                    Some((slot, _)) => *slot,
                    None => {
                        let slot = if self.tools.contains_key(&wire) {
                            next
                        } else {
                            wire
                        };
                        self.slots.insert(wire, (slot, t.id.clone()));
                        slot
                    }
                },
            };
            let e = self.tools.entry(i).or_default();
            if let Some(n) = &t.name {
                e.0.push_str(n);
            }
            e.1.push_str(&t.args);
            if t.whole.is_some() {
                e.2.clone_from(&t.whole);
            }
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// The reasoning streamed so far.
    pub fn reasoning(&self) -> &str {
        &self.reasoning
    }

    pub fn finish(self, req: &ChatRequest, stopped: bool) -> ChatResponse {
        let usage = match (self.input, self.output) {
            (Some(i), Some(o)) => Usage::measured(i, o),
            _ => Usage::estimated(req.prompt_chars(), self.text.len()),
        };
        ChatResponse {
            usage,
            finish_reason: if stopped {
                Some("cancelled".into())
            } else {
                self.finish
            },
            model: self.model,
            text: crate::chat::with_tool_calls(
                self.text,
                &self
                    .tools
                    .into_values()
                    .map(|(name, args, whole)| (name, whole.unwrap_or(Value::String(args))))
                    .collect::<Vec<_>>(),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ChatMessage;
    use tern_core::Measure;

    fn reasoning_of(wire: Wire, raw: &str) -> (String, String) {
        let mut parser = SseParser::default();
        let mut events = parser.push(raw.as_bytes());
        events.extend(parser.finish());
        let mut asm = Assembler::default();
        for e in &events {
            asm.add(&decode(wire, e));
        }
        (asm.reasoning().to_owned(), asm.text().to_owned())
    }

    #[test]
    fn anthropic_thinking_deltas_are_reasoning() {
        let raw = concat!(
            "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"Two plus \"}}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"two.\"}}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"EqQB\"}}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"4\"}}\n\n",
        );
        assert_eq!(
            reasoning_of(Wire::Anthropic, raw),
            ("Two plus two.".into(), "4".into())
        );
    }

    #[test]
    fn openai_style_reasoning_fields() {
        // DeepSeek, xAI: reasoning_content. OpenRouter, Groq parsed, Ollama: reasoning.
        let raw = concat!(
            "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"Think\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"reasoning\":\"ing.\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"Done.\"}}]}\n\n",
            "data: [DONE]\n\n",
        );
        assert_eq!(
            reasoning_of(Wire::OpenAiCompatible, raw),
            ("Thinking.".into(), "Done.".into())
        );
    }

    #[test]
    fn mistral_thinking_chunks() {
        let raw = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":[{\"type\":\"thinking\",\"thinking\":[{\"type\":\"text\",\"text\":\"Hmm, \"}]}]}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":[{\"type\":\"thinking\",\"thinking\":[{\"type\":\"text\",\"text\":\"paging.\"}]}]}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":[{\"type\":\"text\",\"text\":\"Paging maps \"}]}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"pages.\"}}]}\n\n",
        );
        assert_eq!(
            reasoning_of(Wire::OpenAiCompatible, raw),
            ("Hmm, paging.".into(), "Paging maps pages.".into())
        );
    }

    #[test]
    fn gemini_thought_parts_are_reasoning() {
        let raw = concat!(
            "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"**Planning** the reply\",\"thought\":true}]}}]}\n\n",
            "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Paris.\"}]}}]}\n\n",
        );
        assert_eq!(
            reasoning_of(Wire::Gemini, raw),
            ("**Planning** the reply".into(), "Paris.".into())
        );
    }

    fn run(wire: Wire, raw: &str, chunk: usize) -> (ChatResponse, Vec<String>) {
        let mut parser = SseParser::default();
        let mut asm = Assembler::default();
        let mut deltas = Vec::new();
        let bytes = raw.as_bytes();
        let mut events = Vec::new();
        for c in bytes.chunks(chunk) {
            events.extend(parser.push(c));
        }
        events.extend(parser.finish());
        for e in &events {
            let p = decode(wire, e);
            if !p.text.is_empty() {
                deltas.push(p.text.clone());
            }
            asm.add(&p);
        }
        let req = ChatRequest::new("m", vec![ChatMessage::user("hi")]);
        (asm.finish(&req, false), deltas)
    }

    #[test]
    fn parser_survives_any_chunking_and_crlf() {
        let raw = "event: a\r\ndata: one\r\ndata: two\r\n\r\n: comment\n\ndata: é ok\n\ndata: tail";
        for size in [1, 2, 3, 7, 100] {
            let mut p = SseParser::default();
            let mut ev = Vec::new();
            for c in raw.as_bytes().chunks(size) {
                ev.extend(p.push(c));
            }
            ev.extend(p.finish());
            assert_eq!(ev.len(), 3, "size {size}: {ev:?}");
            assert_eq!(ev[0].event.as_deref(), Some("a"));
            assert_eq!(ev[0].data, "one\ntwo");
            assert_eq!(ev[1].data, "é ok");
            assert_eq!(ev[2].data, "tail");
        }
    }

    #[test]
    fn openai_stream_assembles_text_and_usage() {
        let raw = concat!(
            "data: {\"model\":\"gpt-4.1-mini\",\"choices\":[{\"delta\":{\"role\":\"assistant\",\"content\":\"\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"lo.\"},\"finish_reason\":\"stop\"}]}\n\n",
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":9,\"completion_tokens\":2}}\n\n",
            "data: [DONE]\n\n"
        );
        let (r, deltas) = run(Wire::OpenAiCompatible, raw, 5);
        assert_eq!(deltas, ["Hel", "lo."]);
        assert_eq!(r.text, "Hello.");
        assert_eq!(r.usage, Usage::measured(9, 2));
        assert_eq!(r.finish_reason.as_deref(), Some("stop"));
        assert_eq!(r.model.as_deref(), Some("gpt-4.1-mini"));
    }

    #[test]
    fn anthropic_stream_uses_event_names() {
        let raw = concat!(
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"model\":\"claude-x\",\"usage\":{\"input_tokens\":20,\"output_tokens\":1}}}\n\n",
            "event: ping\ndata: {\"type\":\"ping\"}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"Hi\"}}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\" there\"}}\n\n",
            "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":4}}\n\n",
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"
        );
        let (r, deltas) = run(Wire::Anthropic, raw, 11);
        assert_eq!(deltas, ["Hi", " there"]);
        assert_eq!(r.usage, Usage::measured(20, 4));
        assert_eq!(r.finish_reason.as_deref(), Some("end_turn"));
    }

    #[test]
    fn gemini_stream_skips_thoughts() {
        let raw = concat!(
            "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"plan\",\"thought\":true},{\"text\":\"Par\"}]}}]}\n\n",
            "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"is.\"}]},\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":5,\"candidatesTokenCount\":2},\"modelVersion\":\"gemini-2.5-flash\"}\n\n"
        );
        let (r, deltas) = run(Wire::Gemini, raw, 4);
        assert_eq!(deltas, ["Par", "is."]);
        assert_eq!(r.text, "Paris.");
        assert_eq!(r.usage, Usage::measured(5, 2));
        assert_eq!(r.finish_reason.as_deref(), Some("STOP"));
    }

    #[test]
    fn missing_usage_is_estimated_and_errors_surface() {
        let (r, _) = run(
            Wire::OpenAiCompatible,
            "data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}]}\n\n",
            64,
        );
        assert_eq!(r.usage.measure, Measure::Estimated);
        let e = SseEvent {
            event: None,
            data: "{\"error\":{\"message\":\"overloaded\"}}".into(),
        };
        assert_eq!(
            decode(Wire::OpenAiCompatible, &e).error.as_deref(),
            Some("overloaded")
        );
    }
}
