//! What a model call costs (#85, A5): a dated snapshot of models.dev
//! prices, with overrides the app may set, applied to a call's usage with
//! its cache reads and writes priced apart (A19).
//!
//! A model missing from the snapshot has no cost: Tern never shows a
//! made-up one.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use serde::Deserialize;

use crate::Usage;

/// The snapshot, dated in its `as_of`.
pub const PRICES_JSON: &str = include_str!("prices.json");

/// A model's prices, in USD per million tokens.
#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize)]
pub struct Price {
    #[serde(rename = "in")]
    pub input: f64,
    #[serde(rename = "out")]
    pub output: f64,
    /// Cache reads; the input price when the provider has none.
    #[serde(default)]
    pub cache_read: Option<f64>,
    /// Cache writes; the input price when the provider has none.
    #[serde(default)]
    pub cache_write: Option<f64>,
}

impl Price {
    /// The cost of `u` in micro-dollars (1 USD = 1,000,000).
    pub fn cost_micros(&self, u: &Usage) -> u64 {
        // Tokens times USD per million tokens is micro-dollars.
        let micros = u.uncached_input() as f64 * self.input
            + u.cached_tokens as f64 * self.cache_read.unwrap_or(self.input)
            + u.cache_write_tokens as f64 * self.cache_write.unwrap_or(self.input)
            + u.output_tokens as f64 * self.output;
        micros.max(0.0).round() as u64
    }
}

#[derive(Debug, Deserialize)]
struct Table {
    as_of: String,
    providers: HashMap<String, HashMap<String, Price>>,
}

fn table() -> &'static Table {
    static T: OnceLock<Table> = OnceLock::new();
    T.get_or_init(|| {
        serde_json::from_str(PRICES_JSON).unwrap_or(Table {
            as_of: String::new(),
            providers: HashMap::new(),
        })
    })
}

fn overrides() -> &'static Mutex<HashMap<(String, String), Price>> {
    static O: OnceLock<Mutex<HashMap<(String, String), Price>>> = OnceLock::new();
    O.get_or_init(Mutex::default)
}

/// The date the snapshot was taken.
pub fn prices_as_of() -> &'static str {
    &table().as_of
}

/// Sets the price Tern uses for one model, over the snapshot (a price the
/// user typed, or a provider's own list).
pub fn set_override(provider: &str, model: &str, price: Price) {
    if let Ok(mut o) = overrides().lock() {
        o.insert((provider.to_owned(), model.to_owned()), price);
    }
}

/// The price of `model` at `provider`: an override, the snapshot's entry,
/// or the entry without a date suffix (`-2025-04-14`, `-20250219`) or a
/// `-latest` alias. OpenRouter's `:free` models cost nothing. `None` when
/// unknown.
pub fn price(provider: &str, model: &str) -> Option<Price> {
    if let Some(p) = overrides()
        .lock()
        .ok()
        .and_then(|o| o.get(&(provider.to_owned(), model.to_owned())).copied())
    {
        return Some(p);
    }
    if model.ends_with(":free") {
        return Some(Price::default());
    }
    let models = table().providers.get(provider)?;
    if let Some(p) = models.get(model) {
        return Some(*p);
    }
    let base = undated(model);
    models
        .get(base)
        .or_else(|| models.get(&format!("{base}-latest")))
        .copied()
}

/// The cost of a call in micro-dollars, `None` when the price is unknown.
pub fn cost_micros(provider: &str, model: &str, u: &Usage) -> Option<u64> {
    price(provider, model).map(|p| p.cost_micros(u))
}

/// `model` without a trailing date (`gpt-4.1-2025-04-14` → `gpt-4.1`,
/// `claude-3-7-sonnet-20250219` → `claude-3-7-sonnet`).
fn undated(model: &str) -> &str {
    let parts: Vec<&str> = model.rsplitn(4, '-').collect();
    // YYYY-MM-DD at the end.
    if parts.len() == 4
        && parts[0].len() == 2
        && parts[1].len() == 2
        && parts[2].len() == 4
        && parts[..3]
            .iter()
            .all(|p| p.bytes().all(|b| b.is_ascii_digit()))
    {
        return &model[..model.len() - 11];
    }
    // YYYYMMDD at the end.
    if let Some((head, tail)) = model.rsplit_once('-')
        && tail.len() == 8
        && tail.bytes().all(|b| b.is_ascii_digit())
    {
        return head;
    }
    model
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_snapshot_parses_and_is_dated() {
        assert!(!prices_as_of().is_empty());
        assert!(price("openai", "gpt-4.1-mini").is_some());
        assert!(price("anthropic", "claude-sonnet-4-5").is_some());
        assert_eq!(price("nobody", "nothing"), None);
    }

    #[test]
    fn cache_reads_and_writes_are_priced_apart() {
        let p = Price {
            input: 3.0,
            output: 15.0,
            cache_read: Some(0.3),
            cache_write: Some(3.75),
        };
        // 1,000 fresh input, 9,000 cache reads, 1,000 cache writes, 500 out.
        let u = Usage::measured(11_000, 500).with_details(9_000, 1_000, 0);
        assert_eq!(p.cost_micros(&u), 3_000 + 2_700 + 3_750 + 7_500);
        // Without cache prices, cached tokens cost the input price.
        let plain = Price {
            cache_read: None,
            cache_write: None,
            ..p
        };
        assert_eq!(plain.cost_micros(&u), 33_000 + 7_500);
    }

    #[test]
    fn dated_names_free_models_and_overrides() {
        assert_eq!(undated("gpt-4.1-2025-04-14"), "gpt-4.1");
        assert_eq!(undated("claude-3-7-sonnet-20250219"), "claude-3-7-sonnet");
        assert_eq!(undated("mistral-small"), "mistral-small");
        assert_eq!(
            price("openrouter", "meta-llama/llama-3.3-70b-instruct:free"),
            Some(Price::default())
        );
        set_override(
            "tern-test",
            "m",
            Price {
                input: 1.0,
                output: 2.0,
                ..Price::default()
            },
        );
        assert_eq!(
            cost_micros("tern-test", "m", &Usage::measured(1_000, 1_000)),
            Some(3_000)
        );
    }
}
