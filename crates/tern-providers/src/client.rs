//! The HTTP client for one configured provider.

use std::time::{Duration, Instant};

use reqwest::{RequestBuilder, Response, StatusCode};
use serde::Serialize;
use tern_core::{Redactor, SecretString};

use crate::wire;
use crate::{
    ChatRequest, ChatResponse, ModelInfo, ProviderError, ProviderResult, ProviderSpec, Wire,
};

/// The result of `tern provider test`.
#[derive(Clone, Debug, Serialize)]
pub struct ProbeReport {
    pub provider: &'static str,
    pub base_url: String,
    pub models: usize,
    pub chat_models: usize,
    pub latency_ms: u128,
}

/// One provider, ready to call.
#[derive(Debug, Clone)]
pub struct Provider {
    spec: &'static ProviderSpec,
    base_url: String,
    key: Option<SecretString>,
    http: reqwest::Client,
}

const TIMEOUT: Duration = Duration::from_secs(60);
const BODY_LIMIT: usize = 400;

impl Provider {
    /// `base_url` overrides the catalog URL (local servers on another port,
    /// proxies). It must be `http` or `https`.
    pub fn new(
        spec: &'static ProviderSpec,
        key: Option<SecretString>,
        base_url: Option<&str>,
    ) -> ProviderResult<Self> {
        let base = base_url
            .unwrap_or(spec.base_url)
            .trim_end_matches('/')
            .to_owned();
        let parsed = url::Url::parse(&base).map_err(|_| ProviderError::BadUrl(base.clone()))?;
        if !matches!(parsed.scheme(), "http" | "https") || parsed.host().is_none() {
            return Err(ProviderError::BadUrl(base));
        }
        if spec.needs_key && key.as_ref().is_none_or(SecretString::is_empty) {
            return Err(ProviderError::MissingKey {
                provider: spec.name,
                id: spec.id,
            });
        }
        let mut builder = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .user_agent(concat!("Tern/", env!("CARGO_PKG_VERSION")));
        // A local model server is never reached through a system proxy.
        if is_loopback(&parsed) {
            builder = builder.no_proxy();
        }
        let http = builder.build().map_err(|e| ProviderError::Network {
            provider: spec.name,
            reason: e.to_string(),
        })?;
        Ok(Self {
            spec,
            base_url: base,
            key,
            http,
        })
    }

    pub fn spec(&self) -> &'static ProviderSpec {
        self.spec
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    fn url(&self, path: &str) -> String {
        format!("{}/{}", self.base_url, path.trim_start_matches('/'))
    }

    fn authed(&self, rb: RequestBuilder) -> RequestBuilder {
        let Some(key) = &self.key else { return rb };
        match self.spec.wire {
            Wire::OpenAiCompatible => rb.bearer_auth(key.expose()),
            Wire::Anthropic => rb.header("x-api-key", key.expose()),
            Wire::Gemini => rb.header("x-goog-api-key", key.expose()),
        }
    }

    fn with_wire_headers(&self, rb: RequestBuilder) -> RequestBuilder {
        match self.spec.wire {
            Wire::Anthropic => rb.header("anthropic-version", wire::anthropic::VERSION),
            _ => rb,
        }
    }

    async fn send(&self, rb: RequestBuilder, what: &str) -> ProviderResult<serde_json::Value> {
        let rb = self.with_wire_headers(self.authed(rb));
        let resp = rb.send().await.map_err(|e| ProviderError::Network {
            provider: self.spec.name,
            reason: Redactor::default().redact(&without_url(&e)).into_owned(),
        })?;
        self.check(resp, what).await
    }

    async fn check(&self, resp: Response, what: &str) -> ProviderResult<serde_json::Value> {
        let status = resp.status();
        let provider = self.spec.name;
        if status.is_success() {
            return resp.json().await.map_err(|e| ProviderError::Decode {
                provider,
                reason: e.to_string(),
            });
        }
        let retry_after = resp
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse().ok());
        let body = resp.text().await.unwrap_or_default();
        Err(match status {
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => ProviderError::Auth {
                provider,
                status: status.as_u16(),
            },
            StatusCode::TOO_MANY_REQUESTS => ProviderError::RateLimited {
                provider,
                retry_after,
            },
            StatusCode::NOT_FOUND => ProviderError::NotFound {
                provider,
                what: what.to_owned(),
            },
            _ => ProviderError::Http {
                provider,
                status: status.as_u16(),
                body: short(&body),
            },
        })
    }

    /// Lists the models this key can use.
    pub async fn list_models(&self) -> ProviderResult<Vec<ModelInfo>> {
        let (path, what) = match self.spec.wire {
            Wire::OpenAiCompatible | Wire::Anthropic => ("models", "the models endpoint"),
            Wire::Gemini => ("models?pageSize=1000", "the models endpoint"),
        };
        let json = self.send(self.http.get(self.url(path)), what).await?;
        let decode = |reason: String| ProviderError::Decode {
            provider: self.spec.name,
            reason,
        };
        let mut models = match self.spec.wire {
            Wire::OpenAiCompatible => wire::openai::parse_models(&json),
            Wire::Anthropic => wire::anthropic::parse_models(&json),
            Wire::Gemini => wire::gemini::parse_models(&json),
        }
        .map_err(decode)?;
        models.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(models)
    }

    /// One non-streaming chat call.
    pub async fn chat(&self, req: &ChatRequest) -> ProviderResult<ChatResponse> {
        let what = format!("the model `{}`", req.model);
        let (url, body) = match self.spec.wire {
            Wire::OpenAiCompatible => (self.url("chat/completions"), wire::openai::chat_body(req)),
            Wire::Anthropic => (self.url("messages"), wire::anthropic::chat_body(req)),
            Wire::Gemini => {
                let model = req.model.strip_prefix("models/").unwrap_or(&req.model);
                (
                    self.url(&format!("models/{model}:generateContent")),
                    wire::gemini::chat_body(req),
                )
            }
        };
        let json = self.send(self.http.post(url).json(&body), &what).await?;
        let decode = |reason: String| ProviderError::Decode {
            provider: self.spec.name,
            reason,
        };
        match self.spec.wire {
            Wire::OpenAiCompatible => wire::openai::parse_chat(&json, req),
            Wire::Anthropic => wire::anthropic::parse_chat(&json, req),
            Wire::Gemini => wire::gemini::parse_chat(&json, req),
        }
        .map_err(decode)
    }

    /// Checks the key and the endpoint without spending tokens: lists models.
    pub async fn probe(&self) -> ProviderResult<ProbeReport> {
        let start = Instant::now();
        let models = self.list_models().await?;
        Ok(ProbeReport {
            provider: self.spec.name,
            base_url: self.base_url.clone(),
            models: models.len(),
            chat_models: models
                .iter()
                .filter(|m| m.kind == crate::ModelKind::Chat)
                .count(),
            latency_ms: start.elapsed().as_millis(),
        })
    }
}

fn is_loopback(u: &url::Url) -> bool {
    match u.host() {
        Some(url::Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

/// reqwest errors include the URL; Gemini once took keys in the query string,
/// so strip it to be safe.
fn without_url(e: &reqwest::Error) -> String {
    let mut s = e.to_string();
    if let Some(u) = e.url() {
        s = s.replace(u.as_str(), "<url>");
    }
    if e.is_timeout() {
        return "the request timed out".into();
    }
    if e.is_connect() {
        return "the connection failed. Is the server running and the address right?".into();
    }
    s
}

fn short(body: &str) -> String {
    let red = Redactor::default().redact(body).into_owned();
    let trimmed: String = red.chars().take(BODY_LIMIT).collect();
    if red.chars().count() > BODY_LIMIT {
        format!("{trimmed}...")
    } else {
        trimmed
    }
}
