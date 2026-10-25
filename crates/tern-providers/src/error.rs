use thiserror::Error;

/// Provider failures, worded for the user. Response bodies are redacted
/// before they are kept here.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProviderError {
    #[error("{provider} needs an API key. Add one with `tern provider add {id}`.")]
    MissingKey {
        provider: &'static str,
        id: &'static str,
    },
    #[error(
        "{provider} refused the key (HTTP {status}). Check it is current and has access to this model.{}",
        said(detail)
    )]
    Auth {
        provider: &'static str,
        status: u16,
        /// The provider's own message, redacted; may be empty.
        detail: String,
    },
    #[error("{provider} is rate limiting this key{}. Wait and try again.{}", retry_after.map(|s| format!(" (retry after {s} s)")).unwrap_or_default(), said(detail))]
    RateLimited {
        provider: &'static str,
        retry_after: Option<u64>,
        /// The provider's own message, redacted; may be empty.
        detail: String,
    },
    #[error("{provider} does not know {what}.")]
    NotFound {
        provider: &'static str,
        what: String,
    },
    #[error("{provider}: {} (HTTP {status})", plain_body(body))]
    Http {
        provider: &'static str,
        status: u16,
        body: String,
    },
    #[error("Could not reach {provider}: {reason}")]
    Network {
        provider: &'static str,
        reason: String,
    },
    #[error("{provider} sent a reply Tern could not read: {reason}")]
    Decode {
        provider: &'static str,
        reason: String,
    },
    #[error("Invalid base URL `{0}`. Use an http or https address.")]
    BadUrl(String),
}

pub type ProviderResult<T> = Result<T, ProviderError>;

/// A provider's error body in plain words: the `message` (or
/// `error.message`, `error`, `detail`) of a JSON body with its `code`,
/// else the body itself, trimmed to one line of at most 300 characters.
pub fn plain_body(body: &str) -> String {
    let body = body.trim();
    if let Some(start) = body.find('{')
        && let Ok(v) = serde_json::from_str::<serde_json::Value>(&body[start..])
    {
        let text = |v: &serde_json::Value| v.as_str().map(str::to_owned);
        let msg = v
            .get("message")
            .and_then(text)
            .or_else(|| v.pointer("/error/message").and_then(text))
            .or_else(|| v.get("error").and_then(text))
            .or_else(|| v.get("detail").and_then(text))
            .or_else(|| {
                v.get("detail")
                    .and_then(|d| d.as_array())
                    .and_then(|a| a.first())
                    .and_then(|d| d.get("msg"))
                    .and_then(text)
            });
        if let Some(m) = msg.filter(|m| !m.trim().is_empty()) {
            let code = v
                .get("code")
                .or_else(|| v.pointer("/error/code"))
                .and_then(|c| match c {
                    serde_json::Value::String(s) if !s.is_empty() => Some(s.clone()),
                    serde_json::Value::Number(n) => Some(n.to_string()),
                    _ => None,
                });
            let m = m.trim().trim_end_matches('.').to_owned();
            return match code {
                Some(c) => format!("{m} (code {c})"),
                None => m,
            };
        }
    }
    let one: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() > 300 {
        format!("{}…", one.chars().take(300).collect::<String>())
    } else if one.is_empty() {
        "no details".into()
    } else {
        one
    }
}

/// " The provider said: ..." or nothing.
fn said(detail: &str) -> String {
    if detail.is_empty() {
        String::new()
    } else {
        format!(" The provider said: {detail}")
    }
}

#[cfg(test)]
mod plain_tests {
    use super::*;

    #[test]
    fn json_error_bodies_read_as_plain_words() {
        let e = ProviderError::Http {
            provider: "Mistral",
            status: 400,
            body: r#"{"object":"error","message":"Image input is not enabled for this model","type":"invalid_request_invalid_args","param":null,"code":"3051","raw_status_code":400}"#.into(),
        };
        assert_eq!(
            e.to_string(),
            "Mistral: Image input is not enabled for this model (code 3051) (HTTP 400)"
        );
        assert_eq!(
            plain_body(r#"{"error":{"message":"Invalid model.","code":404}}"#),
            "Invalid model (code 404)"
        );
        assert_eq!(plain_body(r#"{"detail":"Not found"}"#), "Not found");
        assert_eq!(plain_body("Bad Gateway\n\n"), "Bad Gateway");
        assert_eq!(plain_body(""), "no details");
        assert!(plain_body(&"x".repeat(500)).ends_with('…'));
    }
}
