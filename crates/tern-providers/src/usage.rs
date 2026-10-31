//! Token usage and the per-run usage ledger.

use std::sync::Mutex;

use tern_core::{Measure, UsageDelta};

/// Tokens for one call.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Usage {
    /// Every input token the provider billed, cache reads and writes
    /// included (Anthropic reports those apart; they are added in here).
    pub input_tokens: u64,
    /// Every output token, reasoning included.
    pub output_tokens: u64,
    pub measure: Measure,
    /// Input tokens read from the provider's prompt cache (#85, A19):
    /// part of `input_tokens`, billed at the cache-read price.
    pub cached_tokens: u64,
    /// Input tokens written to the prompt cache (Anthropic): part of
    /// `input_tokens`, billed at the cache-write price.
    pub cache_write_tokens: u64,
    /// Output tokens spent on reasoning: part of `output_tokens`.
    pub reasoning_tokens: u64,
}

impl Usage {
    pub fn measured(input: u64, output: u64) -> Self {
        Self {
            input_tokens: input,
            output_tokens: output,
            measure: Measure::Measured,
            cached_tokens: 0,
            cache_write_tokens: 0,
            reasoning_tokens: 0,
        }
    }

    /// The same usage with its cache and reasoning breakdown (#85, A19).
    /// Each part is capped at the total it belongs to.
    pub fn with_details(mut self, cached: u64, cache_write: u64, reasoning: u64) -> Self {
        self.cached_tokens = cached.min(self.input_tokens);
        self.cache_write_tokens = cache_write.min(self.input_tokens - self.cached_tokens);
        self.reasoning_tokens = reasoning.min(self.output_tokens);
        self
    }

    /// Input tokens billed at the full input price: neither read from nor
    /// written to the cache.
    pub fn uncached_input(&self) -> u64 {
        self.input_tokens
            .saturating_sub(self.cached_tokens)
            .saturating_sub(self.cache_write_tokens)
    }

    /// Rough count (four characters a token) when a provider reports nothing.
    pub fn estimated(prompt_chars: usize, output_chars: usize) -> Self {
        let t = |c: usize| (c as u64).div_ceil(4);
        Self {
            input_tokens: t(prompt_chars),
            output_tokens: t(output_chars),
            measure: Measure::Estimated,
            cached_tokens: 0,
            cache_write_tokens: 0,
            reasoning_tokens: 0,
        }
    }

    pub fn total(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }

    /// The event-log form. Cost stays `None` until Tern has a price table.
    pub fn to_delta(self) -> UsageDelta {
        UsageDelta {
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            cost_micros: None,
            currency: "USD".into(),
            measure: self.measure,
        }
    }
}

/// The cache and reasoning counts in an OpenAI-style `usage` object:
/// `prompt_tokens_details.cached_tokens` and
/// `completion_tokens_details.reasoning_tokens` (#85, A19). Some
/// OpenAI-compatible servers put `cached_tokens` at the top level.
pub fn openai_details(usage: Option<&serde_json::Value>) -> (u64, u64) {
    let Some(u) = usage else { return (0, 0) };
    let n = |p: &str| u.pointer(p).and_then(serde_json::Value::as_u64);
    (
        n("/prompt_tokens_details/cached_tokens")
            .or_else(|| n("/cached_tokens"))
            .unwrap_or(0),
        n("/completion_tokens_details/reasoning_tokens").unwrap_or(0),
    )
}

/// Anthropic's cache counts (`cache_read_input_tokens`,
/// `cache_creation_input_tokens`). Anthropic leaves both out of
/// `input_tokens`, so the caller adds them to it (#85, A19).
pub fn anthropic_details(usage: Option<&serde_json::Value>) -> (u64, u64) {
    let Some(u) = usage else { return (0, 0) };
    let n = |k: &str| u.get(k).and_then(serde_json::Value::as_u64).unwrap_or(0);
    (
        n("cache_read_input_tokens"),
        n("cache_creation_input_tokens"),
    )
}

/// Gemini's `cachedContentTokenCount` and `thoughtsTokenCount`. Gemini
/// leaves thoughts out of `candidatesTokenCount` but bills them as
/// output, so the caller adds them to the output (#85, A19).
pub fn gemini_details(meta: Option<&serde_json::Value>) -> (u64, u64) {
    let Some(m) = meta else { return (0, 0) };
    let n = |k: &str| m.get(k).and_then(serde_json::Value::as_u64).unwrap_or(0);
    (n("cachedContentTokenCount"), n("thoughtsTokenCount"))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LedgerEntry {
    pub provider: String,
    pub model: String,
    pub usage: Usage,
}

/// Every call a run made, so the footer and the run report add up.
#[derive(Debug, Default)]
pub struct UsageLedger {
    entries: Mutex<Vec<LedgerEntry>>,
}

impl UsageLedger {
    pub fn record(&self, provider: &str, model: &str, usage: Usage) {
        self.entries.lock().expect("ledger lock").push(LedgerEntry {
            provider: provider.to_owned(),
            model: model.to_owned(),
            usage,
        });
    }

    pub fn entries(&self) -> Vec<LedgerEntry> {
        self.entries.lock().expect("ledger lock").clone()
    }

    /// The most recent call's usage, if any call was made.
    pub fn last(&self) -> Option<Usage> {
        self.entries
            .lock()
            .expect("ledger lock")
            .last()
            .map(|e| e.usage)
    }

    pub fn calls(&self) -> usize {
        self.entries.lock().expect("ledger lock").len()
    }

    /// Totals across all calls. `Estimated` if any call was estimated.
    pub fn total(&self) -> Usage {
        let entries = self.entries.lock().expect("ledger lock");
        let mut t = Usage::measured(0, 0);
        for e in entries.iter() {
            t.input_tokens += e.usage.input_tokens;
            t.output_tokens += e.usage.output_tokens;
            t.cached_tokens += e.usage.cached_tokens;
            t.cache_write_tokens += e.usage.cache_write_tokens;
            t.reasoning_tokens += e.usage.reasoning_tokens;
            if e.usage.measure == Measure::Estimated {
                t.measure = Measure::Estimated;
            }
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimates_round_up() {
        let u = Usage::estimated(9, 1);
        assert_eq!((u.input_tokens, u.output_tokens), (3, 1));
        assert_eq!(u.measure, Measure::Estimated);
    }

    #[test]
    fn ledger_totals_and_marks_estimates() {
        let l = UsageLedger::default();
        l.record("openai", "gpt-4.1-mini", Usage::measured(100, 20));
        assert_eq!(l.total(), Usage::measured(100, 20));
        l.record("ollama", "llama3.2", Usage::estimated(40, 8));
        let t = l.total();
        assert_eq!(
            (t.input_tokens, t.output_tokens, t.measure),
            (110, 22, Measure::Estimated)
        );
        assert_eq!(l.entries().len(), 2);
        assert_eq!(t.to_delta().cost_micros, None);
    }

    #[test]
    fn cache_and_reasoning_counts_are_read_on_every_wire() {
        let o = serde_json::json!({"prompt_tokens": 1000, "completion_tokens": 50,
            "prompt_tokens_details": {"cached_tokens": 800},
            "completion_tokens_details": {"reasoning_tokens": 30}});
        assert_eq!(openai_details(Some(&o)), (800, 30));
        let a = serde_json::json!({"input_tokens": 20, "output_tokens": 5,
            "cache_read_input_tokens": 900, "cache_creation_input_tokens": 100});
        assert_eq!(anthropic_details(Some(&a)), (900, 100));
        let g = serde_json::json!({"promptTokenCount": 500, "candidatesTokenCount": 10,
            "cachedContentTokenCount": 400, "thoughtsTokenCount": 70});
        assert_eq!(gemini_details(Some(&g)), (400, 70));
        assert_eq!(openai_details(None), (0, 0));
        let u = Usage::measured(1000, 50).with_details(800, 0, 30);
        assert_eq!(u.uncached_input(), 200);
        // Parts never exceed their totals.
        let u = Usage::measured(10, 5).with_details(50, 50, 50);
        assert_eq!(
            (u.cached_tokens, u.cache_write_tokens, u.reasoning_tokens),
            (10, 0, 5)
        );
    }
}
