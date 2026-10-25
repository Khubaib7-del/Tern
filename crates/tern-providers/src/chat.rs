//! Provider-neutral chat types.

use serde::{Deserialize, Serialize};

use crate::Usage;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

/// A picture sent to the model with a message (vision input).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageInput {
    /// The file name, for chips and notes.
    pub name: String,
    /// `image/png`, `image/jpeg`, `image/gif`, or `image/webp`.
    pub media_type: String,
    /// The bytes, base64-encoded.
    pub data: String,
}

impl ImageInput {
    /// `data:image/png;base64,...`, the form OpenAI-style APIs take.
    pub fn data_url(&self) -> String {
        format!("data:{};base64,{}", self.media_type, self.data)
    }
}

/// Roughly what a picture costs in prompt characters (about 1,500 tokens),
/// for estimates when a provider reports no usage.
pub const IMAGE_CHARS: usize = 6_000;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
    /// Pictures that go with this message. Only user messages carry them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<ImageInput>,
}

impl ChatMessage {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
            images: Vec::new(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            images: Vec::new(),
        }
    }

    /// The same message with pictures attached.
    pub fn with_images(mut self, images: Vec<ImageInput>) -> Self {
        self.images = images;
        self
    }
}

/// A tool the model may call natively (the provider's function calling).
#[derive(Clone, Debug, PartialEq)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// The input as a JSON Schema object.
    pub parameters: serde_json::Value,
}

/// A native tool call written the way Tern's text protocol asks for a tool
/// (a fenced `tool` block), so the turn loop reads both the same way.
pub fn tool_block(name: &str, args: &serde_json::Value) -> String {
    let args = match args {
        // OpenAI-style providers send the arguments as a JSON string.
        serde_json::Value::String(s) if s.trim().is_empty() => serde_json::json!({}),
        serde_json::Value::String(s) => {
            serde_json::from_str(s).unwrap_or_else(|_| serde_json::json!({}))
        }
        serde_json::Value::Null => serde_json::json!({}),
        v => v.clone(),
    };
    format!(
        "```tool\n{}\n```",
        serde_json::json!({"tool": name, "args": args})
    )
}

/// Appends native tool calls to a reply's text as `tool` blocks.
pub(crate) fn with_tool_calls(mut text: String, calls: &[(String, serde_json::Value)]) -> String {
    for (name, args) in calls {
        if name.is_empty() {
            continue;
        }
        if !text.is_empty() {
            text.push_str("\n");
        }
        text.push_str(&tool_block(name, args));
    }
    text
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChatRequest {
    pub model: String,
    pub system: Option<String>,
    pub messages: Vec<ChatMessage>,
    pub max_tokens: u32,
    pub temperature: Option<f32>,
    /// How hard to think, for models that can (see [`crate::effort`]).
    /// `None` leaves the provider's default.
    pub effort: Option<crate::Effort>,
    /// Tools offered for native calling; empty sends none. A call comes
    /// back in the reply's text as a `tool` block ([`tool_block`]).
    pub tools: Vec<ToolSpec>,
}

impl ChatRequest {
    pub fn new(model: impl Into<String>, messages: Vec<ChatMessage>) -> Self {
        Self {
            model: model.into(),
            system: None,
            messages,
            max_tokens: 1024,
            temperature: None,
            effort: None,
            tools: Vec::new(),
        }
    }

    /// Offers `tools` for native calling.
    pub fn with_tools(mut self, tools: Vec<ToolSpec>) -> Self {
        self.tools = tools;
        self
    }

    pub fn with_system(mut self, system: impl Into<String>) -> Self {
        self.system = Some(system.into());
        self
    }

    pub fn with_max_tokens(mut self, n: u32) -> Self {
        self.max_tokens = n;
        self
    }

    pub fn with_effort(mut self, effort: Option<crate::Effort>) -> Self {
        self.effort = effort;
        self
    }

    /// Characters in the prompt, for usage estimates when a provider reports none.
    pub(crate) fn prompt_chars(&self) -> usize {
        self.system.as_deref().map_or(0, str::len)
            + self
                .messages
                .iter()
                .map(|m| m.content.len() + m.images.len() * IMAGE_CHARS)
                .sum::<usize>()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChatResponse {
    pub text: String,
    pub usage: Usage,
    /// Why generation stopped, as the provider words it (`stop`, `length`, `end_turn`).
    pub finish_reason: Option<String>,
    /// The model that actually answered, when the provider says.
    pub model: Option<String>,
}
