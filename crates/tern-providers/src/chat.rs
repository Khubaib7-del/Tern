//! Provider-neutral chat types.

use serde::{Deserialize, Serialize};

use crate::Usage;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
}

impl ChatMessage {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChatRequest {
    pub model: String,
    pub system: Option<String>,
    pub messages: Vec<ChatMessage>,
    pub max_tokens: u32,
    pub temperature: Option<f32>,
}

impl ChatRequest {
    pub fn new(model: impl Into<String>, messages: Vec<ChatMessage>) -> Self {
        Self {
            model: model.into(),
            system: None,
            messages,
            max_tokens: 1024,
            temperature: None,
        }
    }

    pub fn with_system(mut self, system: impl Into<String>) -> Self {
        self.system = Some(system.into());
        self
    }

    pub fn with_max_tokens(mut self, n: u32) -> Self {
        self.max_tokens = n;
        self
    }

    /// Characters in the prompt, for usage estimates when a provider reports none.
    pub(crate) fn prompt_chars(&self) -> usize {
        self.system.as_deref().map_or(0, str::len)
            + self.messages.iter().map(|m| m.content.len()).sum::<usize>()
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
