use super::{client, post_json, Provider, ProviderError, Reply};
use async_trait::async_trait;
use serde_json::json;
use std::fmt;

pub struct Anthropic {
    base: String,
    model: String,
    key: String,
    http: reqwest::Client,
}

impl fmt::Debug for Anthropic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Anthropic").field("base", &self.base).field("model", &self.model).field("key", &"<redacted>").finish()
    }
}

impl Anthropic {
    pub fn new(base: String, model: String, key: String) -> Result<Self, ProviderError> {
        Ok(Anthropic { base: base.trim_end_matches('/').into(), model, key, http: client()? })
    }
}

#[async_trait]
impl Provider for Anthropic {
    async fn complete(&self, system: &str, user: &str) -> Result<Reply, ProviderError> {
        let body = json!({"model": self.model, "max_tokens": 8192,
            "system": [{"type": "text", "text": system, "cache_control": {"type": "ephemeral"}}],
            "messages": [{"role": "user", "content": user}]});
        let req = self.http.post(format!("{}/v1/messages", self.base)).header("x-api-key", &self.key).header("anthropic-version", "2023-06-01").json(&body);
        let v = post_json("anthropic", req).await?;
        let text: String = v["content"].as_array().into_iter().flatten().filter_map(|c| c["text"].as_str()).collect();
        if text.is_empty() {
            return Err(ProviderError::Permanent("anthropic: no text in response".into()));
        }
        let n = |p: &str| v["usage"][p].as_u64().unwrap_or(0) as u32;
        Ok(Reply { text, tokens_in: n("input_tokens"), tokens_out: n("output_tokens") })
    }
}

