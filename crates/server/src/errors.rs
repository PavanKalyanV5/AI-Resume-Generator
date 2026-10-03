//! Structured, user-facing errors. Existing code reports plain strings; `classify` maps them to a code.
//! Messages are the (already secret-free) provider/drive text, except redact.leak whose raw text lists the leaked values.
use crate::fetch::FetchErr;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct AppError {
    pub code: String,
    pub title: String,
    pub message: String,
    pub hint: String,
    pub retryable: bool,
    pub step: String,
}

/// (title, hint, retryable, primary action) per code.
fn meta(code: &str) -> (&'static str, &'static str, bool, &'static str) {
    match code {
        "ai.rate_limited" => ("The AI provider is busy", "Wait a minute and retry, or pick another provider.", true, "retry"),
        "ai.model_unavailable" => ("The AI model is no longer available", "The default model was retired. Set RESUME_GEMINI_MODEL / RESUME_ANTHROPIC_MODEL to a current model and restart.", false, "settings"),
        "ai.auth" => ("The AI provider rejected your key", "Check or re-enter the API key in Settings.", false, "settings"),
        "ai.budget_exceeded" => ("The request is over your token budget", "Raise the job's token limit or trim the resume, then retry.", true, "retry"),
        "ai.timeout" => ("The AI provider did not answer in time", "Retry; if it keeps happening try again later.", true, "retry"),
        "ai.bad_reply" => ("The AI reply could not be used", "Retry to ask again; nothing was sent twice.", true, "retry"),
        "redact.leak" => ("Blocked: private details would have been sent", "Nothing was sent. Add the value to your redaction rules, then run the job again.", false, "open"),
        "grounding.reverted" => ("Some AI rewrites were reverted", "They claimed facts your resume does not support, so the original wording was kept.", false, "open"),
        "render.fit_failed" => ("The resume could not be fitted to the page target", "Try a different page target or trim content.", true, "retry"),
        "render.tex_failed" => ("PDF typesetting failed", "Retry; the DOCX is unaffected.", true, "retry"),
        "drive.not_connected" => ("Google Drive is not connected", "Connect Google Drive in Settings, then retry the upload.", false, "settings"),
        "drive.quota" => ("Google Drive refused the upload", "Free some space or wait for the quota to reset, then retry.", true, "retry"),
        "fetch.login_wall" => ("That page needs a login or JavaScript", "Open it in your browser and paste the description instead.", false, "dismiss"),
        "fetch.blocked" => ("That site blocked the download", "Paste the job description text instead.", false, "dismiss"),
        "import.low_confidence" => ("The resume was only partly understood", "Review the imported fields before using them.", false, "dismiss"),
        "net.offline" => ("No network connection", "Check your connection and retry.", true, "retry"),
        _ => ("Something went wrong", "Retry; if it persists restart the app.", true, "retry"), // server.unreachable
    }
}

impl AppError {
    pub fn new(code: &str, message: &str, step: &str) -> Self {
        let (title, hint, retryable, _) = meta(code);
        AppError { code: code.into(), title: title.into(), message: message.into(), hint: hint.into(), retryable, step: step.into() }
    }

    /// Map a step failure message (as produced by providers/drive/core) to a code.
    pub fn classify(step: &str, msg: &str) -> Self {
        let m = msg.to_lowercase();
        let has = |ks: &[&str]| ks.iter().any(|k| m.contains(k));
        let net = has(&["network error", "could not connect", "could not resolve"]);
        let code = if m.contains("payload leaks") {
            return Self::new("redact.leak", "The redacted payload still contained private values, so nothing was sent.", step);
        } else if m.contains("exceeds job budget") {
            "ai.budget_exceeded"
        } else if step == "upload_drive" {
            if has(&["not connected", "reconnect", "not configured"]) { "drive.not_connected" } else if net { "net.offline" } else { "drive.quota" }
        } else if step == "ai_tailor" || step == "restore" {
            if m.contains("model not available") { "ai.model_unavailable" }
            else if has(&["no api key", "http 401", "http 403", "unknown provider", "provider_for"]) { "ai.auth" }
            else if m.contains("http 429") { "ai.rate_limited" }
            else if net && !has(&["timeout"]) { "net.offline" }
            else if has(&["timeout", "timed out", "http 5"]) { "ai.timeout" }
            else { "ai.bad_reply" }
        } else if step.starts_with("render_") {
            if has(&["tectonic", "latex"]) { "render.tex_failed" } else { "render.fit_failed" }
        } else {
            "server.unreachable"
        };
        Self::new(code, msg, step)
    }

    pub fn from_fetch(e: &FetchErr) -> Self {
        let (code, m) = match e {
            FetchErr::Unusable(m) => ("fetch.login_wall", m),
            FetchErr::Bad(m) => ("fetch.blocked", m),
            FetchErr::Upstream(m) if m.contains("resolve") || m.contains("connect") || m.contains("timed out") => ("net.offline", m),
            FetchErr::Upstream(m) => ("fetch.blocked", m),
        };
        Self::new(code, m, "fetch")
    }

    /// [primary action for the code, optional open-job action]. retry targets the failed step.
    pub fn actions(&self, job: Option<&str>) -> Value {
        let mut a = vec![match (meta(&self.code).3, self.code.as_str()) {
            ("retry", _) => json!({"label": "Retry", "action": "retry", "target": self.step}),
            ("settings", _) => json!({"label": "Open settings", "action": "settings"}),
            ("open", "redact.leak") => json!({"label": "Review privacy rules", "action": "open", "target": "/pii"}),
            (_, _) => json!({"label": "Dismiss", "action": "dismiss"}),
        }];
        if let Some(j) = job {
            a.push(json!({"label": "Open job", "action": "open", "target": format!("/jobs/{j}")}));
        }
        json!(a)
    }
}

/// jobs.error column -> JSON: structured object, or the plain string of old rows.
pub fn err_json(s: Option<String>) -> Value {
    s.map_or(Value::Null, |s| serde_json::from_str::<AppError>(&s).map(|e| json!(e)).unwrap_or(Value::String(s)))
}
