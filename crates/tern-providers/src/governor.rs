//! The rate-limit governor (#85, item A3).
//!
//! A research turn makes many calls in a minute, and free keys allow few:
//! Groq's free tier caps a model at about 8,000 tokens a minute, Mistral's
//! free large model at about 20,000, Gemini's free tier at a few hundred
//! requests a day. Sending and then waiting out a 429 wastes the day's
//! requests and ended turns. The governor keeps, per provider and model,
//! three buckets (requests a minute, tokens a minute, requests a day) and
//! answers one question before each call: send now, wait this long, make
//! the request smaller, or stop for the day.
//!
//! - **Seeds** come from a dated table ([`LIMITS_JSON`]), the limits each
//!   provider documented on that date. They are only a starting point.
//! - **Reconciled** from what the provider says: `x-ratelimit-*` and
//!   `anthropic-ratelimit-*` headers on every reply ([`Governor::observe`]),
//!   and the limit a 429 body states.
//! - **Retry-After** as an HTTP date is a deadline, not ignored
//!   ([`retry_after_from`]).
//! - **Daily limits are never retried** ([`is_daily`]): waiting a minute
//!   cannot fix them, and each retry spends a request of the day.
//! - **Pre-fit**: a request larger than the token window is shrunk before
//!   it is sent ([`Admission::TooBig`]), never sent to fail.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The seed limits, dated. Reconciled from headers at run time.
pub const LIMITS_JSON: &str = include_str!("limits.json");

/// Limits for one provider (and optionally one model).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize)]
pub struct Limits {
    /// Requests a minute.
    #[serde(default)]
    pub rpm: Option<u64>,
    /// Tokens a minute (input plus output).
    #[serde(default)]
    pub tpm: Option<u64>,
    /// Requests a day.
    #[serde(default)]
    pub rpd: Option<u64>,
}

#[derive(Debug, serde::Deserialize)]
struct Table {
    as_of: String,
    providers: HashMap<String, ProviderLimits>,
}

#[derive(Debug, serde::Deserialize)]
struct ProviderLimits {
    #[serde(default)]
    default: Limits,
    /// By model id, or by a suffix such as `:free`.
    #[serde(default)]
    models: HashMap<String, Limits>,
}

fn table() -> &'static Table {
    static T: OnceLock<Table> = OnceLock::new();
    T.get_or_init(|| {
        serde_json::from_str(LIMITS_JSON).unwrap_or(Table {
            as_of: String::new(),
            providers: HashMap::new(),
        })
    })
}

/// The date the seed table was written.
pub fn limits_as_of() -> &'static str {
    &table().as_of
}

/// The seed limits for `model` at `provider`: an exact model entry, else a
/// suffix entry (`:free`), else the provider's default, else none.
pub fn seed(provider: &str, model: &str) -> Limits {
    let Some(p) = table().providers.get(provider) else {
        return Limits::default();
    };
    if let Some(l) = p.models.get(model) {
        return *l;
    }
    if let Some(l) = p
        .models
        .iter()
        .find(|(k, _)| k.starts_with(':') && model.ends_with(k.as_str()))
        .map(|(_, l)| *l)
    {
        return l;
    }
    p.default
}

/// A token bucket that refills evenly over its window.
#[derive(Clone, Copy, Debug)]
struct Bucket {
    cap: f64,
    level: f64,
    window: Duration,
    at: Instant,
}

impl Bucket {
    fn new(cap: u64, window: Duration, now: Instant) -> Self {
        Self {
            cap: cap as f64,
            level: cap as f64,
            window,
            at: now,
        }
    }

    fn refill(&mut self, now: Instant) {
        let dt = now.saturating_duration_since(self.at).as_secs_f64();
        let w = self.window.as_secs_f64().max(1e-9);
        self.level = (self.level + self.cap * dt / w).min(self.cap);
        self.at = now;
    }

    /// How long until `need` is available (zero when it is now).
    fn wait_for(&self, need: f64) -> Duration {
        if self.level >= need {
            return Duration::ZERO;
        }
        let missing = need.min(self.cap) - self.level;
        Duration::from_secs_f64((missing / self.cap.max(1e-9)) * self.window.as_secs_f64())
    }

    fn take(&mut self, n: f64) {
        self.level -= n;
    }

    /// The provider said how much is left: believe it.
    fn set_remaining(&mut self, remaining: f64, now: Instant) {
        self.refill(now);
        self.level = remaining.min(self.cap);
    }

    fn set_cap(&mut self, cap: u64) {
        let cap = cap as f64;
        if cap > 0.0 && (cap - self.cap).abs() > f64::EPSILON {
            self.level = self.level.min(cap);
            self.cap = cap;
        }
    }
}

/// One key's buckets.
#[derive(Clone, Debug)]
struct Key {
    limits: Limits,
    rpm: Option<Bucket>,
    tpm: Option<Bucket>,
    rpd: Option<Bucket>,
    /// Set by a daily 429 or a daily header at zero: no calls until then.
    closed_until: Option<SystemTime>,
}

impl Key {
    fn new(limits: Limits, now: Instant) -> Self {
        Self {
            limits,
            rpm: limits
                .rpm
                .map(|n| Bucket::new(n, Duration::from_secs(60), now)),
            tpm: limits
                .tpm
                .map(|n| Bucket::new(n, Duration::from_secs(60), now)),
            rpd: limits
                .rpd
                .map(|n| Bucket::new(n, Duration::from_secs(86_400), now)),
            closed_until: None,
        }
    }
}

/// What the governor says before a call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Admission {
    /// Send it.
    Now,
    /// Wait this long, then send (the same turn continues).
    Wait(Duration),
    /// The request is larger than the token window allows in one call:
    /// shrink it to at most `max_tokens` (or split it) before sending.
    TooBig { max_tokens: u64 },
    /// The day's requests are used up; they come back at `resets_at`.
    Daily { resets_at: SystemTime },
}

/// The longest a turn waits inside itself before the governor calls it a
/// daily stop instead.
pub const MAX_IN_TURN_WAIT: Duration = Duration::from_secs(90);

/// Share of the token window one request may use: a request that alone
/// fills the window leaves no room and fails at the provider.
const WINDOW_SHARE: f64 = 0.9;

/// Per-key buckets for every provider and model in use.
#[derive(Debug, Default)]
pub struct Governor {
    keys: Mutex<HashMap<(String, String), Key>>,
}

impl Governor {
    pub fn new() -> Self {
        Self::default()
    }

    fn with_key<T>(
        &self,
        provider: &str,
        model: &str,
        now: Instant,
        f: impl FnOnce(&mut Key) -> T,
    ) -> Option<T> {
        let mut keys = self.keys.lock().ok()?;
        let key = keys
            .entry((provider.to_owned(), model.to_owned()))
            .or_insert_with(|| Key::new(seed(provider, model), now));
        Some(f(key))
    }

    /// The limits in use for `model` (seeded, then reconciled).
    pub fn limits(&self, provider: &str, model: &str) -> Limits {
        self.with_key(provider, model, Instant::now(), |k| k.limits)
            .unwrap_or_default()
    }

    /// The largest request, in tokens, one call may carry: the share of the
    /// token window a single request may use. `None` when the window is
    /// unknown.
    pub fn envelope(&self, provider: &str, model: &str) -> Option<u64> {
        self.limits(provider, model)
            .tpm
            .map(|t| (t as f64 * WINDOW_SHARE) as u64)
    }

    /// Asks before a call of about `tokens` tokens.
    pub fn admit(&self, provider: &str, model: &str, tokens: u64) -> Admission {
        self.admit_at(provider, model, tokens, Instant::now(), SystemTime::now())
    }

    /// [`Governor::admit`] at a given time (for tests).
    pub fn admit_at(
        &self,
        provider: &str,
        model: &str,
        tokens: u64,
        now: Instant,
        wall: SystemTime,
    ) -> Admission {
        self.with_key(provider, model, now, |k| {
            if let Some(until) = k.closed_until {
                if until > wall {
                    return Admission::Daily { resets_at: until };
                }
                k.closed_until = None;
            }
            if let Some(t) = k.limits.tpm {
                let max = (t as f64 * WINDOW_SHARE) as u64;
                if tokens > max {
                    return Admission::TooBig { max_tokens: max };
                }
            }
            let mut wait = Duration::ZERO;
            for b in [k.rpm.as_mut(), k.rpd.as_mut()].into_iter().flatten() {
                b.refill(now);
                wait = wait.max(b.wait_for(1.0));
            }
            if let Some(b) = k.tpm.as_mut() {
                b.refill(now);
                wait = wait.max(b.wait_for(tokens as f64));
            }
            if wait > MAX_IN_TURN_WAIT {
                return Admission::Daily {
                    resets_at: wall + wait,
                };
            }
            if wait.is_zero() {
                for b in [k.rpm.as_mut(), k.rpd.as_mut()].into_iter().flatten() {
                    b.take(1.0);
                }
                if let Some(b) = k.tpm.as_mut() {
                    b.take(tokens as f64);
                }
                Admission::Now
            } else {
                Admission::Wait(wait)
            }
        })
        .unwrap_or(Admission::Now)
    }

    /// Takes a call's share from the buckets without asking: the call
    /// already waited out the window the governor named.
    pub fn take(&self, provider: &str, model: &str, tokens: u64) {
        let now = Instant::now();
        self.with_key(provider, model, now, |k| {
            for b in [k.rpm.as_mut(), k.rpd.as_mut()].into_iter().flatten() {
                b.refill(now);
                b.take(1.0);
            }
            if let Some(b) = k.tpm.as_mut() {
                b.refill(now);
                b.take(tokens as f64);
            }
        });
    }

    /// A reply came back with `actual` tokens where `estimated` were
    /// admitted: the difference is settled against the token bucket.
    pub fn settle(&self, provider: &str, model: &str, estimated: u64, actual: u64) {
        let now = Instant::now();
        self.with_key(provider, model, now, |k| {
            if let Some(b) = k.tpm.as_mut() {
                b.refill(now);
                b.take(actual as f64 - estimated as f64);
            }
        });
    }

    /// Reads the provider's rate-limit headers (name, value pairs, any
    /// case) and believes them over the seeds.
    pub fn observe<'h>(
        &self,
        provider: &str,
        model: &str,
        headers: impl IntoIterator<Item = (&'h str, &'h str)>,
    ) {
        let now = Instant::now();
        let mut h = RateHeaders::parse(headers);
        if h == RateHeaders::default() {
            return;
        }
        // Groq's request headers count the day, not the minute.
        if provider == "groq" {
            h.limit_requests_day = h.limit_requests_day.or(h.limit_requests.take());
            h.remaining_requests_day = h.remaining_requests_day.or(h.remaining_requests.take());
        }
        self.with_key(provider, model, now, |k| {
            let minute = Duration::from_secs(60);
            if let Some(cap) = h.limit_requests {
                k.limits.rpm = Some(cap);
                k.rpm
                    .get_or_insert_with(|| Bucket::new(cap, minute, now))
                    .set_cap(cap);
            }
            if let Some(cap) = h.limit_tokens {
                k.limits.tpm = Some(cap);
                k.tpm
                    .get_or_insert_with(|| Bucket::new(cap, minute, now))
                    .set_cap(cap);
            }
            if let Some(cap) = h.limit_requests_day {
                k.limits.rpd = Some(cap);
                k.rpd
                    .get_or_insert_with(|| Bucket::new(cap, Duration::from_secs(86_400), now))
                    .set_cap(cap);
            }
            if let (Some(r), Some(b)) = (h.remaining_requests, k.rpm.as_mut()) {
                b.set_remaining(r as f64, now);
            }
            if let (Some(r), Some(b)) = (h.remaining_tokens, k.tpm.as_mut()) {
                b.set_remaining(r as f64, now);
            }
            if let (Some(r), Some(b)) = (h.remaining_requests_day, k.rpd.as_mut()) {
                b.set_remaining(r as f64, now);
                if r == 0 {
                    k.closed_until =
                        Some(SystemTime::now() + h.reset_day.unwrap_or_else(next_utc_midnight));
                }
            }
        });
    }

    /// A daily 429: no calls on this key until `resets_at` (when the
    /// provider said) or the next UTC midnight.
    pub fn close_for_day(
        &self,
        provider: &str,
        model: &str,
        resets_in: Option<Duration>,
    ) -> SystemTime {
        let until = SystemTime::now() + resets_in.unwrap_or_else(next_utc_midnight);
        self.with_key(provider, model, Instant::now(), |k| {
            k.closed_until = Some(until)
        });
        until
    }

    /// A short 429 said how long to wait: the buckets are empty until then.
    pub fn back_off(&self, provider: &str, model: &str, wait: Duration) {
        let now = Instant::now();
        self.with_key(provider, model, now, |k| {
            for b in [k.rpm.as_mut(), k.tpm.as_mut()].into_iter().flatten() {
                b.refill(now);
                // Empty now, refilled when the wait ends.
                let per = b.cap / b.window.as_secs_f64().max(1e-9);
                b.level = -(per * wait.as_secs_f64()).min(b.cap);
            }
        });
    }
}

/// The governor every provider shares.
pub fn global() -> &'static Governor {
    static G: OnceLock<Governor> = OnceLock::new();
    G.get_or_init(Governor::new)
}

/// How long until the next midnight UTC.
fn next_utc_midnight() -> Duration {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Duration::from_secs(86_400 - secs % 86_400)
}

/// Rate-limit headers, in one shape across providers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RateHeaders {
    pub limit_requests: Option<u64>,
    pub remaining_requests: Option<u64>,
    pub limit_tokens: Option<u64>,
    pub remaining_tokens: Option<u64>,
    pub limit_requests_day: Option<u64>,
    pub remaining_requests_day: Option<u64>,
    /// When the day's requests come back, when a header said.
    pub reset_day: Option<Duration>,
}

impl RateHeaders {
    /// OpenAI, Groq, OpenRouter and Mistral (`x-ratelimit-limit-requests`,
    /// `x-ratelimit-remaining-tokens`, `x-ratelimit-limit-tokens-minute`,
    /// `x-ratelimit-remaining-req-day`, ...) and Anthropic
    /// (`anthropic-ratelimit-requests-remaining`, ...).
    pub fn parse<'h>(headers: impl IntoIterator<Item = (&'h str, &'h str)>) -> Self {
        let mut h = RateHeaders::default();
        for (name, value) in headers {
            let n = name.to_ascii_lowercase();
            if !(n.starts_with("x-ratelimit") || n.starts_with("anthropic-ratelimit")) {
                continue;
            }
            let v = value.trim();
            // What the header says after its prefix ("ratelimit" itself
            // contains "limit").
            let n = n
                .trim_start_matches("x-ratelimit")
                .trim_start_matches("anthropic-ratelimit");
            // Monthly and hourly windows are not tracked.
            if n.contains("month") || n.contains("hour") || n.contains("week") {
                continue;
            }
            let day = n.contains("day") || n.ends_with("-rpd") || n.contains("-tpd");
            // Mistral counts tokens as "by size".
            let tokens = n.contains("token") || n.starts_with("bysize");
            let requests = n.contains("request") || n.contains("-req");
            if n.contains("reset") {
                if day && requests {
                    h.reset_day = parse_reset(v);
                }
                continue;
            }
            let Ok(num) = v.parse::<f64>() else {
                continue;
            };
            if !num.is_finite() || num < 0.0 {
                continue;
            }
            let num = num as u64;
            let limit = n.contains("limit") && !n.contains("remaining");
            let remaining = n.contains("remaining");
            match (requests, tokens, day, limit, remaining) {
                (true, false, false, true, _) => h.limit_requests = Some(num),
                (true, false, false, _, true) => h.remaining_requests = Some(num),
                (false, true, false, true, _) => h.limit_tokens = Some(num),
                (false, true, false, _, true) => h.remaining_tokens = Some(num),
                (true, false, true, true, _) => h.limit_requests_day = Some(num),
                (true, false, true, _, true) => h.remaining_requests_day = Some(num),
                _ => {}
            }
        }
        h
    }
}

/// A reset header's value: seconds (`"7"`, `"1.5"`), a duration
/// (`"1m30.5s"`, `"250ms"`, `"6s"`), or an RFC 3339 / HTTP date.
pub fn parse_reset(v: &str) -> Option<Duration> {
    let v = v.trim();
    if let Ok(s) = v.parse::<f64>() {
        return (s.is_finite() && s >= 0.0).then(|| Duration::from_secs_f64(s));
    }
    if let Some(d) = crate::retry::stated_wait(&format!("try again in {v}")) {
        return Some(d);
    }
    let at = parse_http_date(v).or_else(|| parse_rfc3339_utc(v))?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    Some(Duration::from_secs(at.saturating_sub(now)))
}

/// A `Retry-After` value in seconds: a number of seconds (rounded up) or
/// an HTTP date, taken as a deadline relative to `now` (Unix seconds).
pub fn retry_after_from(value: &str, now: u64) -> Option<u64> {
    let v = value.trim();
    if let Ok(s) = v.parse::<f64>() {
        return (s.is_finite() && s >= 0.0).then(|| s.ceil() as u64);
    }
    parse_http_date(v).map(|at| at.saturating_sub(now))
}

/// An HTTP date (`Sun, 06 Nov 1994 08:49:37 GMT`, RFC 7231 IMF-fixdate)
/// as Unix seconds.
pub fn parse_http_date(v: &str) -> Option<u64> {
    let v = v.trim();
    let rest = v.split_once(", ").map(|(_, r)| r).unwrap_or(v);
    let parts: Vec<&str> = rest.split_whitespace().collect();
    if parts.len() != 5 || !matches!(parts[4], "GMT" | "UTC" | "UT" | "+0000") {
        return None;
    }
    let day: u32 = parts[0].parse().ok()?;
    let month = match &parts[1].to_ascii_lowercase()[..] {
        "jan" => 1,
        "feb" => 2,
        "mar" => 3,
        "apr" => 4,
        "may" => 5,
        "jun" => 6,
        "jul" => 7,
        "aug" => 8,
        "sep" => 9,
        "oct" => 10,
        "nov" => 11,
        "dec" => 12,
        _ => return None,
    };
    let year: i64 = parts[2].parse().ok()?;
    let hms: Vec<u32> = parts[3]
        .split(':')
        .map(|p| p.parse().ok())
        .collect::<Option<Vec<_>>>()?;
    let [h, m, s] = hms.as_slice() else {
        return None;
    };
    unix_secs(year, month, day, *h, *m, *s)
}

/// `2026-11-01T00:00:00Z` (UTC only) as Unix seconds.
fn parse_rfc3339_utc(v: &str) -> Option<u64> {
    let v = v.strip_suffix('Z')?;
    let (date, time) = v.split_once('T')?;
    let d: Vec<i64> = date
        .split('-')
        .map(|p| p.parse().ok())
        .collect::<Option<Vec<_>>>()?;
    let time = time.split('.').next()?;
    let t: Vec<u32> = time
        .split(':')
        .map(|p| p.parse().ok())
        .collect::<Option<Vec<_>>>()?;
    match (&d[..], &t[..]) {
        ([y, mo, da], [h, mi, s]) => unix_secs(*y, *mo as u32, *da as u32, *h, *mi, *s),
        _ => None,
    }
}

/// Civil date and time in UTC to Unix seconds (days from civil, H. Hinnant).
fn unix_secs(year: i64, month: u32, day: u32, h: u32, m: u32, s: u32) -> Option<u64> {
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || h > 23 || m > 59 || s > 60 {
        return None;
    }
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = days * 86_400 + h as i64 * 3600 + m as i64 * 60 + s as i64;
    u64::try_from(secs).ok()
}

/// Whether a 429's words describe a daily limit (requests or tokens per
/// day, a free tier's daily quota): waiting a minute will not fix it, so
/// it is never retried.
pub fn is_daily(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    [
        "per day",
        "requests per day",
        "tokens per day",
        "(rpd)",
        "(tpd)",
        "perday",
        "daily limit",
        "daily quota",
        "per-day",
        "free-models-per-day",
    ]
    .iter()
    .any(|n| t.contains(n))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_seed_table_parses_and_is_dated() {
        assert!(!limits_as_of().is_empty());
        let g = seed("groq", "llama-3.3-70b-versatile");
        assert!(g.tpm.is_some(), "{g:?}");
        let free = seed("openrouter", "deepseek/deepseek-r1:free");
        assert!(free.rpd.is_some(), "suffix entry: {free:?}");
        assert_eq!(seed("nobody", "x"), Limits::default());
    }

    #[test]
    fn a_request_bigger_than_the_window_is_shrunk_first() {
        let g = Governor::new();
        let now = Instant::now();
        let wall = SystemTime::now();
        g.observe("p", "m", [("x-ratelimit-limit-tokens", "8000")]);
        assert_eq!(
            g.admit_at("p", "m", 17_096, now, wall),
            Admission::TooBig { max_tokens: 7200 }
        );
        assert_eq!(g.envelope("p", "m"), Some(7200));
        assert_eq!(g.admit_at("p", "m", 5_000, now, wall), Admission::Now);
        // 5,000 of 8,000 used: 5,000 more needs about 15 s of refill.
        match g.admit_at("p", "m", 5_000, now, wall) {
            Admission::Wait(d) => {
                assert!(
                    d > Duration::from_secs(10) && d < Duration::from_secs(20),
                    "{d:?}"
                )
            }
            other => panic!("expected a wait, got {other:?}"),
        }
        // A minute later the window has refilled.
        assert_eq!(
            g.admit_at("p", "m", 5_000, now + Duration::from_secs(60), wall),
            Admission::Now
        );
    }

    #[test]
    fn headers_are_believed_over_the_seeds() {
        let h = RateHeaders::parse([
            ("X-RateLimit-Limit-Requests", "30"),
            ("x-ratelimit-remaining-requests", "0"),
            ("x-ratelimit-limit-tokens", "6000"),
            ("x-ratelimit-remaining-tokens", "5900"),
            ("x-ratelimit-reset-requests", "2s"),
            ("content-type", "application/json"),
        ]);
        assert_eq!(h.limit_requests, Some(30));
        assert_eq!(h.remaining_requests, Some(0));
        assert_eq!(h.limit_tokens, Some(6000));
        assert_eq!(h.remaining_tokens, Some(5900));
        let a = RateHeaders::parse([
            ("anthropic-ratelimit-requests-limit", "50"),
            ("anthropic-ratelimit-tokens-remaining", "1000"),
        ]);
        assert_eq!(
            (a.limit_requests, a.remaining_tokens),
            (Some(50), Some(1000))
        );
        let m = RateHeaders::parse([
            ("x-ratelimit-limit-req-day", "1000"),
            ("x-ratelimit-remaining-req-day", "999"),
            ("x-ratelimit-limit-tokens-minute", "500000"),
        ]);
        assert_eq!(m.limit_requests_day, Some(1000));
        assert_eq!(m.remaining_requests_day, Some(999));
        assert_eq!(m.limit_tokens, Some(500_000));

        let g = Governor::new();
        g.observe(
            "p",
            "m",
            [
                ("x-ratelimit-limit-requests", "30"),
                ("x-ratelimit-remaining-requests", "0"),
            ],
        );
        assert_eq!(g.limits("p", "m").rpm, Some(30));
        assert!(matches!(g.admit("p", "m", 10), Admission::Wait(_)));
    }

    #[test]
    fn a_daily_stop_holds_until_it_resets() {
        let g = Governor::new();
        let until = g.close_for_day("p", "m", Some(Duration::from_secs(3600)));
        match g.admit("p", "m", 10) {
            Admission::Daily { resets_at } => assert_eq!(resets_at, until),
            other => panic!("expected a daily stop, got {other:?}"),
        }
        assert!(is_daily(
            "Rate limit reached for requests per day (RPD): Limit 1000"
        ));
        assert!(is_daily(
            "Quota exceeded for quota metric 'GenerateRequestsPerDayPerProjectPerModel-FreeTier'"
        ));
        assert!(is_daily("Rate limit exceeded: free-models-per-day"));
        assert!(!is_daily("Rate limit reached for tokens per minute (TPM)"));
    }

    #[test]
    fn retry_after_reads_seconds_and_http_dates() {
        assert_eq!(retry_after_from("7", 0), Some(7));
        assert_eq!(retry_after_from("1.2", 0), Some(2));
        // 2015-10-21 07:28:00 UTC is 1445412480.
        assert_eq!(
            parse_http_date("Wed, 21 Oct 2015 07:28:00 GMT"),
            Some(1_445_412_480)
        );
        assert_eq!(
            retry_after_from("Wed, 21 Oct 2015 07:28:00 GMT", 1_445_412_470),
            Some(10)
        );
        assert_eq!(
            retry_after_from("Wed, 21 Oct 2015 07:28:00 GMT", 1_445_412_490),
            Some(0)
        );
        assert_eq!(parse_http_date("not a date"), None);
        assert_eq!(parse_rfc3339_utc("1970-01-02T00:00:00Z"), Some(86_400));
        assert_eq!(parse_reset("1m30s"), Some(Duration::from_secs(90)));
        assert_eq!(parse_reset("6"), Some(Duration::from_secs(6)));
    }
}
