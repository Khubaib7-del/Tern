//! Thinking effort: one six-step scale for the user, mapped onto each
//! provider's own knob.
//!
//! | Provider | Models | Knob |
//! | --- | --- | --- |
//! | OpenAI | o1, o3, o4, gpt-5 (not `-chat`) | `reasoning_effort` (`minimal`/`none`, `low`, `medium`, `high`, `xhigh` where the model takes it); `max_completion_tokens` instead of `max_tokens` |
//! | Anthropic | Claude 3.7 Sonnet, Claude 4 family | `thinking: {type: enabled, budget_tokens}`; `max_tokens` grows by the budget |
//! | Google AI Studio | Gemini 2.5 | `thinkingConfig.thinkingBudget` (0 turns it off on Flash; Pro cannot turn it off) with `includeThoughts` |
//! | Google AI Studio | Gemini 3 | `thinkingConfig.thinkingLevel` (`low`, `high`) |
//! | OpenRouter | reasoning models | `reasoning: {effort}` or `reasoning: {enabled: false}` |
//! | xAI | grok-3-mini | `reasoning_effort` (`low`, `high`) |
//! | Groq | gpt-oss | `reasoning_effort` (`low`, `medium`, `high`) |
//! | Groq | qwen3 | `reasoning_effort` (`none`, `default`), `reasoning_format: parsed` |
//! | DeepSeek, Mistral, Groq, xAI, local and hosted open models | deepseek-reasoner, magistral, r1, qwq, grok-4… | none: these always think, so the menu says so |
//!
//! Every other model has no thinking knob and the composer hides the menu.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{ChatRequest, ProviderSpec, Wire};

/// How hard the model should think before it answers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effort {
    None,
    Low,
    #[default]
    Medium,
    High,
    #[serde(rename = "xhigh")]
    XHigh,
    Max,
}

impl Effort {
    pub const ALL: [Effort; 6] = [
        Self::None,
        Self::Low,
        Self::Medium,
        Self::High,
        Self::XHigh,
        Self::Max,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
            Self::XHigh => "X-high",
            Self::Max => "Max",
        }
    }

    /// How many of the meter's five bars are lit (0 for None, 5 for Max).
    pub fn bars(self) -> u8 {
        match self {
            Self::None => 0,
            Self::Low => 1,
            Self::Medium => 2,
            Self::High => 3,
            Self::XHigh => 4,
            Self::Max => 5,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
            Self::Max => "max",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|e| e.id().eq_ignore_ascii_case(s.trim()) || e.label().eq_ignore_ascii_case(s))
    }
}

/// What a model offers for thinking.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Thinking {
    /// No knob and no visible reasoning: the menu is hidden.
    Unsupported,
    /// The model always reasons; there is nothing to set.
    Always,
    /// OpenAI `reasoning_effort`. `off` is the word for the lowest step
    /// (`minimal` on gpt-5, `none` on gpt-5.1 and later, absent on o-series);
    /// `xhigh` when the model takes it.
    OpenAi {
        off: Option<&'static str>,
        xhigh: bool,
    },
    /// Anthropic extended thinking with a token budget, capped by the
    /// model's output limit.
    AnthropicBudget { output_cap: u32 },
    /// Gemini 2.5 `thinkingBudget`.
    GeminiBudget {
        can_disable: bool,
        min: u32,
        max: u32,
    },
    /// Gemini 3 `thinkingLevel`.
    GeminiLevel,
    /// OpenRouter's unified `reasoning` object.
    OpenRouter,
    /// xAI grok-3-mini: `low` or `high`.
    XaiMini,
    /// Groq gpt-oss: `low`, `medium`, `high`.
    GroqOss,
    /// Groq qwen3: on (`default`) or off (`none`).
    GroqQwen,
}

impl Thinking {
    /// The menu is shown.
    pub fn visible(self) -> bool {
        self != Self::Unsupported
    }

    /// The menu's steps can be chosen.
    pub fn adjustable(self) -> bool {
        !matches!(self, Self::Unsupported | Self::Always)
    }

    /// Whether `None` really turns thinking off.
    pub fn can_turn_off(self) -> bool {
        match self {
            Self::Unsupported | Self::Always => false,
            Self::OpenAi { off, .. } => off.is_some(),
            Self::GeminiBudget { can_disable, .. } => can_disable,
            Self::GeminiLevel | Self::XaiMini | Self::GroqOss => false,
            Self::AnthropicBudget { .. } | Self::OpenRouter | Self::GroqQwen => true,
        }
    }

    /// What `effort` sends, in the provider's words, for the menu's
    /// second line (`reasoning_effort: high`, `budget 12,000 tokens`).
    pub fn knob(self, effort: Effort) -> String {
        match self.value(effort) {
            Knob::Nothing => match self {
                Self::Always => "always on".into(),
                _ => "not supported".into(),
            },
            Knob::Off => "thinking off".into(),
            Knob::Word(field, w) => format!("{field}: {w}"),
            Knob::Budget(field, n) => format!("{field} {} tokens", grouped(n)),
        }
    }

    fn value(self, e: Effort) -> Knob {
        match self {
            Self::Unsupported | Self::Always => Knob::Nothing,
            Self::OpenAi { off, xhigh } => Knob::Word(
                "reasoning_effort",
                match e {
                    Effort::None => off.unwrap_or("low"),
                    Effort::Low => "low",
                    Effort::Medium => "medium",
                    Effort::High => "high",
                    Effort::XHigh | Effort::Max if xhigh => "xhigh",
                    Effort::XHigh | Effort::Max => "high",
                },
            ),
            Self::AnthropicBudget { output_cap } => match e {
                Effort::None => Knob::Off,
                _ => Knob::Budget(
                    "budget",
                    anthropic_budget(e).min(output_cap.saturating_sub(1024).max(1024)),
                ),
            },
            Self::GeminiBudget {
                can_disable,
                min,
                max,
            } => {
                let n = match e {
                    Effort::None if can_disable => return Knob::Off,
                    Effort::None => min,
                    Effort::Low => 1024,
                    Effort::Medium => 8192,
                    Effort::High => 16_384,
                    Effort::XHigh => 24_576,
                    Effort::Max => max,
                };
                Knob::Budget("thinkingBudget", n.clamp(min, max))
            }
            Self::GeminiLevel => Knob::Word(
                "thinkingLevel",
                match e {
                    Effort::None | Effort::Low => "low",
                    _ => "high",
                },
            ),
            Self::OpenRouter => match e {
                Effort::None => Knob::Off,
                Effort::Low => Knob::Word("reasoning.effort", "low"),
                Effort::Medium => Knob::Word("reasoning.effort", "medium"),
                _ => Knob::Word("reasoning.effort", "high"),
            },
            Self::XaiMini => Knob::Word(
                "reasoning_effort",
                match e {
                    Effort::None | Effort::Low | Effort::Medium => "low",
                    _ => "high",
                },
            ),
            Self::GroqOss => Knob::Word(
                "reasoning_effort",
                match e {
                    Effort::None | Effort::Low => "low",
                    Effort::Medium => "medium",
                    _ => "high",
                },
            ),
            Self::GroqQwen => match e {
                Effort::None => Knob::Word("reasoning_effort", "none"),
                _ => Knob::Word("reasoning_effort", "default"),
            },
        }
    }
}

enum Knob {
    Nothing,
    Off,
    Word(&'static str, &'static str),
    Budget(&'static str, u32),
}

fn anthropic_budget(e: Effort) -> u32 {
    match e {
        Effort::None => 0,
        Effort::Low => 1024,
        Effort::Medium => 4096,
        Effort::High => 12_000,
        Effort::XHigh => 24_000,
        Effort::Max => 32_000,
    }
}

fn grouped(n: u32) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// OpenAI's reasoning models (they also want `max_completion_tokens`).
fn openai_reasoning(m: &str) -> bool {
    let m = m.rsplit('/').next().unwrap_or(m);
    (m.starts_with("o1") || m.starts_with("o3") || m.starts_with("o4") || m.starts_with("gpt-5"))
        && !m.contains("-chat")
        && !m.contains("audio")
        && !m.contains("realtime")
}

/// Ids of open and hosted models that always reason (and stream it as
/// `reasoning_content`, `reasoning`, or `<think>` tags).
fn always_thinks(m: &str) -> bool {
    [
        "deepseek-r1",
        "deepseek-reasoner",
        "qwq",
        "magistral",
        "-thinking",
        "thinking-",
        "reasoning",
        "gpt-oss",
        "qwen3",
        "phi-4-reasoning",
        "grok-4",
        "grok-code",
        "kimi-k2-thinking",
    ]
    .iter()
    .any(|k| m.contains(k))
}

/// Models OpenRouter can reason with, by their upstream names.
fn openrouter_reasoning(m: &str) -> bool {
    let bare = m.rsplit('/').next().unwrap_or(m);
    openai_reasoning(bare)
        || anthropic_thinks(bare)
        || bare.starts_with("gemini-2.5")
        || bare.starts_with("gemini-3")
        || bare.starts_with("grok-3-mini")
        || always_thinks(bare)
}

fn anthropic_thinks(m: &str) -> bool {
    m.starts_with("claude-3-7")
        || m.starts_with("claude-sonnet-4")
        || m.starts_with("claude-opus-4")
        || m.starts_with("claude-haiku-4")
        || m.starts_with("claude-4")
        || m.contains("claude-3.7")
        || m.contains("claude-sonnet-4")
        || m.contains("claude-opus-4")
        || m.contains("claude-haiku-4")
}

/// What `model` on `provider` (a catalog id) offers for thinking.
pub fn thinking_support(provider: &str, model: &str) -> Thinking {
    let m = model.to_ascii_lowercase();
    let m = m.strip_prefix("models/").unwrap_or(&m);
    match provider {
        "openai" if openai_reasoning(m) => {
            let newer = m.starts_with("gpt-5.") || m.starts_with("gpt-6");
            let off = if m.starts_with("gpt-5") {
                Some(if newer { "none" } else { "minimal" })
            } else {
                None
            };
            let xhigh =
                m.starts_with("gpt-5.2") || m.contains("codex-max") || m.starts_with("gpt-6");
            Thinking::OpenAi { off, xhigh }
        }
        "anthropic" if anthropic_thinks(m) => Thinking::AnthropicBudget {
            output_cap: if m.contains("opus-4-0") || m == "claude-opus-4" || m.contains("opus-4-1")
            {
                32_000
            } else {
                64_000
            },
        },
        "google" | "gemini" if m.starts_with("gemini-2.5-pro") => Thinking::GeminiBudget {
            can_disable: false,
            min: 128,
            max: 32_768,
        },
        "google" | "gemini" if m.starts_with("gemini-2.5") => Thinking::GeminiBudget {
            can_disable: true,
            min: 0,
            max: 24_576,
        },
        "google" | "gemini" if m.starts_with("gemini-3") => Thinking::GeminiLevel,
        "openrouter" if openrouter_reasoning(m) => Thinking::OpenRouter,
        "xai" if m.starts_with("grok-3-mini") => Thinking::XaiMini,
        "groq" if m.contains("gpt-oss") => Thinking::GroqOss,
        "groq" if m.contains("qwen3") => Thinking::GroqQwen,
        "deepseek" if m.contains("reasoner") => Thinking::Always,
        _ if always_thinks(m) => Thinking::Always,
        _ => Thinking::Unsupported,
    }
}

/// Writes the effort (and the request-shape fixes reasoning models need)
/// into a request body built for `spec`'s wire.
pub(crate) fn apply(body: &mut Value, spec: &ProviderSpec, req: &ChatRequest) {
    let support = thinking_support(spec.id, &req.model);
    let effort = req.effort;
    match (spec.wire, support) {
        (Wire::OpenAiCompatible, Thinking::OpenAi { .. }) => {
            // Reasoning models take max_completion_tokens and no temperature.
            if let Some(obj) = body.as_object_mut() {
                if let Some(n) = obj.remove("max_tokens") {
                    obj.insert("max_completion_tokens".into(), n);
                }
                obj.remove("temperature");
            }
            if let Some(e) = effort
                && let Knob::Word(_, w) = support.value(e)
            {
                body["reasoning_effort"] = json!(w);
            }
        }
        (Wire::OpenAiCompatible, Thinking::OpenRouter) => match effort.map(|e| support.value(e)) {
            Some(Knob::Off) => body["reasoning"] = json!({"enabled": false}),
            Some(Knob::Word(_, w)) => body["reasoning"] = json!({"effort": w}),
            _ => {}
        },
        (Wire::OpenAiCompatible, Thinking::XaiMini | Thinking::GroqOss) => {
            if let Some(e) = effort
                && let Knob::Word(_, w) = support.value(e)
            {
                body["reasoning_effort"] = json!(w);
            }
        }
        (Wire::OpenAiCompatible, Thinking::GroqQwen) => {
            body["reasoning_format"] = json!("parsed");
            if let Some(e) = effort
                && let Knob::Word(_, w) = support.value(e)
            {
                body["reasoning_effort"] = json!(w);
            }
        }
        (Wire::OpenAiCompatible, Thinking::Always) if spec.id == "groq" => {
            body["reasoning_format"] = json!("parsed");
        }
        (Wire::Anthropic, Thinking::AnthropicBudget { output_cap }) => {
            if let Some(e) = effort
                && let Knob::Budget(_, budget) = support.value(e)
            {
                let max = (req.max_tokens.saturating_add(budget)).min(output_cap);
                body["max_tokens"] = json!(max.max(budget + 1024));
                body["thinking"] = json!({"type": "enabled", "budget_tokens": budget});
                // Extended thinking does not take a temperature.
                if let Some(obj) = body.as_object_mut() {
                    obj.remove("temperature");
                }
            }
        }
        (Wire::Gemini, Thinking::GeminiBudget { min, .. }) => {
            if let Some(e) = effort {
                let cfg = match support.value(e) {
                    Knob::Off => json!({"thinkingBudget": 0}),
                    Knob::Budget(_, n) => {
                        json!({"thinkingBudget": n.max(min), "includeThoughts": true})
                    }
                    _ => return,
                };
                body["generationConfig"]["thinkingConfig"] = cfg;
            }
        }
        (Wire::Gemini, Thinking::GeminiLevel) => {
            if let Some(e) = effort
                && let Knob::Word(_, w) = support.value(e)
            {
                body["generationConfig"]["thinkingConfig"] =
                    json!({"thinkingLevel": w, "includeThoughts": true});
            }
        }
        _ => {}
    }
}

/// Splits `<think>…</think>` at the start of a reply off as reasoning, for
/// local and hosted open models that write their thinking inline. Text
/// arrives in pieces cut anywhere, so it holds back what could still turn
/// out to be a tag.
#[derive(Debug, Default)]
pub struct ThinkSplitter {
    state: SplitState,
    held: String,
}

#[derive(Debug, Default, PartialEq, Eq)]
enum SplitState {
    /// Nothing but whitespace yet: a `<think>` may still open.
    #[default]
    Start,
    Inside,
    /// Just past `</think>`: blank lines before the answer are dropped.
    After,
    /// Past the thinking (or there was none): everything is answer.
    Answer,
}

const OPEN: &str = "<think>";
const CLOSE: &str = "</think>";

impl ThinkSplitter {
    /// Takes a piece of streamed text; returns (reasoning, answer) to show now.
    pub fn push(&mut self, text: &str) -> (String, String) {
        let mut reasoning = String::new();
        let mut answer = String::new();
        self.held.push_str(text);
        loop {
            match self.state {
                SplitState::Answer => {
                    answer.push_str(&std::mem::take(&mut self.held));
                    break;
                }
                SplitState::After => {
                    let t = self.held.trim_start();
                    if t.is_empty() {
                        self.held.clear();
                        break;
                    }
                    self.held = t.to_owned();
                    self.state = SplitState::Answer;
                }
                SplitState::Start => {
                    let t = self.held.trim_start();
                    if t.is_empty() {
                        break;
                    }
                    if let Some(rest) = t.strip_prefix(OPEN) {
                        self.held = rest.to_owned();
                        self.state = SplitState::Inside;
                    } else if OPEN.starts_with(t) {
                        break;
                    } else {
                        self.state = SplitState::Answer;
                    }
                }
                SplitState::Inside => {
                    if let Some(at) = self.held.find(CLOSE) {
                        reasoning.push_str(&self.held[..at]);
                        let rest = self.held[at + CLOSE.len()..].to_owned();
                        self.held = rest;
                        self.state = SplitState::After;
                        continue;
                    }
                    // Keep back a tail that could be the start of `</think>`.
                    let keep = (1..CLOSE.len())
                        .rev()
                        .find(|n| self.held.ends_with(&CLOSE[..*n]))
                        .unwrap_or(0);
                    let cut = self.held.len() - keep;
                    reasoning.push_str(&self.held[..cut]);
                    self.held.drain(..cut);
                    break;
                }
            }
        }
        (reasoning, answer)
    }

    /// The stream ended: whatever is held back.
    pub fn finish(&mut self) -> (String, String) {
        let held = std::mem::take(&mut self.held);
        match self.state {
            SplitState::Inside => (held, String::new()),
            _ => (String::new(), held),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CATALOG, ChatMessage, find};

    fn body_for(provider: &str, model: &str, effort: Option<Effort>) -> Value {
        let spec = find(provider).expect("in catalog");
        let mut req = ChatRequest::new(model, vec![ChatMessage::user("hi")]).with_max_tokens(8192);
        req.effort = effort;
        let mut body = match spec.wire {
            Wire::OpenAiCompatible => crate::wire::openai::chat_body(&req),
            Wire::Anthropic => crate::wire::anthropic::chat_body(&req),
            Wire::Gemini => crate::wire::gemini::chat_body(&req),
        };
        apply(&mut body, spec, &req);
        body
    }

    #[test]
    fn scale_round_trips_and_has_six_steps() {
        assert_eq!(Effort::ALL.len(), 6);
        for e in Effort::ALL {
            assert_eq!(Effort::parse(e.id()), Some(e));
            assert_eq!(Effort::parse(e.label()), Some(e));
        }
        assert_eq!(Effort::None.bars(), 0);
        assert_eq!(Effort::Max.bars(), 5);
        assert_eq!(serde_json::to_string(&Effort::XHigh).unwrap(), "\"xhigh\"");
    }

    #[test]
    fn openai_reasoning_effort_and_completion_tokens() {
        let b = body_for("openai", "o4-mini", Some(Effort::High));
        assert_eq!(b["reasoning_effort"], "high");
        assert_eq!(b["max_completion_tokens"], 8192);
        assert!(b.get("max_tokens").is_none());
        // o-series has no "off": None asks for low.
        let b = body_for("openai", "o3", Some(Effort::None));
        assert_eq!(b["reasoning_effort"], "low");
        assert_eq!(
            body_for("openai", "gpt-5", Some(Effort::None))["reasoning_effort"],
            "minimal"
        );
        assert_eq!(
            body_for("openai", "gpt-5.1", Some(Effort::None))["reasoning_effort"],
            "none"
        );
        assert_eq!(
            body_for("openai", "gpt-5", Some(Effort::Max))["reasoning_effort"],
            "high"
        );
        assert_eq!(
            body_for("openai", "gpt-5.2", Some(Effort::Max))["reasoning_effort"],
            "xhigh"
        );
        // Plain chat models get nothing.
        let b = body_for("openai", "gpt-4.1-mini", Some(Effort::High));
        assert!(b.get("reasoning_effort").is_none());
        assert_eq!(b["max_tokens"], 8192);
        assert_eq!(
            thinking_support("openai", "gpt-5-chat-latest"),
            Thinking::Unsupported
        );
    }

    #[test]
    fn anthropic_budget_grows_max_tokens() {
        let b = body_for("anthropic", "claude-sonnet-4-5", Some(Effort::High));
        assert_eq!(b["thinking"]["type"], "enabled");
        assert_eq!(b["thinking"]["budget_tokens"], 12_000);
        assert_eq!(b["max_tokens"], 8192 + 12_000);
        let b = body_for("anthropic", "claude-opus-4-1", Some(Effort::Max));
        assert!(b["max_tokens"].as_u64().unwrap() <= 32_000);
        assert!(b["thinking"]["budget_tokens"].as_u64().unwrap() < 32_000);
        assert!(
            body_for("anthropic", "claude-sonnet-4-5", Some(Effort::None))
                .get("thinking")
                .is_none()
        );
        assert_eq!(
            thinking_support("anthropic", "claude-3-5-haiku-latest"),
            Thinking::Unsupported
        );
    }

    #[test]
    fn gemini_budget_and_level() {
        let b = body_for("google", "gemini-2.5-flash", Some(Effort::None));
        assert_eq!(b["generationConfig"]["thinkingConfig"]["thinkingBudget"], 0);
        let b = body_for("google", "gemini-2.5-pro", Some(Effort::None));
        assert_eq!(
            b["generationConfig"]["thinkingConfig"]["thinkingBudget"],
            128
        );
        let b = body_for("google", "gemini-2.5-flash", Some(Effort::Medium));
        assert_eq!(
            b["generationConfig"]["thinkingConfig"]["thinkingBudget"],
            8192
        );
        assert_eq!(
            b["generationConfig"]["thinkingConfig"]["includeThoughts"],
            true
        );
        assert_eq!(b["generationConfig"]["maxOutputTokens"], 8192);
        let b = body_for("google", "gemini-3-pro-preview", Some(Effort::High));
        assert_eq!(
            b["generationConfig"]["thinkingConfig"]["thinkingLevel"],
            "high"
        );
    }

    #[test]
    fn openrouter_xai_groq() {
        let b = body_for(
            "openrouter",
            "anthropic/claude-sonnet-4.5",
            Some(Effort::Low),
        );
        assert_eq!(b["reasoning"]["effort"], "low");
        let b = body_for("openrouter", "deepseek/deepseek-r1", Some(Effort::None));
        assert_eq!(b["reasoning"]["enabled"], false);
        assert_eq!(
            thinking_support("openrouter", "meta-llama/llama-3.3-70b-instruct"),
            Thinking::Unsupported
        );
        assert_eq!(
            body_for("xai", "grok-3-mini", Some(Effort::Max))["reasoning_effort"],
            "high"
        );
        assert_eq!(
            body_for("groq", "openai/gpt-oss-120b", Some(Effort::Medium))["reasoning_effort"],
            "medium"
        );
        let b = body_for("groq", "qwen/qwen3-32b", Some(Effort::None));
        assert_eq!(b["reasoning_effort"], "none");
        assert_eq!(b["reasoning_format"], "parsed");
    }

    #[test]
    fn always_on_models_and_plain_ones() {
        assert_eq!(
            thinking_support("deepseek", "deepseek-reasoner"),
            Thinking::Always
        );
        assert_eq!(
            thinking_support("deepseek", "deepseek-chat"),
            Thinking::Unsupported
        );
        assert_eq!(
            thinking_support("mistral", "magistral-medium-latest"),
            Thinking::Always
        );
        assert_eq!(
            thinking_support("mistral", "ministral-8b-latest"),
            Thinking::Unsupported
        );
        assert_eq!(
            thinking_support("ollama", "deepseek-r1:8b"),
            Thinking::Always
        );
        assert!(!Thinking::Always.adjustable());
        assert!(Thinking::Always.visible());
        assert!(!Thinking::Unsupported.visible());
        // No effort set: bodies are unchanged apart from reasoning-model fixes.
        let b = body_for("deepseek", "deepseek-reasoner", Some(Effort::High));
        assert!(b.get("reasoning_effort").is_none());
        // Every catalog provider answers without panicking.
        for s in CATALOG {
            let _ = thinking_support(s.id, "some-model");
        }
    }

    #[test]
    fn knob_lines_name_the_provider_setting() {
        let t = thinking_support("anthropic", "claude-sonnet-4-5");
        assert_eq!(t.knob(Effort::High), "budget 12,000 tokens");
        assert_eq!(t.knob(Effort::None), "thinking off");
        assert_eq!(
            thinking_support("openai", "o3").knob(Effort::Low),
            "reasoning_effort: low"
        );
        assert_eq!(Thinking::Always.knob(Effort::Low), "always on");
    }

    #[test]
    fn think_tags_split_across_any_chunking() {
        let raw = "  <think>Let me see.\nPlan: 2+2.</think>\n\nThe answer is 4.";
        for size in [1, 2, 3, 5, 8, 100] {
            let mut s = ThinkSplitter::default();
            let (mut r, mut a) = (String::new(), String::new());
            let chars: Vec<char> = raw.chars().collect();
            for c in chars.chunks(size) {
                let piece: String = c.iter().collect();
                let (x, y) = s.push(&piece);
                r.push_str(&x);
                a.push_str(&y);
            }
            let (x, y) = s.finish();
            r.push_str(&x);
            a.push_str(&y);
            assert_eq!(r, "Let me see.\nPlan: 2+2.", "size {size}");
            assert_eq!(a, "The answer is 4.", "size {size}");
        }
        // No tag: all answer, nothing held for long.
        let mut s = ThinkSplitter::default();
        assert_eq!(
            s.push("Hello <think> later"),
            (String::new(), "Hello <think> later".into())
        );
    }
}
