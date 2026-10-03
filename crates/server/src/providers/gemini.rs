use super::{client, post_json, Provider, ProviderError, Reply};
use async_trait::async_trait;
use serde_json::json;
use std::fmt;

pub struct Gemini {
    base: String,
    model: String,
    key: String,
    http: reqwest::Client,
}

impl fmt::Debug for Gemini {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Gemini").field("base", &self.base).field("model", &self.model).field("key", &"<redacted>").finish()
    }
}

impl Gemini {
    pub fn new(base: String, model: String, key: String) -> Result<Self, ProviderError> {
        Ok(Gemini { base: base.trim_end_matches('/').into(), model, key, http: client()? })
    }
}

#[async_trait]
impl Provider for Gemini {
    async fn complete(&self, system: &str, user: &str) -> Result<Reply, ProviderError> {
        let body = json!({"systemInstruction": {"parts": [{"text": system}]},
            "contents": [{"role": "user", "parts": [{"text": user}]}],
            "generationConfig": {"responseMimeType": "application/json", "temperature": 0.3}});
        // Key goes in a header, never the URL, so it can't leak into logs.
        let req = self.http.post(format!("{}/v1beta/models/{}:generateContent", self.base, self.model)).header("x-goog-api-key", &self.key).json(&body);
        let v = post_json("gemini", req).await?;
        let text: String = v["candidates"][0]["content"]["parts"].as_array().into_iter().flatten().filter_map(|p| p["text"].as_str()).collect();
        if text.is_empty() {
            return Err(ProviderError::Permanent("gemini: no text in response".into()));
        }
        let n = |p: &str| v["usageMetadata"][p].as_u64().unwrap_or(0) as u32;
        Ok(Reply { text, tokens_in: n("promptTokenCount"), tokens_out: n("candidatesTokenCount") })
    }
}
