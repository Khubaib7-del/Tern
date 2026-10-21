# ADR 0003 · Own provider adapters over three wire formats

| Field | Value |
| --- | --- |
| Status | Accepted |
| Date | 2026-10-21 |
| Decider | Khubaib Nazeer |

ADR 0002 is reserved for the phase 1 palette and font choice.

## Context

Tern is bring-your-own-key across many providers. The architecture listed two options for `tern-providers`: the `genai` multi-provider crate, or Tern's own adapters. Tern needs exact control over three things a general SDK hides: which headers carry the key, what usage the provider reported versus what Tern estimated, and that a failed call never falls back to another provider silently.

Almost every provider speaks one of three request formats.

| Wire | Providers in the first catalog |
| --- | --- |
| OpenAI-compatible (`/chat/completions`, `/models`) | OpenAI, OpenRouter, Groq, Mistral, DeepSeek, xAI, Together, Fireworks, Cerebras, Ollama, LM Studio |
| Anthropic Messages (`/messages`, `/models`) | Anthropic |
| Gemini (`:generateContent`, `/models`) | Google AI Studio |

## Decision

Write Tern's own adapters for these three formats in `tern-providers`, on `reqwest` (rustls) and `tokio`. A provider is a catalog row (id, name, wire, base URL, whether it needs a key, where to get one), so adding an OpenAI-compatible provider is one line.

| Rule | How it holds |
| --- | --- |
| Keys travel only in headers | Bearer, `x-api-key`, or `x-goog-api-key`; never a query string. Tested |
| A key is saved only after the provider accepts it | `tern provider add` lists models first, then writes the keychain |
| No silent fallback | Errors name the provider and the reason; switching is the user's choice |
| Usage is honest | Provider-reported tokens are `measured`; otherwise a four-characters-a-token estimate marked `estimated`. Cost is unknown (`None`) until a price table exists |
| Error bodies are redacted | Passed through the core redactor and cut to 400 characters |
| Local servers skip system proxies | A loopback base URL is reached directly |

## Consequences

- Streaming, tool calls, and vision inputs are added per wire format as later slices need them.
- Provider-specific extras (reasoning effort, prompt caching) are added deliberately, one at a time.
- `genai` stays a reference, not a dependency.
