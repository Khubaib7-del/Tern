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
pub mod error;
pub mod model;
pub mod stream;
pub mod usage;
mod wire;

pub use catalog::{CATALOG, ProviderSpec, Wire, find};
pub use chat::{ChatMessage, ChatRequest, ChatResponse, Role};
pub use client::{ProbeReport, Provider};
pub use error::{ProviderError, ProviderResult};
pub use model::{ModelInfo, ModelKind};
pub use usage::{LedgerEntry, Usage, UsageLedger};
