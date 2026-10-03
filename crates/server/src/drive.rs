//! Google Drive: OAuth installed-app flow (PKCE, loopback redirect, scope drive.file) and idempotent uploads.
//! Secrets (client secret, refresh token) live in the encrypted key vault; access tokens only in memory. Nothing secret is logged.
use crate::{keys, providers::ProviderError, App};
use anyhow::Result;
use axum::{extract::{Query, State}, http::StatusCode, response::Html, routing::{get, post, put}, Json, Router};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64, Engine};
use rand::RngCore;
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, path::Path, sync::{Arc, Mutex}, time::{Duration, Instant}};

const SCOPE: &str = "https://www.googleapis.com/auth/drive.file";
const FOLDER_MIME: &str = "application/vnd.google-apps.folder";
const OAUTH: &str = "google_oauth"; // vault entry: {"client_id","client_secret"}
const REFRESH: &str = "google_refresh"; // vault entry: refresh token

#[derive(Default)]
pub struct DriveState {
    /// state -> (pkce verifier, expiry); single use.
    pending: Mutex<HashMap<String, (String, Instant)>>,
    token: Mutex<Option<(String, Instant)>>,
}

fn env(k: &str, d: &str) -> String {
    std::env::var(k).ok().filter(|v| !v.is_empty()).unwrap_or(d.into())
}
fn token_url() -> String { env("RESUME_GOOGLE_TOKEN_URL", "https://oauth2.googleapis.com/token") }
fn api_url() -> String { env("RESUME_GOOGLE_API_URL", "https://www.googleapis.com") }
fn upload_url() -> String { env("RESUME_GOOGLE_UPLOAD_URL", "https://www.googleapis.com") }
pub fn redirect_uri() -> String { format!("http://127.0.0.1:{}/api/drive/callback", env("PORT", "8787")) }

fn rand_b64(n: usize) -> String {
    let mut b = vec![0u8; n];
    rand::rngs::OsRng.fill_bytes(&mut b);
    B64.encode(b)
}
pub fn pkce_challenge(verifier: &str) -> String {
    B64.encode(Sha256::digest(verifier.as_bytes()))
}

fn net(e: reqwest::Error) -> ProviderError {
    let e = e.without_url();
    if e.is_timeout() || e.is_connect() || e.is_request() { ProviderError::Transient("Google: network error".into()) } else { ProviderError::Permanent("Google: request failed".into()) }
}
fn status_err(st: reqwest::StatusCode) -> Option<ProviderError> {
    let n = st.as_u16();
    if n == 429 || n == 408 || st.is_server_error() { Some(ProviderError::Transient(format!("Google Drive: HTTP {n}"))) }
    else if !st.is_success() { Some(ProviderError::Permanent(format!("Google Drive: HTTP {n}"))) } else { None }
}
fn pe(e: anyhow::Error) -> ProviderError { ProviderError::Permanent(e.to_string()) }
fn q(s: &str) -> String { s.replace('\\', "\\\\").replace('\'', "\\'") }

impl App {
    async fn secret(self: &Arc<Self>, name: &'static str) -> Result<Option<String>> {
        let blob: Option<Vec<u8>> = self.db(move |c| Ok(c.query_row("SELECT blob FROM keys WHERE provider=?", [name], |r| r.get(0)).optional()?)).await?;
        blob.map(|b| keys::decrypt(&self.key_file, name, &b)).transpose()
    }
    async fn put_secret(self: &Arc<Self>, name: &'static str, v: String) -> Result<()> {
        let kf = self.key_file.clone();
        self.db(move |c| { let b = keys::encrypt(&kf, name, &v)?; c.execute("INSERT OR REPLACE INTO keys VALUES(?,?)", params![name, b])?; Ok(()) }).await
    }
    pub(crate) async fn setting(self: &Arc<Self>, k: &'static str) -> Option<String> {
        self.db(move |c| Ok(c.query_row("SELECT value FROM settings WHERE key=?", [k], |r| r.get::<_, String>(0)).optional()?)).await.ok().flatten()
    }
    pub(crate) async fn set_setting(self: &Arc<Self>, k: &'static str, v: Option<String>) -> Result<()> {
        self.db(move |c| { match v { Some(v) => c.execute("INSERT OR REPLACE INTO settings VALUES(?,?)", params![k, v])?, None => c.execute("DELETE FROM settings WHERE key=?", [k])? }; Ok(()) }).await
    }
    /// Stored client wins over env.
    async fn oauth_client(self: &Arc<Self>) -> Option<(String, String)> {
        if let Ok(Some(s)) = self.secret(OAUTH).await {
            if let Ok(v) = serde_json::from_str::<Value>(&s) {
                return Some((v["client_id"].as_str()?.into(), v["client_secret"].as_str().unwrap_or("").into()));
            }
        }
        let id = std::env::var("GOOGLE_OAUTH_CLIENT_ID").ok().filter(|v| !v.is_empty())?;
        Some((id, std::env::var("GOOGLE_OAUTH_CLIENT_SECRET").unwrap_or_default()))
    }
    pub async fn drive_connected(self: &Arc<Self>) -> bool {
        matches!(self.secret(REFRESH).await, Ok(Some(_)))
    }

    async fn token_call(self: &Arc<Self>, form: Vec<(&str, String)>) -> Result<Value, ProviderError> {
        let resp = crate::providers::client()?.post(token_url()).form(&form).send().await.map_err(net)?;
        let st = resp.status();
        let body: Value = resp.json().await.unwrap_or(Value::Null);
        if let Some(e) = status_err(st) {
            return Err(match e {
                ProviderError::Permanent(_) => ProviderError::Permanent(format!("Google rejected the authorization ({}); reconnect Google Drive in Settings", body["error"].as_str().unwrap_or("error"))),
                t => t,
            });
        }
        Ok(body)
    }

    /// Cached access token, refreshed with the stored refresh token when expired.
    async fn access_token(self: &Arc<Self>) -> Result<String, ProviderError> {
        if let Some((t, exp)) = self.drive.token.lock().unwrap().clone() {
            if exp > Instant::now() { return Ok(t); }
        }
        let rt = self.secret(REFRESH).await.map_err(pe)?.ok_or_else(|| ProviderError::Permanent("Google Drive is not connected: connect it in Settings, then retry from the upload step".into()))?;
        let (id, sec) = self.oauth_client().await.ok_or_else(|| ProviderError::Permanent("Google OAuth client is not configured".into()))?;
        let r = self.token_call(vec![("grant_type", "refresh_token".into()), ("refresh_token", rt), ("client_id", id), ("client_secret", sec)]).await?;
        let t = r["access_token"].as_str().ok_or_else(|| ProviderError::Permanent("Google: no access token in response".into()))?.to_string();
        let ttl = r["expires_in"].as_u64().unwrap_or(3600).saturating_sub(60);
        *self.drive.token.lock().unwrap() = Some((t.clone(), Instant::now() + Duration::from_secs(ttl)));
        Ok(t)
    }

    /// Authenticated JSON request; one transparent refresh on 401.
    async fn gapi(self: &Arc<Self>, build: impl Fn(&reqwest::Client) -> reqwest::RequestBuilder) -> Result<Value, ProviderError> {
        let c = crate::providers::client()?;
        for attempt in 0..2 {
            let t = self.access_token().await?;
            let resp = build(&c).bearer_auth(t).send().await.map_err(net)?;
            if resp.status().as_u16() == 401 && attempt == 0 {
                *self.drive.token.lock().unwrap() = None;
                continue;
            }
            if let Some(e) = status_err(resp.status()) {
                return Err(if resp.status().as_u16() == 401 { ProviderError::Permanent("Google Drive rejected the credentials; reconnect in Settings".into()) } else { e });
            }
            return resp.json().await.map_err(|_| ProviderError::Permanent("Google Drive: unparsable response".into()));
        }
        unreachable!()
    }

    async fn find_or_create_folder(self: &Arc<Self>, name: &str) -> Result<String, ProviderError> {
        let url = format!("{}/drive/v3/files", api_url());
        let query = format!("name='{}' and mimeType='{FOLDER_MIME}' and trashed=false", q(name));
        let r = self.gapi(|c| c.get(&url).query(&[("q", query.as_str()), ("fields", "files(id)"), ("spaces", "drive")])).await?;
        if let Some(id) = r["files"][0]["id"].as_str() { return Ok(id.into()); }
        let r = self.gapi(|c| c.post(&url).query(&[("fields", "id")]).json(&json!({"name": name, "mimeType": FOLDER_MIME}))).await?;
        r["id"].as_str().map(String::from).ok_or_else(|| ProviderError::Permanent("Google Drive: folder create returned no id".into()))
    }

    /// Upload `files` (kind = "pdf"|"docx", display name) from `dir/resume.<kind>`; re-uses a file already tagged with the same job_id+kind.
    pub async fn drive_upload(self: &Arc<Self>, job: &str, dir: &Path, files: &[(&str, String)]) -> Result<Vec<Value>, ProviderError> {
        if !self.drive_connected().await {
            return Err(ProviderError::Permanent("Google Drive is not connected: connect it in Settings, then retry from the upload step".into()));
        }
        let folder = self.find_or_create_folder(&self.drive_folder().await).await?;
        let (api, up) = (format!("{}/drive/v3/files", api_url()), format!("{}/upload/drive/v3/files", upload_url()));
        let mut out = vec![];
        for (kind, name) in files {
            let bytes = tokio::fs::read(dir.join(format!("resume.{kind}"))).await.map_err(|e| ProviderError::Permanent(format!("cannot read resume.{kind}: {e}")))?;
            let mime = if *kind == "pdf" { "application/pdf" } else { "application/vnd.openxmlformats-officedocument.wordprocessingml.document" };
            let query = format!("'{}' in parents and trashed=false and appProperties has {{ key='job_id' and value='{}' }} and appProperties has {{ key='kind' and value='{kind}' }}", q(&folder), q(job));
            let found = self.gapi(|c| c.get(&api).query(&[("q", query.as_str()), ("fields", "files(id)")])).await?;
            let existing = found["files"][0]["id"].as_str().map(String::from);
            let mut meta = json!({"name": name, "appProperties": {"job_id": job, "kind": kind}});
            if existing.is_none() { meta["parents"] = json!([folder]); }
            let boundary = format!("b{}", uuid::Uuid::new_v4().simple());
            let mut body = format!("--{boundary}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n{meta}\r\n--{boundary}\r\nContent-Type: {mime}\r\n\r\n").into_bytes();
            body.extend_from_slice(&bytes);
            body.extend_from_slice(format!("\r\n--{boundary}--").as_bytes());
            let ct = format!("multipart/related; boundary={boundary}");
            let r = self.gapi(|c| {
                let rb = match &existing { Some(id) => c.patch(format!("{up}/{id}")), None => c.post(&up) };
                rb.query(&[("uploadType", "multipart"), ("fields", "id,webViewLink")]).header("content-type", &ct).body(body.clone())
            }).await?;
            let id = r["id"].as_str().ok_or_else(|| ProviderError::Permanent("Google Drive: upload returned no id".into()))?;
            out.push(json!({"name": name, "url": r["webViewLink"].as_str().map(String::from).unwrap_or_else(|| format!("https://drive.google.com/file/d/{id}/view")), "id": id}));
        }
        Ok(out)
    }
}

type E = (StatusCode, String);
type S = State<Arc<App>>;
fn bad(s: &str) -> E { (StatusCode::BAD_REQUEST, s.into()) }
fn ie(e: impl std::fmt::Display) -> E { (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()) }

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/drive", get(status).put(put_folder))
        .route("/api/drive/oauth", put(put_oauth))
        .route("/api/drive/connect", post(connect))
        .route("/api/drive/callback", get(callback))
        .route("/api/drive/disconnect", post(disconnect))
}

async fn status(State(a): S) -> Json<Value> {
    Json(json!({"configured": a.oauth_client().await.is_some(), "connected": a.drive_connected().await, "folder_name": a.drive_folder().await,
        "account_email": a.setting("drive_email").await, "redirect_uri": redirect_uri()}))
}

async fn put_folder(State(a): S, Json(b): Json<Value>) -> Result<StatusCode, E> {
    let n = crate::sanitize_name(b["folder_name"].as_str().unwrap_or(""), 100);
    a.set_setting("drive_folder", (!n.is_empty()).then_some(n)).await.map_err(ie)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn put_oauth(State(a): S, Json(b): Json<Value>) -> Result<StatusCode, E> {
    let id = b["client_id"].as_str().map(str::trim).filter(|s| !s.is_empty()).ok_or_else(|| bad("client_id required"))?;
    let sec = b["client_secret"].as_str().map(str::trim).unwrap_or("");
    a.put_secret(OAUTH, json!({"client_id": id, "client_secret": sec}).to_string()).await.map_err(ie)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn connect(State(a): S) -> Result<Json<Value>, E> {
    let (id, _) = a.oauth_client().await.ok_or_else(|| bad("Google OAuth client is not configured: set GOOGLE_OAUTH_CLIENT_ID/GOOGLE_OAUTH_CLIENT_SECRET or add them in Settings"))?;
    let (verifier, state) = (rand_b64(32), rand_b64(24));
    {
        let mut p = a.drive.pending.lock().unwrap();
        p.retain(|_, (_, exp)| *exp > Instant::now());
        p.insert(state.clone(), (verifier.clone(), Instant::now() + Duration::from_secs(600)));
    }
    let base = env("RESUME_GOOGLE_AUTH_URL", "https://accounts.google.com/o/oauth2/v2/auth");
    let url = reqwest::Url::parse_with_params(&base, [("response_type", "code"), ("client_id", id.as_str()), ("redirect_uri", redirect_uri().as_str()), ("scope", SCOPE), ("state", state.as_str()),
        ("code_challenge", pkce_challenge(&verifier).as_str()), ("code_challenge_method", "S256"), ("access_type", "offline"), ("prompt", "consent")]).map_err(ie)?;
    Ok(Json(json!({"auth_url": url.as_str()})))
}

#[derive(Deserialize)]
struct Cb { code: Option<String>, state: Option<String>, error: Option<String> }

fn page(ok: bool, msg: &str) -> (StatusCode, Html<String>) {
    (if ok { StatusCode::OK } else { StatusCode::BAD_REQUEST }, Html(format!("<!doctype html><meta charset=utf-8><title>Google Drive</title><body style=\"font-family:sans-serif;margin:3em\"><h2>{msg}</h2></body>")))
}

async fn callback(State(a): S, Query(q): Query<Cb>) -> (StatusCode, Html<String>) {
    let pending = q.state.as_deref().and_then(|s| a.drive.pending.lock().unwrap().remove(s)); // single use
    let Some((verifier, exp)) = pending else { return page(false, "Invalid or expired sign-in request. Start again from Settings.") };
    if exp < Instant::now() { return page(false, "Sign-in request expired. Start again from Settings."); }
    let Some(code) = q.code.filter(|_| q.error.is_none()) else { return page(false, "Google did not authorize access. You can close this tab.") };
    let Some((id, sec)) = a.oauth_client().await else { return page(false, "Google OAuth client is not configured.") };
    let r = a.token_call(vec![("grant_type", "authorization_code".into()), ("code", code), ("client_id", id), ("client_secret", sec), ("redirect_uri", redirect_uri()), ("code_verifier", verifier)]).await;
    let Ok(r) = r else { return page(false, "Could not complete the Google sign-in. Try again.") };
    let Some(rt) = r["refresh_token"].as_str() else { return page(false, "Google returned no refresh token. Remove the app's access in your Google account and retry.") };
    if a.put_secret(REFRESH, rt.into()).await.is_err() { return page(false, "Could not store the credentials."); }
    if let Some(t) = r["access_token"].as_str() {
        *a.drive.token.lock().unwrap() = Some((t.into(), Instant::now() + Duration::from_secs(r["expires_in"].as_u64().unwrap_or(3600).saturating_sub(60))));
    }
    let url = format!("{}/drive/v3/about", api_url());
    let email = a.gapi(|c| c.get(&url).query(&[("fields", "user(emailAddress)")])).await.ok().and_then(|v| v["user"]["emailAddress"].as_str().map(String::from));
    let _ = a.set_setting("drive_email", email).await;
    page(true, "Connected, you can close this tab.")
}

async fn disconnect(State(a): S) -> Result<StatusCode, E> {
    if let Ok(Some(rt)) = a.secret(REFRESH).await {
        let url = env("RESUME_GOOGLE_REVOKE_URL", &format!("{}/revoke", token_url().trim_end_matches("/token")));
        if let Ok(c) = crate::providers::client() { let _ = c.post(url).form(&[("token", rt)]).send().await; } // best effort
    }
    *a.drive.token.lock().unwrap() = None;
    a.db(|c| Ok(c.execute("DELETE FROM keys WHERE provider=?", [REFRESH])?)).await.map_err(ie)?;
    a.set_setting("drive_email", None).await.map_err(ie)?;
    Ok(StatusCode::NO_CONTENT)
}

