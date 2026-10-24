//! BYOK provider layer. Only this crate talks to model APIs.
//!
//! Tern ships its own adapters for three wire formats instead of a
//! multi-provider SDK, so every request, header, retry, and usage figure is
//! visible here:
//!
//! | Wire | Providers |
//! | --- | --- |
//! | OpenAI-compatible | OpenAI, Groq, OpenRouter, Mistral, DeepSeek, xAI, Together, Fireworks, Cerebras, Ollama, LM Studio, custom |
//! | Anthropic Messages | Anthropic |
//! | Gemini | Google AI Studio |
//!
//! There is no silent fallback: a failed call fails with a plain reason, and
//! switching provider is the user's choice.

#![forbid(unsafe_code)]

pub mod catalog;
pub mod chat;
mod client;
pub mod custom;
pub mod effort;
pub mod error;
pub mod model;
pub mod rpc_http;
pub mod stream;
pub mod usage;
mod wire;

pub use catalog::{CATALOG, DEFAULT_MAX_OUTPUT, ProviderSpec, Wire, default_max_output, find};
pub use chat::{ChatMessage, ChatRequest, ChatResponse, IMAGE_CHARS, ImageInput, Role};
pub use client::{Delta, ProbeReport, Provider};
pub use custom::{CustomProvider, CustomWire};
pub use effort::{Effort, ThinkSplitter, Thinking, thinking_support};
pub use error::{ProviderError, ProviderResult};
pub use model::{
    Capability, Category, Lifecycle, ModelInfo, ModelKind, Source, known_context_window,
    known_display_name, table_capabilities, tidy_aliases,
};
pub use usage::{LedgerEntry, Usage, UsageLedger};
