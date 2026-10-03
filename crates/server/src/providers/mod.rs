//! LLM providers behind one trait. Real ones (gemini/anthropic) are built by `App::provider_for` once their key is loaded.
pub mod anthropic;
pub mod gemini;
pub mod mock;
use async_trait::async_trait;
use std::{fmt, sync::Arc, time::Duration};

#[derive(Debug, Clone, PartialEq)]
pub enum ProviderError {
    /// Network, 5xx, 429, timeout: worth retrying.
    Transient(String),
    Permanent(String),
}

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProviderError::Transient(m) | ProviderError::Permanent(m) => write!(f, "{m}"),
        }
    }
}
impl std::error::Error for ProviderError {}

#[derive(Debug, Clone)]
pub struct Reply {
    pub text: String,
    pub tokens_in: u32,
    pub tokens_out: u32,
}

#[async_trait]
pub trait Provider: Send + Sync {
    async fn complete(&self, system: &str, user: &str) -> Result<Reply, ProviderError>;
}

pub fn provider_for(name: &str) -> Result<Arc<dyn Provider>, ProviderError> {
    match name {
        "mock" => Ok(Arc::new(mock::MockProvider::default())),
        "gemini" | "anthropic" => Err(ProviderError::Permanent(format!("provider {name} needs App::provider_for (key vault)"))),
        _ => Err(ProviderError::Permanent(format!("unknown provider {name}"))),
    }
}

pub(crate) fn client() -> Result<reqwest::Client, ProviderError> {
    reqwest::Client::builder().connect_timeout(Duration::from_secs(10)).timeout(Duration::from_secs(90)).build().map_err(|e| ProviderError::Permanent(format!("http client: {}", e.without_url())))
}

/// POST json, map failures to Transient/Permanent. Messages carry only the status/kind: never headers, URL or bodies.
/// A Retry-After (seconds, capped at 30) is honoured by sleeping before returning Transient.
pub(crate) async fn post_json(name: &str, req: reqwest::RequestBuilder) -> Result<serde_json::Value, ProviderError> {
    let resp = req.send().await.map_err(|e| {
        let e = e.without_url();
        if e.is_timeout() || e.is_connect() || e.is_request() { ProviderError::Transient(format!("{name}: network error ({})", if e.is_timeout() { "timeout" } else { "connect/request" })) } else { ProviderError::Permanent(format!("{name}: request failed")) }
    })?;
    let st = resp.status();
    if st.as_u16() == 429 || st.as_u16() == 408 || st.is_server_error() {
        if let Some(s) = resp.headers().get("retry-after").and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse::<u64>().ok()) {
            tokio::time::sleep(Duration::from_secs(s.min(30))).await;
        }
        return Err(ProviderError::Transient(format!("{name}: HTTP {}", st.as_u16())));
    }
    if st.as_u16() == 404 {
        return Err(ProviderError::Permanent(format!("{name}: HTTP 404 model not available (set RESUME_{}_MODEL to a current model)", name.to_uppercase())));
    }
    if !st.is_success() {
        return Err(ProviderError::Permanent(format!("{name}: HTTP {}", st.as_u16())));
    }
    resp.json().await.map_err(|_| ProviderError::Permanent(format!("{name}: unparsable response body")))
}
