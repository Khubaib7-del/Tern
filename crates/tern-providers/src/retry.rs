//! Visible retries for transient provider failures (#72).
//!
//! A long research run makes dozens of model calls, and one rate limit or
//! overloaded server used to end it. Calls that fail for a passing reason
//! (HTTP 429 rate limits, 5xx and Anthropic's 529 "overloaded", a dropped
//! connection, a timeout) are tried again with exponential backoff and
//! jitter. Every wait is reported first (`RetryNotice`), so the window can
//! say "Retrying in 8 s (attempt 2/4)"; nothing is retried silently, and
//! there is still no fallback to another provider (ADR 0004).
//!
//! Rules:
//! - A provider's `Retry-After` is a floor: Tern never waits less.
//! - When `Retry-After` is longer than [`RetryPolicy::max_wait`], Tern
//!   gives up at once and says so instead of hanging.
//! - A spending cap or exhausted quota (Anthropic's "usage limits",
//!   OpenAI's `insufficient_quota`) is a 429 that waiting will not fix: it
//!   is never retried ([`ProviderError::SpendCap`]).
//! - A wrong key, a bad request, a missing model, an unreadable reply, a
//!   DNS or TLS failure, or a local server that is not running are not
//!   retried either.

use std::future::Future;
use std::time::Duration;

use crate::{ProviderError, ProviderResult};

/// How many times, and how long, a failed call is tried again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Retries after the first try (so `retries + 1` tries in all).
    pub retries: u32,
    /// The first wait before jitter; each later wait doubles.
    pub base: Duration,
    /// No backoff wait grows past this (a `Retry-After` may).
    pub max_delay: Duration,
    /// The longest `Retry-After` Tern waits out; a longer one gives up.
    pub max_wait: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            retries: 4,
            base: Duration::from_secs(1),
            max_delay: Duration::from_secs(32),
            // #79: a stated wait up to a minute is waited out (with the
            // countdown); a longer one stops with the provider's words.
            max_wait: Duration::from_secs(60),
        }
    }
}

impl RetryPolicy {
    /// No retries at all.
    pub const NONE: Self = Self {
        retries: 0,
        base: Duration::ZERO,
        max_delay: Duration::ZERO,
        max_wait: Duration::ZERO,
    };
}

/// What kind of failure an error is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// Worth trying again: `reason` in a few words ("rate limited"), and
    /// the provider's `Retry-After`, if it sent one.
    Transient {
        reason: &'static str,
        retry_after: Option<Duration>,
    },
    /// A spending cap or quota: waiting will not help.
    SpendCap,
    /// Anything else: trying again would fail the same way.
    Terminal,
}

/// Sorts a provider error into [`Class`].
pub fn classify(e: &ProviderError) -> Class {
    match e {
        ProviderError::RateLimited {
            retry_after,
            detail,
            ..
        } => {
            if is_spend_cap(detail) {
                Class::SpendCap
            } else {
                Class::Transient {
                    reason: "rate limited",
                    // The wait a provider states only in words (Groq: "try
                    // again in 41.87s") counts like a `Retry-After` (#79).
                    retry_after: retry_after
                        .map(Duration::from_secs)
                        .or_else(|| stated_wait(detail)),
                }
            }
        }
        ProviderError::SpendCap { .. } => Class::SpendCap,
        ProviderError::Unavailable {
            status,
            retry_after,
            ..
        } => Class::Transient {
            reason: if *status == 529 || *status == 503 {
                "overloaded"
            } else {
                "server error"
            },
            retry_after: retry_after.map(Duration::from_secs),
        },
        ProviderError::Http { status, body, .. } => match status {
            408 => Class::Transient {
                reason: "timed out",
                retry_after: None,
            },
            500..=599 => Class::Transient {
                reason: "server error",
                retry_after: None,
            },
            _ if is_spend_cap(body) => Class::SpendCap,
            _ => Class::Terminal,
        },
        ProviderError::Network { reason, .. } => {
            if is_lasting_network(reason) {
                Class::Terminal
            } else if reason.contains("stream reported an error") && !is_overload_text(reason) {
                // An error event inside a stream: retried only when it says
                // the service is busy.
                Class::Terminal
            } else {
                Class::Transient {
                    reason: "connection problem",
                    retry_after: None,
                }
            }
        }
        _ => Class::Terminal,
    }
}

/// Whether a provider's words describe a spending cap or an exhausted
/// quota rather than a rate limit. Anthropic: "You have reached your
/// specified API usage limits"; OpenAI: `insufficient_quota`, "exceeded
/// your current quota"; others mention billing or credits.
pub fn is_spend_cap(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    // A per-minute or per-day limit that clears by itself is a rate limit,
    // even when the message also sells a bigger plan (#79: Groq's TPM
    // limit says "Upgrade to Dev Tier ... /settings/billing").
    if is_rate_window(&t) {
        return false;
    }
    [
        "usage limit",
        "spend limit",
        "spending limit",
        "spend cap",
        "insufficient_quota",
        "exceeded your current quota",
        "check your plan and billing",
        "credit balance",
        "billing",
        "out of credits",
        "payment required",
    ]
    .iter()
    .any(|n| t.contains(n))
}

/// Whether a provider's words describe a limit on requests or tokens per
/// minute, hour or day ("Rate limit reached ... tokens per minute (TPM)
/// ... Please try again in 41.87s"): it clears by itself.
pub fn is_rate_window(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    [
        "rate limit reached",
        "rate_limit_exceeded",
        "tokens per minute",
        "requests per minute",
        "tokens per day",
        "requests per day",
        "per minute",
        "(tpm)",
        "(rpm)",
        "(tpd)",
        "(rpd)",
        "try again in",
    ]
    .iter()
    .any(|n| t.contains(n))
}

/// A request too large for the model's per-request or per-minute token
/// cap (#80: Groq's HTTP 413 "Request too large for model ... on tokens
/// per minute (TPM): Limit 8000, Requested 17096"). Waiting does not
/// help; a smaller request might.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RequestCap {
    /// Tokens the provider accepts in one request.
    pub limit: u64,
    /// Tokens this request asked for, when the provider said.
    pub requested: Option<u64>,
}

/// The cap a provider error states, when it says the request was too
/// large: HTTP 413, or the words "request too large" / "reduce your
/// message size", with "Limit N" in the text.
pub fn request_cap(e: &ProviderError) -> Option<RequestCap> {
    let (status, text) = match e {
        ProviderError::Http { status, body, .. } => (*status, body.as_str()),
        ProviderError::RateLimited { detail, .. } | ProviderError::SpendCap { detail, .. } => {
            (429, detail.as_str())
        }
        _ => return None,
    };
    let t = text.to_ascii_lowercase();
    let too_large = status == 413
        || t.contains("request too large")
        || t.contains("reduce your message size")
        || t.contains("request entity too large");
    if !too_large {
        return None;
    }
    let number_after = |key: &str| -> Option<u64> {
        let at = t.find(key)? + key.len();
        let digits: String = t[at..]
            .trim_start_matches([' ', ':'])
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == ',')
            .filter(char::is_ascii_digit)
            .collect();
        digits.parse().ok()
    };
    Some(RequestCap {
        limit: number_after("limit")?,
        requested: number_after("requested"),
    })
}

/// The wait a provider's message states in words: "try again in 41.87s",
/// "in 1m30.5s", "in 250ms", "in 20 seconds" (rounded up to whole
/// seconds). `None` when it states none.
pub fn stated_wait(text: &str) -> Option<Duration> {
    let t = text.to_ascii_lowercase();
    let at = t.find("try again in")? + "try again in".len();
    let rest = t[at..].trim_start();
    let mut secs = 0f64;
    let mut num = String::new();
    let mut any = false;
    let mut chars = rest.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_ascii_digit() || c == '.' {
            num.push(c);
            continue;
        }
        if num.is_empty() {
            if c == ' ' {
                continue;
            }
            break;
        }
        let n: f64 = num.parse().ok()?;
        num.clear();
        let mut unit = String::from(c);
        while let Some(&d) = chars.peek() {
            if d.is_ascii_alphabetic() {
                unit.push(d);
                chars.next();
            } else {
                break;
            }
        }
        let unit = unit.trim().to_owned();
        let f = match unit.as_str() {
            "ms" => 0.001,
            "s" | "sec" | "secs" | "second" | "seconds" => 1.0,
            "m" | "min" | "mins" | "minute" | "minutes" => 60.0,
            "h" | "hr" | "hour" | "hours" => 3600.0,
            "" => {
                // "in 20 seconds": a space before the unit.
                let mut u = String::new();
                while let Some(&d) = chars.peek() {
                    if d.is_ascii_alphabetic() {
                        u.push(d);
                        chars.next();
                    } else if u.is_empty() && d == ' ' {
                        chars.next();
                    } else {
                        break;
                    }
                }
                match u.as_str() {
                    "ms" => 0.001,
                    "s" | "sec" | "secs" | "second" | "seconds" => 1.0,
                    "m" | "min" | "mins" | "minute" | "minutes" => 60.0,
                    "h" | "hr" | "hour" | "hours" => 3600.0,
                    _ => return None,
                }
            }
            _ => break,
        };
        secs += n * f;
        any = true;
        if !chars.peek().is_some_and(|d| d.is_ascii_digit()) {
            break;
        }
    }
    (any && secs.is_finite() && secs >= 0.0).then(|| Duration::from_secs(secs.ceil() as u64))
}

/// A stream error event that says the service is busy (Anthropic's
/// `overloaded_error`, "rate limit", "try again").
fn is_overload_text(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    [
        "overload",
        "rate limit",
        "rate_limit",
        "try again",
        "capacity",
        "unavailable",
    ]
    .iter()
    .any(|n| t.contains(n))
}

/// Network failures that another try will not fix: DNS, TLS, a proxy, or
/// nothing listening at a local address (the wording comes from
/// `client::network_reason`).
fn is_lasting_network(reason: &str) -> bool {
    let r = reason.to_ascii_lowercase();
    [
        "(dns)",
        "tls",
        "proxy",
        "nothing is listening",
        "is the server running",
        "unreachable from this network",
    ]
    .iter()
    .any(|n| r.contains(n))
}

/// The wait before retry number `attempt` (from 1), or `None` to give up:
/// out of retries, or a `Retry-After` longer than the policy's `max_wait`.
/// `jitter` in `[0, 1)` spreads the wait over the upper half of the
/// backoff ("equal jitter"), so many workers do not retry in step. A
/// `Retry-After` is never shortened.
pub fn next_delay(
    policy: RetryPolicy,
    attempt: u32,
    retry_after: Option<Duration>,
    jitter: f64,
) -> Option<Duration> {
    if attempt == 0 || attempt > policy.retries {
        return None;
    }
    if retry_after.is_some_and(|ra| ra > policy.max_wait) {
        return None;
    }
    let exp = policy
        .base
        .saturating_mul(1u32 << (attempt - 1).min(16))
        .min(policy.max_delay);
    let jitter = jitter.clamp(0.0, 1.0);
    let backoff = exp.mul_f64(0.5 + 0.5 * jitter);
    Some(match retry_after {
        Some(ra) => ra.max(backoff),
        None => backoff,
    })
}

/// One retry about to happen, for the window and the run log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetryNotice {
    /// This retry's number, from 1.
    pub attempt: u32,
    /// How many retries the policy allows.
    pub of: u32,
    /// How long Tern waits first.
    pub wait: Duration,
    /// In a few words: "rate limited", "overloaded", "server error",
    /// "connection problem", "timed out".
    pub reason: &'static str,
    /// The failure, as the user would read it.
    pub error: String,
}

impl RetryNotice {
    /// "Retrying in 8 s (attempt 2/4)".
    pub fn line(&self) -> String {
        retry_line(self.wait.as_secs_f64().ceil() as u64, self.attempt, self.of)
    }
}

/// "Retrying in 8 s (attempt 2/4)"; "Retrying now (attempt 2/4)" at 0.
pub fn retry_line(secs: u64, attempt: u32, of: u32) -> String {
    if secs == 0 {
        format!("Retrying now (attempt {attempt}/{of})")
    } else {
        format!("Retrying in {secs} s (attempt {attempt}/{of})")
    }
}

/// Runs `call` until it succeeds, fails for good, or runs out of retries.
/// `call` gets the try number (0 for the first). Before each wait,
/// `notify` hears about it. `may_retry` is asked before a retry and every
/// tenth of a second while waiting; `false` (the user stopped, or text
/// already reached the screen and a second reply would repeat it) ends
/// the wait with the last error.
pub async fn with_retry<T, F, Fut>(
    policy: RetryPolicy,
    notify: impl Fn(&RetryNotice),
    may_retry: impl Fn() -> bool,
    mut call: F,
) -> ProviderResult<T>
where
    F: FnMut(u32) -> Fut,
    Fut: Future<Output = ProviderResult<T>>,
{
    let mut attempt = 0u32;
    loop {
        let e = match call(attempt).await {
            Ok(v) => return Ok(v),
            Err(e) => e,
        };
        let Class::Transient {
            reason,
            retry_after,
        } = classify(&e)
        else {
            return Err(e);
        };
        if !may_retry() {
            return Err(e);
        }
        attempt += 1;
        let Some(wait) = next_delay(policy, attempt, retry_after, jitter()) else {
            return Err(e);
        };
        notify(&RetryNotice {
            attempt,
            of: policy.retries,
            wait,
            reason,
            error: e.to_string(),
        });
        let until = tokio::time::Instant::now() + wait;
        loop {
            let now = tokio::time::Instant::now();
            if now >= until {
                break;
            }
            tokio::time::sleep((until - now).min(Duration::from_millis(100))).await;
            if !may_retry() {
                return Err(e);
            }
        }
    }
}

/// A number in `[0, 1)` from the clock and a counter: enough to keep
/// parallel workers from retrying in step, without a random-number crate.
fn jitter() -> f64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0x9E37_79B9_7F4A_7C15);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    let mut x = N.fetch_add(0x9E37_79B9_7F4A_7C15, Ordering::Relaxed) ^ nanos;
    // xorshift64*
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    let x = x.wrapping_mul(0x2545_F491_4F6C_DD1D);
    (x >> 11) as f64 / (1u64 << 53) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_too_large_states_its_cap() {
        let e = ProviderError::Http {
            provider: "Groq",
            status: 413,
            body: r#"{"error":{"message":"Request too large for model `openai/gpt-oss-120b` in organization `org_x` service tier `on_demand` on tokens per minute (TPM): Limit 8000, Requested 17096, please reduce your message size and try again.","type":"tokens","code":"rate_limit_exceeded"}}"#.into(),
        };
        assert_eq!(
            request_cap(&e),
            Some(RequestCap {
                limit: 8000,
                requested: Some(17096)
            })
        );
        assert_eq!(classify(&e), Class::Terminal, "never retried as it is");
        let other = ProviderError::Http {
            provider: "Groq",
            status: 400,
            body: "max_tokens must be less than or equal to 512".into(),
        };
        assert_eq!(request_cap(&other), None);
    }
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn policy() -> RetryPolicy {
        RetryPolicy {
            retries: 3,
            base: Duration::from_millis(10),
            max_delay: Duration::from_millis(40),
            max_wait: Duration::from_secs(1),
        }
    }

    fn limited(retry_after: Option<u64>, detail: &str) -> ProviderError {
        ProviderError::RateLimited {
            provider: "Anthropic",
            retry_after,
            detail: detail.into(),
        }
    }

    #[test]
    fn transient_errors_are_told_apart_from_lasting_ones() {
        assert!(matches!(
            classify(&limited(Some(3), "")),
            Class::Transient {
                reason: "rate limited",
                retry_after: Some(d)
            } if d == Duration::from_secs(3)
        ));
        let overloaded = ProviderError::Unavailable {
            provider: "Anthropic",
            status: 529,
            retry_after: None,
            body: "Overloaded".into(),
        };
        assert!(matches!(
            classify(&overloaded),
            Class::Transient {
                reason: "overloaded",
                ..
            }
        ));
        let bad_gateway = ProviderError::Http {
            provider: "OpenAI",
            status: 502,
            body: String::new(),
        };
        assert!(matches!(classify(&bad_gateway), Class::Transient { .. }));
        let reset = ProviderError::Network {
            provider: "OpenAI",
            reason: "the connection to api.openai.com was cut off (reset). Try again in a moment."
                .into(),
        };
        assert!(matches!(classify(&reset), Class::Transient { .. }));
        for terminal in [
            ProviderError::Auth {
                provider: "OpenAI",
                status: 401,
                detail: String::new(),
            },
            ProviderError::Http {
                provider: "OpenAI",
                status: 400,
                body: "bad request".into(),
            },
            ProviderError::NotFound {
                provider: "OpenAI",
                what: "the model `x`".into(),
            },
            ProviderError::Network {
                provider: "Ollama",
                reason: "nothing is listening at localhost. Is the server running and the address right?".into(),
            },
            ProviderError::Network {
                provider: "OpenAI",
                reason: "could not look up api.openai.com (DNS). Check the internet connection, VPN, or proxy settings.".into(),
            },
            ProviderError::Network {
                provider: "OpenAI",
                reason: "the stream reported an error: invalid_request_error".into(),
            },
        ] {
            assert_eq!(classify(&terminal), Class::Terminal, "{terminal:?}");
        }
        let busy_stream = ProviderError::Network {
            provider: "Anthropic",
            reason: "the stream reported an error: {\"type\":\"overloaded_error\"}".into(),
        };
        assert!(matches!(classify(&busy_stream), Class::Transient { .. }));
    }

    #[test]
    fn a_spend_cap_429_is_never_retried() {
        let cap = limited(
            None,
            "You have reached your specified API usage limits. You will regain access on 2026-11-01 at 00:00 UTC.",
        );
        assert_eq!(classify(&cap), Class::SpendCap);
        assert_eq!(
            classify(&limited(
                None,
                "You exceeded your current quota, please check your plan and billing details. (code insufficient_quota)"
            )),
            Class::SpendCap
        );
        assert_eq!(
            classify(&ProviderError::SpendCap {
                provider: "Anthropic",
                detail: String::new()
            }),
            Class::SpendCap
        );
    }

    #[test]
    fn a_per_minute_limit_is_waited_out_even_when_it_mentions_billing() {
        let groq = "Rate limit reached for model `openai/gpt-oss-120b` in organization `org_x` service tier `on_demand` on tokens per minute (TPM): Limit 8000, Used 6521, Requested 7062. Please try again in 41.87s. Need more tokens? Upgrade to Dev Tier today at https://console.groq.com/settings/billing";
        assert!(!is_spend_cap(groq));
        assert!(is_rate_window(groq));
        assert_eq!(
            classify(&limited(None, groq)),
            Class::Transient {
                reason: "rate limited",
                retry_after: Some(Duration::from_secs(42)),
            }
        );
        // A header wins over the words.
        assert!(matches!(
            classify(&limited(Some(5), groq)),
            Class::Transient { retry_after: Some(d), .. } if d == Duration::from_secs(5)
        ));
        assert_eq!(
            stated_wait("Please try again in 1m30.5s."),
            Some(Duration::from_secs(91))
        );
        assert_eq!(
            stated_wait("try again in 250ms"),
            Some(Duration::from_secs(1))
        );
        assert_eq!(
            stated_wait("Try again in 20 seconds"),
            Some(Duration::from_secs(20))
        );
        assert_eq!(stated_wait("try again later"), None);
        assert_eq!(stated_wait("no wait here"), None);
        // A real quota stays terminal.
        assert!(is_spend_cap(
            "You exceeded your current quota, please check your plan and billing details."
        ));
        // A wait longer than a minute gives up at once by default.
        assert_eq!(
            next_delay(
                RetryPolicy::default(),
                1,
                Some(Duration::from_secs(432)),
                0.5
            ),
            None
        );
        assert!(
            next_delay(
                RetryPolicy::default(),
                1,
                Some(Duration::from_secs(42)),
                0.5
            )
            .is_some()
        );
    }

    #[test]
    fn backoff_doubles_with_jitter_and_stops_after_the_last_retry() {
        let p = policy();
        // Jitter 0 → half the backoff; jitter ~1 → nearly all of it.
        assert_eq!(next_delay(p, 1, None, 0.0), Some(Duration::from_millis(5)));
        assert_eq!(next_delay(p, 2, None, 0.0), Some(Duration::from_millis(10)));
        assert_eq!(next_delay(p, 3, None, 0.0), Some(Duration::from_millis(20)));
        let top = next_delay(p, 3, None, 0.999).unwrap();
        assert!(top <= Duration::from_millis(40) && top > Duration::from_millis(39));
        assert_eq!(next_delay(p, 4, None, 0.5), None);
        assert_eq!(next_delay(p, 0, None, 0.5), None);
        // The cap holds however many retries are allowed.
        let long = RetryPolicy { retries: 30, ..p };
        assert!(next_delay(long, 30, None, 0.999).unwrap() <= Duration::from_millis(40));
    }

    #[test]
    fn retry_after_is_a_floor_and_too_long_a_wait_gives_up() {
        let p = policy();
        // Never shorter than the server asked.
        assert_eq!(
            next_delay(p, 1, Some(Duration::from_millis(900)), 0.0),
            Some(Duration::from_millis(900))
        );
        // Shorter than the backoff: the backoff wins.
        assert_eq!(
            next_delay(p, 3, Some(Duration::from_millis(1)), 0.0),
            Some(Duration::from_millis(20))
        );
        // Longer than max_wait: give up at once.
        assert_eq!(next_delay(p, 1, Some(Duration::from_secs(2)), 0.0), None);
    }

    #[test]
    fn jitter_stays_in_range() {
        for _ in 0..1000 {
            let j = jitter();
            assert!((0.0..1.0).contains(&j), "{j}");
        }
    }

    #[test]
    fn the_status_line_counts_down_the_wait() {
        assert_eq!(retry_line(12, 2, 4), "Retrying in 12 s (attempt 2/4)");
        assert_eq!(retry_line(0, 1, 4), "Retrying now (attempt 1/4)");
        let n = RetryNotice {
            attempt: 1,
            of: 4,
            wait: Duration::from_millis(1500),
            reason: "rate limited",
            error: String::new(),
        };
        assert_eq!(n.line(), "Retrying in 2 s (attempt 1/4)");
    }

    #[tokio::test]
    async fn transient_failures_are_retried_and_each_wait_is_announced() {
        let tries = AtomicU32::new(0);
        let notes = Mutex::new(Vec::new());
        let out = with_retry(
            policy(),
            |n| notes.lock().unwrap().push((n.attempt, n.of, n.reason)),
            || true,
            |_| {
                let k = tries.fetch_add(1, Ordering::SeqCst);
                async move {
                    if k < 2 {
                        Err(limited(None, ""))
                    } else {
                        Ok("answer")
                    }
                }
            },
        )
        .await;
        assert_eq!(out, Ok("answer"));
        assert_eq!(tries.load(Ordering::SeqCst), 3);
        assert_eq!(
            *notes.lock().unwrap(),
            vec![(1, 3, "rate limited"), (2, 3, "rate limited")]
        );
    }

    #[tokio::test]
    async fn lasting_failures_and_spend_caps_fail_at_once() {
        for e in [
            ProviderError::Auth {
                provider: "OpenAI",
                status: 401,
                detail: String::new(),
            },
            limited(None, "You have reached your specified API usage limits."),
        ] {
            let tries = AtomicU32::new(0);
            let out: ProviderResult<()> = with_retry(
                policy(),
                |_| panic!("no retry for {e:?}"),
                || true,
                |_| {
                    tries.fetch_add(1, Ordering::SeqCst);
                    let e = e.clone();
                    async move { Err(e) }
                },
            )
            .await;
            assert!(out.is_err());
            assert_eq!(tries.load(Ordering::SeqCst), 1);
        }
    }

    #[tokio::test]
    async fn retries_run_out_and_the_last_error_is_returned() {
        let tries = AtomicU32::new(0);
        let out: ProviderResult<()> = with_retry(
            policy(),
            |_| {},
            || true,
            |_| {
                tries.fetch_add(1, Ordering::SeqCst);
                async { Err(limited(None, "")) }
            },
        )
        .await;
        assert!(matches!(out, Err(ProviderError::RateLimited { .. })));
        assert_eq!(tries.load(Ordering::SeqCst), 4);
    }

    #[tokio::test]
    async fn a_too_long_retry_after_gives_up_without_waiting() {
        let started = std::time::Instant::now();
        let out: ProviderResult<()> = with_retry(
            policy(),
            |_| panic!("no wait announced"),
            || true,
            |_| async { Err(limited(Some(3600), "")) },
        )
        .await;
        assert!(out.is_err());
        assert!(started.elapsed() < Duration::from_millis(500));
    }

    #[tokio::test]
    async fn stopping_ends_the_wait() {
        let stop = std::sync::atomic::AtomicBool::new(false);
        let long = RetryPolicy {
            retries: 2,
            base: Duration::from_secs(20),
            max_delay: Duration::from_secs(20),
            max_wait: Duration::from_secs(60),
        };
        let started = std::time::Instant::now();
        let out: ProviderResult<()> = with_retry(
            long,
            |_| stop.store(true, Ordering::SeqCst),
            || !stop.load(Ordering::SeqCst),
            |_| async { Err(limited(None, "")) },
        )
        .await;
        assert!(out.is_err());
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
