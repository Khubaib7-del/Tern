//! A plain JSON-over-HTTP transport for tool servers (MCP's Streamable HTTP).
//!
//! The network stays in this crate: callers hand over a URL, headers, and a
//! JSON body and get back the status, the reply headers they care about, and
//! the body as text. Protocol logic (JSON-RPC ids, SSE framing, sessions)
//! lives with the caller.

use std::time::Duration;

/// Why a request did not produce a reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RpcHttpError {
    /// The request or the body read ran out of time.
    Timeout,
    /// The client could not be built or the connection failed.
    Connect(String),
    /// The connection dropped while reading the reply.
    Read(String),
}

impl std::fmt::Display for RpcHttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Timeout => write!(f, "timed out"),
            Self::Connect(e) | Self::Read(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for RpcHttpError {}

/// A reply, as received.
#[derive(Debug, Clone)]
pub struct RpcHttpReply {
    /// The HTTP status code.
    pub status: u16,
    /// The `Content-Type` header, if any.
    pub content_type: Option<String>,
    /// The `Mcp-Session-Id` header, if any.
    pub session: Option<String>,
    /// The body as text.
    pub body: String,
}

impl RpcHttpReply {
    /// Whether the status is 2xx.
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// One server endpoint.
#[derive(Debug, Clone)]
pub struct RpcHttp {
    client: reqwest::Client,
    url: String,
}

impl RpcHttp {
    /// A client for `url`. A server on this machine is reached directly,
    /// never through a proxy.
    pub fn new(url: &str) -> Result<Self, RpcHttpError> {
        let mut b = reqwest::Client::builder().connect_timeout(Duration::from_secs(10));
        let local = url::Url::parse(url).ok().is_some_and(|u| {
            matches!(
                u.host_str(),
                Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
            )
        });
        if local {
            b = b.no_proxy();
        }
        let client = b
            .build()
            .map_err(|e| RpcHttpError::Connect(e.to_string()))?;
        Ok(Self {
            client,
            url: url.to_owned(),
        })
    }

    /// POST `body` as JSON with `headers`.
    pub async fn post(
        &self,
        headers: &[(String, String)],
        body: &serde_json::Value,
        timeout: Duration,
    ) -> Result<RpcHttpReply, RpcHttpError> {
        let mut req = self
            .client
            .post(&self.url)
            .timeout(timeout)
            .header("Content-Type", "application/json");
        for (k, v) in headers {
            req = req.header(k.as_str(), v.as_str());
        }
        let resp = req.json(body).send().await.map_err(|e| {
            if e.is_timeout() {
                RpcHttpError::Timeout
            } else {
                RpcHttpError::Connect(e.to_string())
            }
        })?;
        let header = |name: &str| {
            resp.headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
        };
        let status = resp.status().as_u16();
        let content_type = header("content-type");
        let session = header("mcp-session-id");
        let body = resp.text().await.map_err(|e| {
            if e.is_timeout() {
                RpcHttpError::Timeout
            } else {
                RpcHttpError::Read(e.to_string())
            }
        })?;
        Ok(RpcHttpReply {
            status,
            content_type,
            session,
            body,
        })
    }

    /// DELETE with `headers`, ignoring the outcome (ending a session).
    pub async fn delete(&self, headers: &[(String, String)], timeout: Duration) {
        let mut req = self.client.delete(&self.url).timeout(timeout);
        for (k, v) in headers {
            req = req.header(k.as_str(), v.as_str());
        }
        let _ = req.send().await;
    }
}
