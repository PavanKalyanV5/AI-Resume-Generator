use super::{Provider, ProviderError, Reply};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::{atomic::{AtomicUsize, Ordering}, Mutex};

/// Deterministic: echoes every bullet id with its (still redacted) text. `failures` are popped first.
#[derive(Default)]
pub struct MockProvider {
    pub calls: AtomicUsize,
    pub failures: Mutex<Vec<ProviderError>>,
    /// Scripted reply for the plan-review call (default: no changes); review calls are counted apart from `calls`.
    pub review: Mutex<Option<String>>,
    pub review_calls: AtomicUsize,
    /// Scripted reply for the tailoring call (default: echo bullets with a generic summary).
    pub tailor: Mutex<Option<String>>,
    /// Scripted reply for the grounding self-repair call (default: no lines, so nothing is repaired); counted apart.
    pub repair: Mutex<Option<String>>,
    pub repair_calls: AtomicUsize,
    /// Every review prompt received, for payload assertions.
    pub review_prompts: Mutex<Vec<String>>,
}

impl MockProvider {
    pub fn with_failures(f: Vec<ProviderError>) -> Self {
        MockProvider { failures: Mutex::new(f), ..Default::default() }
    }
    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
    pub fn repair_calls(&self) -> usize {
        self.repair_calls.load(Ordering::SeqCst)
    }
    pub fn review_calls(&self) -> usize {
        self.review_calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl Provider for MockProvider {
    async fn complete(&self, system: &str, user: &str) -> Result<Reply, ProviderError> {
        let review = system.starts_with(resume_core::review::SYSTEM); // owner rules may be appended
        let repair = system == resume_core::grounding::REPAIR_SYSTEM;
        if repair {
            self.repair_calls.fetch_add(1, Ordering::SeqCst);
        } else if review {
            self.review_calls.fetch_add(1, Ordering::SeqCst);
            self.review_prompts.lock().unwrap().push(format!("{system}\n{user}"));
        } else {
            self.calls.fetch_add(1, Ordering::SeqCst);
        }
        {
            let mut f = self.failures.lock().unwrap();
            if !f.is_empty() {
                return Err(f.remove(0));
            }
        }
        if repair {
            let text = self.repair.lock().unwrap().clone().unwrap_or_else(|| r#"{"lines":[]}"#.into());
            return Ok(Reply { tokens_in: (user.len() / 4) as u32, tokens_out: (text.len() / 4) as u32, text });
        }
        if review {
            let text = self.review.lock().unwrap().clone().unwrap_or_else(|| r#"{"overrides":{},"issues":[],"confidence":0.5}"#.into());
            return Ok(Reply { tokens_in: (user.len() / 4) as u32, tokens_out: (text.len() / 4) as u32, text });
        }
        if let Some(text) = self.tailor.lock().unwrap().clone() {
            return Ok(Reply { tokens_in: (user.len() / 4) as u32, tokens_out: (text.len() / 4) as u32, text });
        }
        let v: Value = serde_json::from_str(user).map_err(|e| ProviderError::Permanent(e.to_string()))?;
        let bullets: Vec<Value> = v["bullets"].as_array().into_iter().flatten().map(|b| json!({"id": b["id"], "text": b["text"]})).collect();
        let text = json!({"summary": if v["summary"].as_array().is_some_and(|s| !s.is_empty()) { v["summary"].clone() } else { json!(["Engineer who ships reliable systems."]) }, "bullets": bullets}).to_string();
        Ok(Reply { tokens_in: (user.len() / 4) as u32, tokens_out: (text.len() / 4) as u32, text })
    }
}
