//! The HTTP client for one configured provider.

use std::time::{Duration, Instant};

use reqwest::{RequestBuilder, Response, StatusCode};
use serde::Serialize;
use tern_core::{Redactor, SecretString};

use crate::stream::{self, Assembler, SseParser};
use crate::wire;
use crate::{
    ChatRequest, ChatResponse, ModelInfo, ProviderError, ProviderResult, ProviderSpec, Wire,
};

/// One piece of a streamed reply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delta<'a> {
    /// Answer text.
    Text(&'a str),
    /// The model's reasoning, for the Thinking section.
    Reasoning(&'a str),
}

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
/// How long opening a connection may take before Tern gives up and says
/// so; a request that never connects should not wait the full timeout.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a model listing is reused before it is fetched again.
pub const LISTING_TTL: Duration = Duration::from_secs(10 * 60);
/// A streamed reply may run long; the whole stream gets this much.
const STREAM_TIMEOUT: Duration = Duration::from_secs(600);
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
        // One shared client per kind of address: building a client loads
        // the TLS roots, which is slow on some machines, and a shared pool
        // keeps connections warm between calls.
        let http =
            shared_client(is_loopback(&parsed)).map_err(|reason| ProviderError::Network {
                provider: spec.name,
                reason,
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
        let resp = self.dispatch(rb).await?;
        self.check(resp, what).await
    }

    /// Sends a request, trying once more when the connection itself could
    /// not be made (nothing reached the server, so a second try is safe):
    /// a stale pooled connection or a brief network blip should not fail a
    /// turn. Errors say what failed (DNS, TLS, refused, timed out).
    async fn dispatch(&self, rb: RequestBuilder) -> ProviderResult<Response> {
        let again = rb.try_clone();
        match rb.send().await {
            Ok(r) => Ok(r),
            Err(e) if e.is_connect() && again.is_some() => {
                tokio::time::sleep(Duration::from_millis(400)).await;
                match again {
                    Some(rb) => rb.send().await.map_err(|e| self.network(&e)),
                    None => Err(self.network(&e)),
                }
            }
            Err(e) => Err(self.network(&e)),
        }
    }

    fn network(&self, e: &reqwest::Error) -> ProviderError {
        ProviderError::Network {
            provider: self.spec.name,
            reason: Redactor::default()
                .redact(&describe_network(e, &self.base_url))
                .into_owned(),
        }
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
        let detail = said(&body);
        Err(match status {
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => ProviderError::Auth {
                provider,
                status: status.as_u16(),
                detail,
            },
            StatusCode::TOO_MANY_REQUESTS => ProviderError::RateLimited {
                provider,
                retry_after,
                detail,
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

    /// The models the provider lists, reused for [`LISTING_TTL`] after a
    /// fetch: opening or reopening a chat, switching models, and the picker
    /// do not each go back to the network. A server on this computer
    /// (Ollama, LM Studio) answers instantly and can change its models at
    /// any moment, so its listing is never cached.
    pub async fn list_models_cached(&self) -> ProviderResult<Vec<ModelInfo>> {
        if let Some(m) = self.cached_models(LISTING_TTL) {
            return Ok(m);
        }
        self.list_models().await
    }

    /// The last listing fetched for this provider, address, and key, when
    /// it is younger than `max_age`.
    pub fn cached_models(&self, max_age: Duration) -> Option<Vec<ModelInfo>> {
        let key = self.listing_key()?;
        let cache = listings().lock().ok()?;
        let (at, list) = cache.get(&key)?;
        (at.elapsed() <= max_age).then(|| list.clone())
    }

    /// Drops the cached listing so the next call fetches it again.
    pub fn forget_cached_models(&self) {
        let Some(key) = self.listing_key() else {
            return;
        };
        if let Ok(mut c) = listings().lock() {
            c.remove(&key);
        }
    }

    /// The cache key: provider, address, and a fingerprint of the key (a
    /// new key may see other models). The key itself is never stored.
    /// `None` for a server on this computer, whose listing is not cached.
    fn listing_key(&self) -> Option<String> {
        let remote = url::Url::parse(&self.base_url).is_ok_and(|u| !is_loopback(&u));
        if !remote {
            return None;
        }
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        if let Some(k) = &self.key {
            k.expose().hash(&mut h);
        }
        Some(format!(
            "{}|{}|{:x}",
            self.spec.id,
            self.base_url,
            h.finish()
        ))
    }

    /// Lists every model this key can use, every page of it, with what
    /// each can do (see `docs/models.md`). Nothing is filtered out: retired
    /// models come back marked, and the caller decides what to show. The
    /// result is cached for [`Self::list_models_cached`].
    pub async fn list_models(&self) -> ProviderResult<Vec<ModelInfo>> {
        const MAX_PAGES: usize = 50;
        let what = "the models endpoint";
        let decode = |reason: String| ProviderError::Decode {
            provider: self.spec.name,
            reason,
        };
        let mut models = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let path = self.models_path(cursor.as_deref());
            let json = self.send(self.http.get(self.url(&path)), what).await?;
            let (page, next) = match self.spec.wire {
                Wire::OpenAiCompatible => (
                    wire::openai::parse_models(&json),
                    wire::openai::next_cursor(&json),
                ),
                Wire::Anthropic => (
                    wire::anthropic::parse_models(&json),
                    wire::anthropic::next_cursor(&json),
                ),
                Wire::Gemini => (
                    wire::gemini::parse_models(&json),
                    wire::gemini::next_cursor(&json),
                ),
            };
            models.extend(page.map_err(decode)?);
            match next {
                Some(n) if Some(&n) != cursor.as_ref() => cursor = Some(n),
                _ => break,
            }
        }
        self.enrich(&mut models).await;
        for m in &mut models {
            m.finish(self.spec.id);
        }
        let mut models = crate::model::tidy_aliases(models);
        models.sort_by(|a, b| a.id.cmp(&b.id));
        models.dedup_by(|a, b| a.id == b.id);
        if let (Some(key), Ok(mut c)) = (self.listing_key(), listings().lock()) {
            c.insert(key, (Instant::now(), models.clone()));
        }
        Ok(models)
    }

    /// The listing path for one page.
    fn models_path(&self, cursor: Option<&str>) -> String {
        let enc = |c: &str| url::form_urlencoded::byte_serialize(c.as_bytes()).collect::<String>();
        match (self.spec.wire, cursor) {
            // Anthropic sends 20 a page unless asked for more.
            (Wire::Anthropic, None) => "models?limit=1000".into(),
            (Wire::Anthropic, Some(c)) => format!("models?limit=1000&after_id={}", enc(c)),
            (Wire::Gemini, None) => "models?pageSize=1000".into(),
            (Wire::Gemini, Some(c)) => format!("models?pageSize=1000&pageToken={}", enc(c)),
            // OpenRouter lists text-output models only unless asked for all.
            (Wire::OpenAiCompatible, _) if self.spec.id == "openrouter" => {
                "models?output_modalities=all".into()
            }
            (Wire::OpenAiCompatible, None) => "models".into(),
            (Wire::OpenAiCompatible, Some(c)) => format!("models?after={}", enc(c)),
        }
    }

    /// Adds what a second, cheap endpoint knows and `/models` does not:
    /// xAI's modalities, Ollama's capabilities, LM Studio's model types.
    /// A failure here leaves the listing as it was.
    async fn enrich(&self, models: &mut [ModelInfo]) {
        let extra: Vec<(&str, String)> = match self.spec.id {
            "xai" => vec![
                ("data", self.url("language-models")),
                ("data", self.url("image-generation-models")),
            ],
            "ollama" => vec![("models", format!("{}/api/tags", self.root()))],
            "lmstudio" => vec![("data", format!("{}/api/v0/models", self.root()))],
            _ => return,
        };
        for (key, url) in extra {
            let Ok(json) = self.send(self.http.get(url), "the model details").await else {
                continue;
            };
            let Some(list) = json.get(key).and_then(serde_json::Value::as_array) else {
                continue;
            };
            for card in list {
                let card = with_ollama_caps(card);
                let Ok(found) = wire::openai::parse_models(&serde_json::json!([card])) else {
                    continue;
                };
                let Some(found) = found.into_iter().next() else {
                    continue;
                };
                let same = |m: &ModelInfo| m.id == found.id || found.aliases.contains(&m.id);
                if let Some(m) = models.iter_mut().find(|m| same(m)) {
                    if found.source != crate::model::Source::Table {
                        m.capabilities = found.capabilities.clone();
                        m.source = found.source;
                    }
                    if m.context_window.is_none() {
                        m.context_window = found.context_window;
                    }
                    for a in &found.aliases {
                        if !m.aliases.contains(a) && *a != m.id {
                            m.aliases.push(a.clone());
                        }
                    }
                }
            }
        }
    }

    /// The server root for a local server's own API (`/v1` taken off).
    fn root(&self) -> &str {
        self.base_url.strip_suffix("/v1").unwrap_or(&self.base_url)
    }

    /// Transcribes `audio` with a speech-to-text model through the
    /// OpenAI-style `/audio/transcriptions` endpoint (Groq, OpenAI,
    /// Mistral's Voxtral, and other compatible servers). `file_name` tells
    /// the server the format (`speech.wav`).
    pub async fn transcribe(
        &self,
        model: &str,
        audio: &[u8],
        file_name: &str,
        language: Option<&str>,
    ) -> ProviderResult<String> {
        if self.spec.wire != Wire::OpenAiCompatible {
            return Err(ProviderError::Http {
                provider: self.spec.name,
                status: 400,
                body: "Tern transcribes through OpenAI-style providers only (Groq, OpenAI, Mistral, and compatible servers).".into(),
            });
        }
        let boundary = format!(
            "tern-{:x}",
            audio.len().wrapping_mul(2_654_435_761) ^ 0x5bd1_e995
        );
        let mut fields = vec![("model", model), ("response_format", "json")];
        if let Some(l) = language {
            fields.push(("language", l));
        }
        let body = multipart(&boundary, &fields, ("file", file_name, audio));
        let rb = self
            .http
            .post(self.url("audio/transcriptions"))
            .header(
                "content-type",
                format!("multipart/form-data; boundary={boundary}"),
            )
            .timeout(STREAM_TIMEOUT)
            .body(body);
        let json = self.send(rb, &format!("the model `{model}`")).await?;
        json.get("text")
            .and_then(serde_json::Value::as_str)
            .map(|t| t.trim().to_owned())
            .ok_or_else(|| ProviderError::Decode {
                provider: self.spec.name,
                reason: "no `text` in the transcription".into(),
            })
    }

    /// Reads the text out of a document or picture with Mistral's OCR
    /// endpoint (`POST /ocr`). `data` is base64; `media_type` is
    /// `application/pdf` or an image type. Returns the pages as Markdown.
    pub async fn ocr(&self, model: &str, media_type: &str, data: &str) -> ProviderResult<String> {
        let url = format!("data:{media_type};base64,{data}");
        let document = if media_type.starts_with("image/") {
            serde_json::json!({"type": "image_url", "image_url": url})
        } else {
            serde_json::json!({"type": "document_url", "document_url": url})
        };
        let body = serde_json::json!({"model": model, "document": document});
        let rb = self
            .http
            .post(self.url("ocr"))
            .json(&body)
            .timeout(STREAM_TIMEOUT);
        let json = self.send(rb, &format!("the model `{model}`")).await?;
        let pages = json
            .get("pages")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| ProviderError::Decode {
                provider: self.spec.name,
                reason: "no `pages` in the OCR reply".into(),
            })?;
        Ok(pages
            .iter()
            .filter_map(|p| p.get("markdown").and_then(serde_json::Value::as_str))
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n"))
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
        let mut body = body;
        crate::effort::apply(&mut body, self.spec, req);
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

    /// One streaming chat call. `on_text` gets each piece of text as it
    /// arrives and returns `false` to stop early (the user pressed Stop); the
    /// reply then holds what arrived, with finish reason `cancelled`.
    /// Reasoning is dropped; [`Provider::chat_stream_deltas`] passes it on.
    pub async fn chat_stream(
        &self,
        req: &ChatRequest,
        on_text: &(dyn Fn(&str) -> bool + Send + Sync),
    ) -> ProviderResult<ChatResponse> {
        let on = |d: Delta<'_>| match d {
            Delta::Text(t) => on_text(t),
            Delta::Reasoning(_) => true,
        };
        self.chat_stream_deltas(req, &on).await
    }

    /// [`Provider::chat_stream`] that also streams the model's reasoning
    /// (thinking) as [`Delta::Reasoning`]. Reasoning never joins the
    /// reply's text. `<think>` blocks at the start of a reply (open models)
    /// count as reasoning too.
    pub async fn chat_stream_deltas(
        &self,
        req: &ChatRequest,
        on: &(dyn Fn(Delta<'_>) -> bool + Send + Sync),
    ) -> ProviderResult<ChatResponse> {
        let what = format!("the model `{}`", req.model);
        let (url, body) = match self.spec.wire {
            Wire::OpenAiCompatible => {
                let mut body = wire::openai::chat_body(req);
                body["stream"] = serde_json::json!(true);
                if reports_stream_usage(self.spec.id) {
                    body["stream_options"] = serde_json::json!({"include_usage": true});
                }
                crate::effort::apply(&mut body, self.spec, req);
                (self.url("chat/completions"), body)
            }
            Wire::Anthropic => {
                let mut body = wire::anthropic::chat_body(req);
                body["stream"] = serde_json::json!(true);
                crate::effort::apply(&mut body, self.spec, req);
                (self.url("messages"), body)
            }
            Wire::Gemini => {
                let model = req.model.strip_prefix("models/").unwrap_or(&req.model);
                let mut body = wire::gemini::chat_body(req);
                crate::effort::apply(&mut body, self.spec, req);
                (
                    self.url(&format!("models/{model}:streamGenerateContent?alt=sse")),
                    body,
                )
            }
        };
        let provider = self.spec.name;
        let rb = self
            .http
            .post(url)
            .json(&body)
            .header("accept", "text/event-stream")
            .timeout(STREAM_TIMEOUT);
        let mut resp = self
            .dispatch(self.with_wire_headers(self.authed(rb)))
            .await?;
        if !resp.status().is_success() {
            return Err(self
                .check(resp, &what)
                .await
                .err()
                .unwrap_or(ProviderError::Decode {
                    provider,
                    reason: "the stream failed".into(),
                }));
        }
        let mut parser = SseParser::default();
        let mut asm = Assembler::default();
        let mut split = crate::effort::ThinkSplitter::default();
        let inline = self.spec.wire == Wire::OpenAiCompatible;
        let mut stopped = false;
        // Sends a piece on: its reasoning, then its text (with any leading
        // `<think>` block moved over to reasoning). `false` stops.
        let mut forward = |piece: &mut stream::Piece| -> bool {
            if inline && !piece.text.is_empty() {
                let (r, t) = split.push(&piece.text);
                piece.reasoning.push_str(&r);
                piece.text = t;
            }
            if !piece.reasoning.is_empty() && !on(Delta::Reasoning(&piece.reasoning)) {
                return false;
            }
            !(!piece.text.is_empty() && !on(Delta::Text(&piece.text)))
        };
        'read: while let Some(chunk) = resp.chunk().await.map_err(|e| self.network(&e))? {
            for e in parser.push(&chunk) {
                let mut piece = stream::decode(self.spec.wire, &e);
                if let Some(err) = piece.error {
                    return Err(ProviderError::Network {
                        provider,
                        reason: format!("the stream reported an error: {}", short(&err)),
                    });
                }
                let go = forward(&mut piece);
                asm.add(&piece);
                if !go {
                    stopped = true;
                    break 'read;
                }
                if piece.done {
                    break 'read;
                }
            }
        }
        if !stopped {
            let mut tail = parser
                .finish()
                .map(|e| stream::decode(self.spec.wire, &e))
                .unwrap_or_default();
            forward(&mut tail);
            asm.add(&tail);
            let (r, t) = split.finish();
            let mut rest = stream::Piece {
                text: t,
                reasoning: r,
                ..Default::default()
            };
            if !rest.text.is_empty() || !rest.reasoning.is_empty() {
                if !rest.reasoning.is_empty() {
                    on(Delta::Reasoning(&rest.reasoning));
                }
                if !rest.text.is_empty() {
                    on(Delta::Text(&rest.text));
                }
                asm.add(&std::mem::take(&mut rest));
            }
        }
        Ok(asm.finish(req, stopped))
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
                .filter(|m| m.can_chat() && !m.is_retired())
                .count(),
            latency_ms: start.elapsed().as_millis(),
        })
    }
}

type Listings = std::collections::HashMap<String, (Instant, Vec<ModelInfo>)>;

/// Model listings by provider, address, and key fingerprint.
fn listings() -> &'static std::sync::Mutex<Listings> {
    static L: std::sync::OnceLock<std::sync::Mutex<Listings>> = std::sync::OnceLock::new();
    L.get_or_init(Default::default)
}

/// OpenAI-compatible servers known to accept `stream_options.include_usage`.
/// Others get no option (some reject unknown fields) and usage is estimated
/// unless they send it anyway.
fn reports_stream_usage(id: &str) -> bool {
    matches!(
        id,
        "openai"
            | "openrouter"
            | "groq"
            | "deepseek"
            | "together"
            | "fireworks"
            | "xai"
            | "cerebras"
            | "ollama"
    )
}

fn is_loopback(u: &url::Url) -> bool {
    match u.host() {
        Some(url::Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

/// The shared HTTP client: one for remote services (through the system
/// proxy, if any) and one for servers on this computer (never proxied).
fn shared_client(loopback: bool) -> Result<reqwest::Client, String> {
    use std::sync::OnceLock;
    static REMOTE: OnceLock<Result<reqwest::Client, String>> = OnceLock::new();
    static LOCAL: OnceLock<Result<reqwest::Client, String>> = OnceLock::new();
    let slot = if loopback { &LOCAL } else { &REMOTE };
    slot.get_or_init(|| {
        let mut b = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .connect_timeout(CONNECT_TIMEOUT)
            // Servers drop idle keep-alive connections; reusing one they
            // already closed fails the next call, so idle ones go sooner.
            .pool_idle_timeout(Duration::from_secs(30))
            .tcp_keepalive(Duration::from_secs(30))
            .user_agent(concat!("Tern/", env!("CARGO_PKG_VERSION")));
        if loopback {
            b = b.no_proxy();
        }
        b.build().map_err(|e| e.to_string())
    })
    .clone()
}

/// What went wrong with a request that never got an answer, in words the
/// user can act on: the DNS lookup, the TLS handshake, a refusal, a reset,
/// or a timeout. Only a server on this computer gets "is it running?".
/// reqwest's message includes the URL (Gemini once took keys in the query
/// string), so it is never shown as is.
fn describe_network(e: &reqwest::Error, base_url: &str) -> String {
    let mut chain = Vec::new();
    let mut src: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(e);
    while let Some(s) = src {
        chain.push(s.to_string());
        src = s.source();
    }
    let host = url::Url::parse(base_url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .unwrap_or_else(|| "the server".into());
    let local = url::Url::parse(base_url).is_ok_and(|u| is_loopback(&u));
    network_reason(
        &chain,
        &host,
        local,
        e.is_timeout(),
        e.is_connect(),
        &without_url(e),
    )
}

/// [`describe_network`] on plain strings, for tests: `chain` is the error's
/// sources, outermost first.
pub(crate) fn network_reason(
    chain: &[String],
    host: &str,
    local: bool,
    timed_out: bool,
    connect: bool,
    fallback: &str,
) -> String {
    let all = chain.join(" | ").to_ascii_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| all.contains(n));
    let inner = chain
        .last()
        .map(|s| short(s))
        .unwrap_or_else(|| fallback.to_owned());
    if has(&[
        "dns error",
        "failed to lookup",
        "no such host",
        "name or service not known",
        "nodename nor servname",
        "getaddrinfo",
        "11001",
    ]) {
        return format!(
            "could not look up {host} (DNS). Check the internet connection, VPN, or proxy settings."
        );
    }
    if has(&[
        "certificate",
        "invalid peer",
        "unknownissuer",
        "handshake",
        "tls",
        "ssl",
    ]) {
        return format!(
            "the secure (TLS) connection to {host} failed: {inner}. A proxy, VPN, or antivirus that inspects HTTPS can cause this."
        );
    }
    if has(&["proxy"]) {
        return format!("the proxy refused or failed the connection to {host}: {inner}.");
    }
    if timed_out || has(&["timed out", "10060"]) {
        return if connect || has(&["connect"]) {
            format!(
                "connecting to {host} took longer than {} s. The network may be blocking it.",
                CONNECT_TIMEOUT.as_secs()
            )
        } else {
            format!("{host} did not answer in time (the request timed out).")
        };
    }
    if has(&["refused", "10061"]) {
        return if local {
            format!("nothing is listening at {host}. Is the server running and the address right?")
        } else {
            format!("{host} refused the connection.")
        };
    }
    if has(&[
        "reset",
        "10054",
        "forcibly closed",
        "broken pipe",
        "connection closed",
        "unexpected eof",
    ]) {
        return format!("the connection to {host} was cut off ({inner}). Try again in a moment.");
    }
    if has(&["unreachable", "10051", "10065"]) {
        return format!("{host} is unreachable from this network.");
    }
    if connect {
        return if local {
            format!(
                "the connection to {host} failed ({inner}). Is the server running and the address right?"
            )
        } else {
            format!("the connection to {host} failed ({inner}).")
        };
    }
    fallback.to_owned()
}

fn without_url(e: &reqwest::Error) -> String {
    let mut s = e.to_string();
    if let Some(u) = e.url() {
        s = s.replace(u.as_str(), "<url>");
    }
    if e.is_timeout() {
        return "the request timed out".into();
    }
    s
}

/// The provider's own words from an error body (`error.message`,
/// `message`, `detail`), redacted and shortened; empty when there are none.
fn said(body: &str) -> String {
    let v: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
    let msg = v
        .pointer("/error/message")
        .or_else(|| v.get("message"))
        .or_else(|| v.get("detail"))
        .or_else(|| v.get("error"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| body.trim().to_owned());
    if msg.is_empty() || msg.starts_with('<') {
        return String::new();
    }
    short(&msg)
}

/// A `multipart/form-data` body: text fields, then one file.
fn multipart(boundary: &str, fields: &[(&str, &str)], file: (&str, &str, &[u8])) -> Vec<u8> {
    let mut out = Vec::with_capacity(file.2.len() + 512);
    for (k, v) in fields {
        out.extend_from_slice(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{v}\r\n")
                .as_bytes(),
        );
    }
    let (name, file_name, bytes) = file;
    let mime = match file_name
        .rsplit('.')
        .next()
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("wav") => "audio/wav",
        Some("mp3") | Some("mpga") | Some("mpeg") => "audio/mpeg",
        Some("m4a") | Some("mp4") => "audio/mp4",
        Some("ogg") => "audio/ogg",
        Some("webm") => "audio/webm",
        Some("flac") => "audio/flac",
        _ => "application/octet-stream",
    };
    let safe: String = file_name
        .chars()
        .map(|c| if c == '"' || c.is_control() { '_' } else { c })
        .collect();
    out.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"; filename=\"{safe}\"\r\nContent-Type: {mime}\r\n\r\n"
        )
        .as_bytes(),
    );
    out.extend_from_slice(bytes);
    out.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    out
}

/// Ollama's `/api/tags` names a model in `name` and says `capabilities` as
/// words; xAI's listing names modalities on top. Turns either into the
/// card shape `parse_models` reads.
fn with_ollama_caps(card: &serde_json::Value) -> serde_json::Value {
    let mut card = card.clone();
    if card.get("id").is_none()
        && let Some(n) = card.get("name").or_else(|| card.get("model")).cloned()
    {
        card["id"] = n;
    }
    if let Some(words) = card
        .get("capabilities")
        .and_then(serde_json::Value::as_array)
        .cloned()
    {
        let has = |w: &str| words.iter().any(|x| x.as_str() == Some(w));
        let mut flags = serde_json::Map::new();
        flags.insert("completion_chat".into(), has("completion").into());
        flags.insert("vision".into(), has("vision").into());
        flags.insert("function_calling".into(), has("tools").into());
        flags.insert("reasoning".into(), has("thinking").into());
        card["capabilities"] = serde_json::Value::Object(flags);
        if has("embedding") && !has("completion") {
            card["type"] = "embeddings".into();
            card.as_object_mut().map(|o| o.remove("capabilities"));
        }
    }
    card
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

#[cfg(test)]
mod network_tests {
    use super::network_reason;

    fn chain(s: &[&str]) -> Vec<String> {
        s.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn dns_failure_names_the_host_and_dns() {
        let r = network_reason(
            &chain(&[
                "client error (Connect)",
                "dns error: No such host is known. (os error 11001)",
            ]),
            "api.groq.com",
            false,
            false,
            true,
            "x",
        );
        assert!(r.contains("api.groq.com") && r.contains("DNS"), "{r}");
        assert!(!r.contains("server running"), "{r}");
    }

    #[test]
    fn tls_failure_mentions_inspection() {
        let r = network_reason(
            &chain(&[
                "client error (Connect)",
                "invalid peer certificate: UnknownIssuer",
            ]),
            "api.groq.com",
            false,
            false,
            true,
            "x",
        );
        assert!(r.contains("TLS") && r.contains("UnknownIssuer"), "{r}");
    }

    #[test]
    fn refused_only_asks_about_a_local_server() {
        let c = chain(&[
            "tcp connect error",
            "No connection could be made because the target machine actively refused it. (os error 10061)",
        ]);
        let local = network_reason(&c, "localhost", true, false, true, "x");
        assert!(local.contains("Is the server running"), "{local}");
        let remote = network_reason(&c, "api.mistral.ai", false, false, true, "x");
        assert_eq!(remote, "api.mistral.ai refused the connection.");
    }

    #[test]
    fn connect_timeout_and_reset_are_told_apart() {
        let t = network_reason(
            &chain(&["tcp connect error", "timed out"]),
            "api.groq.com",
            false,
            true,
            true,
            "x",
        );
        assert!(t.contains("took longer than 10 s"), "{t}");
        let r = network_reason(
            &chain(&[
                "connection error",
                "An existing connection was forcibly closed by the remote host. (os error 10054)",
            ]),
            "api.groq.com",
            false,
            false,
            false,
            "x",
        );
        assert!(r.contains("cut off"), "{r}");
    }

    #[test]
    fn unknown_errors_keep_the_plain_reason() {
        assert_eq!(
            network_reason(&[], "h", false, false, false, "plain"),
            "plain"
        );
    }
}

#[cfg(test)]
mod listing_cache_tests {
    use super::Provider;

    #[test]
    fn a_server_on_this_computer_is_never_cached() {
        let spec = crate::find("ollama").unwrap();
        for url in ["http://localhost:11434/v1", "http://127.0.0.1:9/v1"] {
            let p = Provider::new(spec, None, Some(url)).unwrap();
            assert!(p.listing_key().is_none(), "{url}");
            assert!(p.cached_models(super::LISTING_TTL).is_none());
        }
        let remote = Provider::new(spec, None, Some("https://models.example.com/v1")).unwrap();
        assert!(remote.listing_key().is_some());
    }
}
