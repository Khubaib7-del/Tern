//! Dev chaos switches (#85, A17): faults injected into model calls so the
//! durable-turn paths (waits, pauses, resumes, argument repair) can be
//! tried by hand without a provider misbehaving on cue.
//!
//! Off unless `TERN_DEV=1` and `TERN_CHAOS` lists faults, comma-separated,
//! fired one per model call in order, each once:
//!
//! | Fault | What the next model call does |
//! | --- | --- |
//! | `ok` | Runs normally (to place a later fault) |
//! | `429` or `429-short` | Fails with a rate limit and Retry-After 2 s |
//! | `429-daily` | Fails with a daily-quota rate limit (never retried) |
//! | `drop:N` | Streams N characters, then the connection drops |
//! | `bad-args` | Runs, and its tool calls come back with their arguments as a JSON string |
//! | `kill` | Ends the process before the call is sent (a crash mid-step) |
//!
//! Example: `TERN_DEV=1 TERN_CHAOS=ok,429-daily` lets the first call through
//! and fails the second with a daily limit.

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

use serde_json::Value;
use tern_providers::{ChatRequest, ChatResponse, ProviderError, ProviderResult};

use crate::engine::Model;
use crate::tools::BoxFuture;

/// One injected fault.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fault {
    Pass,
    RateLimitShort,
    RateLimitDaily,
    DropAfter(usize),
    BadArgs,
    Kill,
}

impl Fault {
    /// The fault a `TERN_CHAOS` item names. `None` for an unknown item.
    pub fn parse(item: &str) -> Option<Self> {
        let item = item.trim().to_ascii_lowercase();
        Some(match item.as_str() {
            "ok" | "pass" => Self::Pass,
            "429" | "429-short" => Self::RateLimitShort,
            "429-daily" => Self::RateLimitDaily,
            "bad-args" | "malformed-args" => Self::BadArgs,
            "kill" => Self::Kill,
            _ => {
                let n = item.strip_prefix("drop:")?.trim().parse().ok()?;
                Self::DropAfter(n)
            }
        })
    }

    fn id(&self) -> String {
        match self {
            Self::Pass => "ok".into(),
            Self::RateLimitShort => "429-short".into(),
            Self::RateLimitDaily => "429-daily".into(),
            Self::DropAfter(n) => format!("drop:{n}"),
            Self::BadArgs => "bad-args".into(),
            Self::Kill => "kill".into(),
        }
    }
}

/// The faults still to fire, in order.
#[derive(Debug, Default)]
pub struct Chaos {
    queue: Mutex<VecDeque<Fault>>,
    /// Faults that could not be read, for the startup log.
    pub unknown: Vec<String>,
}

impl Chaos {
    /// Faults from a `TERN_CHAOS` value.
    pub fn new(spec: &str) -> Self {
        let mut queue = VecDeque::new();
        let mut unknown = Vec::new();
        for item in spec.split(',').filter(|i| !i.trim().is_empty()) {
            match Fault::parse(item) {
                Some(f) => queue.push_back(f),
                None => unknown.push(item.trim().to_owned()),
            }
        }
        Self {
            queue: Mutex::new(queue),
            unknown,
        }
    }

    /// The next fault, consumed. `None` once all have fired.
    pub fn next(&self) -> Option<Fault> {
        self.queue.lock().ok()?.pop_front()
    }

    /// Faults left to fire.
    pub fn remaining(&self) -> usize {
        self.queue.lock().map(|q| q.len()).unwrap_or(0)
    }
}

/// Whether chaos may run: only with `TERN_DEV=1`.
pub fn dev_mode() -> bool {
    std::env::var("TERN_DEV").is_ok_and(|v| v.trim() == "1")
}

/// The process-wide chaos from `TERN_DEV=1` and `TERN_CHAOS`, read once.
/// `None` in normal use.
pub fn global() -> Option<&'static Chaos> {
    static C: OnceLock<Option<Chaos>> = OnceLock::new();
    C.get_or_init(|| {
        if !dev_mode() {
            return None;
        }
        let spec = std::env::var("TERN_CHAOS").ok()?;
        let c = Chaos::new(&spec);
        crate::diag::note(&format!(
            "chaos: TERN_CHAOS={spec:?} faults={} unknown={:?}",
            c.remaining(),
            c.unknown
        ));
        Some(c)
    })
    .as_ref()
}

/// A model with chaos in front of it: each call takes the next fault.
pub struct ChaosModel<'m> {
    pub inner: &'m dyn Model,
    pub chaos: &'m Chaos,
}

impl std::fmt::Debug for ChaosModel<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChaosModel")
            .field("provider", &self.inner.provider_id())
            .field("chaos", &self.chaos)
            .finish()
    }
}

fn rate_limited(daily: bool) -> ProviderError {
    ProviderError::RateLimited {
        provider: "Tern chaos",
        retry_after: if daily { None } else { Some(2) },
        detail: if daily {
            "simulated limit: requests per day used up".into()
        } else {
            "simulated rate limit".into()
        },
    }
}

/// A reply whose tool blocks carry their arguments as a JSON string, the
/// way some models send them.
pub fn stringify_args(text: &str) -> String {
    const FENCE: &str = "```tool\n";
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(FENCE) {
        let body = &rest[start + FENCE.len()..];
        let Some(end) = body.find("```") else {
            break;
        };
        out.push_str(&rest[..start + FENCE.len()]);
        let block = body[..end].trim();
        match serde_json::from_str::<Value>(block) {
            Ok(mut v) if v.get("args").is_some_and(Value::is_object) => {
                let s = v["args"].to_string();
                v["args"] = Value::String(s);
                out.push_str(&v.to_string());
                out.push('\n');
            }
            _ => out.push_str(&body[..end]),
        }
        out.push_str("```");
        rest = &body[end + 3..];
    }
    out.push_str(rest);
    out
}

impl ChaosModel<'_> {
    /// The fault for this call, logged. `kill` ends the process here.
    fn take(&self) -> Option<Fault> {
        let f = self.chaos.next()?;
        crate::diag::note(&format!("chaos: injecting {}", f.id()));
        if f == Fault::Kill {
            std::process::exit(86);
        }
        Some(f)
    }
}

impl Model for ChaosModel<'_> {
    fn provider_id(&self) -> &str {
        self.inner.provider_id()
    }

    fn complete<'a>(&'a self, req: &'a ChatRequest) -> BoxFuture<'a, ProviderResult<ChatResponse>> {
        let fault = self.take();
        Box::pin(async move {
            match fault {
                Some(Fault::RateLimitShort) => Err(rate_limited(false)),
                Some(Fault::RateLimitDaily) => Err(rate_limited(true)),
                Some(Fault::DropAfter(_)) => Err(ProviderError::Network {
                    provider: "Tern chaos",
                    reason: "simulated dropped connection".into(),
                }),
                Some(Fault::BadArgs) => {
                    let mut r = self.inner.complete(req).await?;
                    r.text = stringify_args(&r.text);
                    Ok(r)
                }
                _ => self.inner.complete(req).await,
            }
        })
    }

    fn complete_deltas<'a>(
        &'a self,
        req: &'a ChatRequest,
        on: &'a (dyn Fn(tern_providers::Delta<'_>) -> bool + Send + Sync),
    ) -> BoxFuture<'a, ProviderResult<ChatResponse>> {
        let fault = self.take();
        Box::pin(async move {
            match fault {
                Some(Fault::RateLimitShort) => Err(rate_limited(false)),
                Some(Fault::RateLimitDaily) => Err(rate_limited(true)),
                Some(Fault::DropAfter(n)) => {
                    let sent = Mutex::new(0usize);
                    let cut = move |d: tern_providers::Delta<'_>| match d {
                        tern_providers::Delta::Text(t) => {
                            let Ok(mut s) = sent.lock() else {
                                return false;
                            };
                            let left = n.saturating_sub(*s);
                            if left == 0 {
                                return false;
                            }
                            let piece: String = t.chars().take(left).collect();
                            *s += piece.chars().count();
                            on(tern_providers::Delta::Text(&piece)) && *s < n
                        }
                        other => on(other),
                    };
                    let _ = self.inner.complete_deltas(req, &cut).await;
                    Err(ProviderError::Network {
                        provider: "Tern chaos",
                        reason: format!("simulated dropped connection after {n} characters"),
                    })
                }
                Some(Fault::BadArgs) => {
                    let mut r = self.inner.complete_deltas(req, on).await?;
                    r.text = stringify_args(&r.text);
                    Ok(r)
                }
                _ => self.inner.complete_deltas(req, on).await,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn faults_parse_in_order_and_fire_once() {
        let c = Chaos::new("ok, 429-daily,drop:400,bad-args,429,nonsense");
        assert_eq!(c.unknown, vec!["nonsense".to_owned()]);
        assert_eq!(c.next(), Some(Fault::Pass));
        assert_eq!(c.next(), Some(Fault::RateLimitDaily));
        assert_eq!(c.next(), Some(Fault::DropAfter(400)));
        assert_eq!(c.next(), Some(Fault::BadArgs));
        assert_eq!(c.next(), Some(Fault::RateLimitShort));
        assert_eq!(c.next(), None);
    }

    #[test]
    fn the_daily_fault_reads_as_a_daily_limit() {
        let e = rate_limited(true);
        assert_eq!(
            crate::engine::error_class(&e, false),
            Some(tern_core::ErrorClass::RateLimitDaily)
        );
        assert_eq!(
            crate::engine::error_class(&rate_limited(false), false),
            Some(tern_core::ErrorClass::RateLimitShort)
        );
    }

    #[test]
    fn bad_args_turns_argument_objects_into_strings() {
        let text = "Reading.\n```tool\n{\"tool\": \"read_url\", \"args\": {\"url\": \"https://a.org\"}}\n```";
        let out = stringify_args(text);
        assert!(out.starts_with("Reading.\n```tool\n"));
        let calls = crate::protocol::parse_calls(&out);
        // Read back as the object it was (A6).
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1["url"], "https://a.org");
    }
}
