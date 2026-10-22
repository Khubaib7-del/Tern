//! Token usage and the per-run usage ledger.

use std::sync::Mutex;

use tern_core::{Measure, UsageDelta};

/// Tokens for one call.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub measure: Measure,
}

impl Usage {
    pub fn measured(input: u64, output: u64) -> Self {
        Self {
            input_tokens: input,
            output_tokens: output,
            measure: Measure::Measured,
        }
    }

    /// Rough count (four characters a token) when a provider reports nothing.
    pub fn estimated(prompt_chars: usize, output_chars: usize) -> Self {
        let t = |c: usize| (c as u64).div_ceil(4);
        Self {
            input_tokens: t(prompt_chars),
            output_tokens: t(output_chars),
            measure: Measure::Estimated,
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
}
