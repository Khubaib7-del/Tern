//! The secret redactor.
//!
//! Every event passes through it before it is persisted or shown in a log.
//! It replaces key-shaped strings with `[redacted:<kind>]`. It errs towards
//! hiding: a false positive costs a few characters of a log line, a false
//! negative leaks a key.

use std::borrow::Cow;

use regex::Regex;

/// A pattern for one kind of secret.
#[derive(Debug)]
struct Rule {
    kind: &'static str,
    re: Regex,
}

/// Replaces key-shaped strings with `[redacted:<kind>]`.
#[derive(Debug)]
pub struct Redactor {
    rules: Vec<Rule>,
}

const RULES: &[(&str, &str)] = &[
    // Anthropic before OpenAI: both start with `sk-`.
    ("anthropic", r"sk-ant-[A-Za-z0-9_\-]{20,}"),
    ("openrouter", r"sk-or-[A-Za-z0-9_\-]{20,}"),
    (
        "openai",
        r"sk-(?:proj-|svcacct-|admin-)?[A-Za-z0-9_\-]{20,}",
    ),
    ("google", r"AIza[0-9A-Za-z_\-]{35}"),
    ("github", r"(?:ghp|gho|ghu|ghs|ghr)_[A-Za-z0-9]{36,}"),
    ("github", r"github_pat_[A-Za-z0-9_]{22,}"),
    ("groq", r"gsk_[A-Za-z0-9]{20,}"),
    ("xai", r"xai-[A-Za-z0-9]{20,}"),
    ("huggingface", r"hf_[A-Za-z0-9]{30,}"),
    ("perplexity", r"pplx-[A-Za-z0-9]{20,}"),
    ("tavily", r"tvly-[A-Za-z0-9\-]{20,}"),
    ("aws", r"(?:AKIA|ASIA)[0-9A-Z]{16}"),
    ("slack", r"xox[abposr]-[A-Za-z0-9\-]{10,}"),
    ("stripe", r"(?:sk|rk)_(?:live|test)_[A-Za-z0-9]{16,}"),
    (
        "jwt",
        r"eyJ[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}",
    ),
    (
        "private_key",
        r"-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z ]*PRIVATE KEY-----",
    ),
    // Header and assignment forms: keep the name, hide the value.
    ("bearer", r"(?i)\bbearer\s+[A-Za-z0-9._~+/\-]{16,}=*"),
    (
        "secret",
        r#"(?i)\b(?:api[_-]?key|secret|token|password|passwd)\b["']?\s*[:=]\s*["']?[^\s"',;]{8,}"#,
    ),
];

impl Default for Redactor {
    fn default() -> Self {
        let rules = RULES
            .iter()
            .map(|(kind, pat)| Rule {
                kind,
                re: Regex::new(pat).expect("redaction pattern compiles"),
            })
            .collect();
        Self { rules }
    }
}

impl Redactor {
    /// Redacts one string. Borrows when nothing matched.
    pub fn redact<'a>(&self, text: &'a str) -> Cow<'a, str> {
        let mut out: Cow<'a, str> = Cow::Borrowed(text);
        for rule in &self.rules {
            if rule.re.is_match(&out) {
                let replacement = format!("[redacted:{}]", rule.kind);
                let replaced = rule.re.replace_all(&out, replacement.as_str()).into_owned();
                out = Cow::Owned(replaced);
            }
        }
        out
    }

    /// Whether `text` contains anything the redactor would hide.
    pub fn contains_secret(&self, text: &str) -> bool {
        self.rules.iter().any(|r| r.re.is_match(text))
    }

    /// Redacts every string inside a JSON value, including object keys' values
    /// whose key name says they hold a secret.
    pub fn redact_json(&self, value: &mut serde_json::Value) {
        use serde_json::Value;
        match value {
            Value::String(s) => {
                if let Cow::Owned(r) = self.redact(s) {
                    *s = r;
                }
            }
            Value::Array(items) => items.iter_mut().for_each(|v| self.redact_json(v)),
            Value::Object(map) => {
                for (k, v) in map.iter_mut() {
                    if is_secret_key(k) && v.is_string() {
                        *v = Value::String("[redacted:field]".into());
                    } else {
                        self.redact_json(v);
                    }
                }
            }
            _ => {}
        }
    }
}

fn is_secret_key(key: &str) -> bool {
    let k = key.to_ascii_lowercase().replace(['-', '_'], "");
    matches!(
        k.as_str(),
        "apikey"
            | "secret"
            | "token"
            | "accesstoken"
            | "refreshtoken"
            | "password"
            | "authorization"
            | "clientsecret"
            | "privatekey"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn r() -> Redactor {
        Redactor::default()
    }

    #[test]
    fn redacts_provider_keys() {
        let cases = [
            (
                "sk-ant-api03-AbCdEfGhIjKlMnOpQrStUvWxYz0123456789",
                "anthropic",
            ),
            ("sk-proj-AbCdEfGhIjKlMnOpQrStUvWxYz0123456789", "openai"),
            ("sk-AbCdEfGhIjKlMnOpQrStUvWx", "openai"),
            ("sk-or-v1-0123456789abcdef0123456789abcdef", "openrouter"),
            ("AIzaSyA1234567890abcdefghijklmnopqrstuv", "google"),
            ("ghp_0123456789abcdefghijklmnopqrstuvwxyz", "github"),
            ("github_pat_11ABCDEFG0123456789_abcdefghijklmnop", "github"),
            ("gsk_0123456789abcdefghijABCDEFGHIJ", "groq"),
            ("xai-0123456789abcdefghijABCDEFGHIJ", "xai"),
            ("hf_0123456789abcdefghijABCDEFGHIJklmn", "huggingface"),
            ("pplx-0123456789abcdefghijABCDEFGHIJ", "perplexity"),
            ("tvly-dev-0123456789abcdefghijABCD", "tavily"),
            ("AKIAIOSFODNN7EXAMPLE", "aws"),
        ];
        for (key, kind) in cases {
            let text = format!("key is {key} ok");
            let out = r().redact(&text);
            assert!(!out.contains(key), "{kind} key leaked: {out}");
            assert!(out.contains(&format!("[redacted:{kind}]")), "{kind}: {out}");
            assert!(out.starts_with("key is ") && out.ends_with(" ok"));
        }
    }

    #[test]
    fn redacts_headers_and_assignments() {
        let out = r().redact("Authorization: Bearer abcdefghijklmnop.qrstuvwx");
        assert!(!out.contains("abcdefghijklmnop"), "{out}");
        let out = r().redact("API_KEY=hunter2hunter2 and password: 'correct-horse'");
        assert!(!out.contains("hunter2hunter2"), "{out}");
        assert!(!out.contains("correct-horse"), "{out}");
    }

    #[test]
    fn redacts_private_keys_and_jwts() {
        let pem = "-----BEGIN PRIVATE KEY-----\nMIIEvQIBADANBg\nkqhkiG9w0BAQEF\n-----END PRIVATE KEY-----";
        assert_eq!(r().redact(pem), "[redacted:private_key]");
        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U";
        assert!(!r().redact(jwt).contains("dozjgNryP4J3"));
    }

    #[test]
    fn leaves_ordinary_text_alone() {
        let text = "Attention Is All You Need (Vaswani et al., 2017), arXiv:1706.03762, \
                    see https://arxiv.org/abs/1706.03762 and the skeleton key idea.";
        assert!(matches!(r().redact(text), Cow::Borrowed(_)));
        assert!(!r().contains_secret(text));
    }

    #[test]
    fn redacts_json_recursively() {
        let mut v = json!({
            "api_key": "short",
            "nested": {"Authorization": "anything at all", "note": "uses sk-ant-api03-AAAAAAAAAAAAAAAAAAAAAAAA"},
            "list": ["ghp_0123456789abcdefghijklmnopqrstuvwxyz", 3, null],
            "count": 2
        });
        r().redact_json(&mut v);
        let text = v.to_string();
        assert!(!text.contains("short"));
        assert!(!text.contains("anything at all"));
        assert!(!text.contains("sk-ant-api03"));
        assert!(!text.contains("ghp_0123"));
        assert_eq!(v["count"], 2);
    }
}
