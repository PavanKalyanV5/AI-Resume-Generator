//! Real provider adapters, key vault and HTTP routes against a local fake vendor (no real network, no real keys).
use axum::{body::{to_bytes, Body}, extract::{Path as AxPath, State}, http::{HeaderMap, Request, StatusCode, Uri}, response::IntoResponse, routing::post, Json, Router};
use resume_server::{api::router, create_job, keys, providers::{anthropic::Anthropic, gemini::Gemini, Provider, ProviderError}, App};
use serde_json::{json, Value};
use std::{collections::HashMap, os::unix::fs::PermissionsExt, path::PathBuf, sync::{Arc, Mutex, OnceLock}};
use tower::ServiceExt;

#[derive(Default)]
struct Fake {
    seen: Mutex<Vec<(String, String, Value)>>, // (uri, api key header, body)
    hits: Mutex<HashMap<String, u32>>,
}

/// Behaviour is selected by the API key prefix: retry-* = 429(Retry-After: 0) once then ok, bad-* = 401, boom-* = 500, junk-* = 200 non-json.
async fn vendor(f: Arc<Fake>, uri: Uri, h: HeaderMap, body: Value, gemini: bool) -> axum::response::Response {
    let hdr = if gemini { "x-goog-api-key" } else { "x-api-key" };
    let key = h.get(hdr).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    f.seen.lock().unwrap().push((uri.to_string(), key.clone(), body.clone()));
    let n = { let mut m = f.hits.lock().unwrap(); let e = m.entry(key.clone()).or_default(); *e += 1; *e };
    if key.starts_with("bad-") { return StatusCode::UNAUTHORIZED.into_response(); }
    if key.starts_with("boom-") { return StatusCode::INTERNAL_SERVER_ERROR.into_response(); }
    if key.starts_with("junk-") { return "not json".into_response(); }
    if key.starts_with("retry-") && n == 1 { return (StatusCode::TOO_MANY_REQUESTS, [("retry-after", "0")], "slow down").into_response(); }
    let user = if gemini { body["contents"][0]["parts"][0]["text"].as_str() } else { body["messages"][0]["content"].as_str() }.unwrap_or("");
    let text = match serde_json::from_str::<Value>(user) {
        Ok(v) if v["bullets"].is_array() => {
            let b: Vec<Value> = v["bullets"].as_array().unwrap().iter().map(|b| json!({"id": b["id"], "text": b["text"]})).collect();
            json!({"summary": v["summary"], "bullets": b}).to_string()
        }
        _ => format!("echo:{user}"),
    };
    if gemini {
        Json(json!({"candidates": [{"content": {"parts": [{"text": &text[..text.len() / 2]}, {"text": &text[text.len() / 2..]}]}}], "usageMetadata": {"promptTokenCount": 11, "candidatesTokenCount": 7}})).into_response()
    } else {
        Json(json!({"content": [{"type": "text", "text": text}], "usage": {"input_tokens": 11, "output_tokens": 7}})).into_response()
    }
}

/// One shared fake for the whole test binary; env vars point the engine at it.
fn fake() -> &'static (Arc<Fake>, String) {
    static F: OnceLock<(Arc<Fake>, String)> = OnceLock::new();
    F.get_or_init(|| {
        let f = Arc::new(Fake::default());
        let app = Router::new()
            .route("/v1/messages", post(|State(f): State<Arc<Fake>>, u: Uri, h: HeaderMap, Json(b): Json<Value>| vendor(f, u, h, b, false)))
            .route("/v1beta/models/:m", post(|State(f): State<Arc<Fake>>, AxPath(_m): AxPath<String>, u: Uri, h: HeaderMap, Json(b): Json<Value>| vendor(f, u, h, b, true)))
            .with_state(f.clone());
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.set_nonblocking(true).unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        // dedicated runtime so the fake outlives each #[tokio::test] runtime
        std::thread::spawn(move || {
            tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap().block_on(async move {
                axum::serve(tokio::net::TcpListener::from_std(l).unwrap(), app).await.unwrap();
            })
        });
        std::env::set_var("RESUME_ANTHROPIC_URL", &url);
        std::env::set_var("RESUME_GEMINI_URL", &url);
        std::env::set_var("NO_PROXY", "127.0.0.1");
        std::env::remove_var("RESUME_ANTHROPIC_MODEL");
        std::env::remove_var("RESUME_GEMINI_MODEL");
        (f, url)
    })
}

/// Serialises tests that read/write provider key env vars; clears them on entry.
async fn env_lock() -> tokio::sync::MutexGuard<'static, ()> {
    static L: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let g = L.lock().await;
    for k in ["ANTHROPIC_API_KEY", "GEMINI_API_KEY", "GOOGLE_API_KEY"] { std::env::remove_var(k); }
    g
}

fn calls_for(key: &str) -> Vec<(String, String, Value)> {
    fake().0.seen.lock().unwrap().iter().filter(|s| s.1 == key).cloned().collect()
}

#[tokio::test]
async fn anthropic_request_shape_parsing_and_header_auth() {
    let (_, url) = fake();
    let p = Anthropic::new(url.clone(), "m1".into(), "ok-anth-1".into()).unwrap();
    let r = p.complete("SYS", "hello").await.unwrap();
    assert_eq!((r.text.as_str(), r.tokens_in, r.tokens_out), ("echo:hello", 11, 7));
    let (uri, _, body) = calls_for("ok-anth-1").remove(0);
    assert_eq!(uri, "/v1/messages");
    assert!(!uri.contains("ok-anth-1"));
    assert_eq!(body["model"], "m1");
    assert!(body["max_tokens"].as_u64().unwrap() > 0);
    assert_eq!(body["system"][0], json!({"type": "text", "text": "SYS", "cache_control": {"type": "ephemeral"}}));
    assert_eq!(body["messages"][0], json!({"role": "user", "content": "hello"}));
    assert!(!body.to_string().contains("ok-anth-1"));
    assert!(!format!("{p:?}").contains("ok-anth-1"));
}

#[tokio::test]
async fn gemini_request_shape_parsing_and_header_auth() {
    let (_, url) = fake();
    let p = Gemini::new(url.clone(), "g1".into(), "ok-gem-1".into()).unwrap();
    let r = p.complete("SYS", "hello world").await.unwrap();
    assert_eq!((r.text.as_str(), r.tokens_in, r.tokens_out), ("echo:hello world", 11, 7)); // parts concatenated
    let (uri, _, body) = calls_for("ok-gem-1").remove(0);
    assert_eq!(uri, "/v1beta/models/g1:generateContent");
    assert!(!uri.contains("ok-gem-1") && !uri.contains('?'));
    assert_eq!(body["systemInstruction"]["parts"][0]["text"], "SYS");
    assert_eq!(body["contents"][0]["parts"][0]["text"], "hello world");
    assert_eq!(body["generationConfig"], json!({"responseMimeType": "application/json", "temperature": 0.3}));
    assert!(!format!("{p:?}").contains("ok-gem-1"));
}

#[tokio::test]
async fn error_mapping_and_no_secret_in_messages() {
    let (_, url) = fake();
    for gemini in [false, true] {
        let mk = |k: &str| -> Box<dyn Provider> {
            if gemini { Box::new(Gemini::new(url.clone(), "g".into(), k.into()).unwrap()) } else { Box::new(Anthropic::new(url.clone(), "m".into(), k.into()).unwrap()) }
        };
        let e = mk("bad-secret-key").complete("s", "u").await.unwrap_err();
        assert!(matches!(e, ProviderError::Permanent(_)), "401 permanent");
        assert!(!e.to_string().contains("bad-secret-key") && !e.to_string().contains("\"u\""));
        assert!(matches!(mk("boom-1").complete("s", "u").await.unwrap_err(), ProviderError::Transient(_)), "500 transient");
        assert!(matches!(mk("junk-1").complete("s", "u").await.unwrap_err(), ProviderError::Permanent(_)), "unparsable permanent");
        // 429 with Retry-After: 0 is Transient; next call succeeds
        let k = format!("retry-direct-{gemini}");
        assert!(matches!(mk(&k).complete("s", "u").await.unwrap_err(), ProviderError::Transient(_)));
        assert!(mk(&k).complete("s", "u").await.is_ok());
    }
    // connection refused -> Transient
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let dead = format!("http://{}", l.local_addr().unwrap());
    drop(l);
    let e = Anthropic::new(dead, "m".into(), "k".into()).unwrap().complete("s", "u").await.unwrap_err();
    assert!(matches!(e, ProviderError::Transient(_)), "{e}");
}

fn setup() -> (Arc<App>, PathBuf) {
    fake();
    let dir = std::env::temp_dir().join(format!("rs-vendor-{}", uuid::Uuid::new_v4()));
    let app = App::open(&dir.join("app.db"), &dir.join("out")).unwrap();
    app.conn.lock().unwrap().execute("INSERT INTO resume VALUES(1,?)", [resume()]).unwrap();
    let app = Arc::try_unwrap(app).ok().map(|mut a| { a.backoff_ms = 1; a.key_file = dir.join("master.key"); Arc::new(a) }).unwrap();
    (app, dir)
}

fn resume() -> String {
    json!({"profile": {"name": "Jane Doe", "email": "jane.doe@acme-robotics.test", "phone": "+1 555 010 4477", "role": "Engineer", "location": "Springfield"},
        "summary": ["Engineer who ships."],
        "experience": [{"id": "x1", "role": "Backend Engineer", "organization": "Acme Robotics Pvt Ltd", "client": "Globex Corp", "dateLabel": "2020-2024",
            "bullets": ["Built Python services on Kubernetes for Globex Corp, cutting latency 40%", "Maintained C# and .NET billing code"]}],
        "skills": [{"label": "Cloud", "skills": ["Python", "Terraform", "Kubernetes"]}]}).to_string()
}
const JD: &str = "Senior Backend Engineer\nRequirements:\n- 5+ years of Python and Kubernetes\n- Strong C# and .NET experience\n";

fn put_key(a: &App, provider: &str, key: &str) {
    let blob = keys::encrypt(&a.key_file, provider, key).unwrap();
    a.conn.lock().unwrap().execute("INSERT OR REPLACE INTO keys VALUES(?,?)", rusqlite::params![provider, blob]).unwrap();
}

#[tokio::test]
async fn engine_end_to_end_with_retry_for_both_vendors() {
    for (prov, key) in [("anthropic", "retry-e2e-a"), ("gemini", "retry-e2e-g"), ("anthropic", "ok-e2e-a"), ("gemini", "ok-e2e-g")] {
        let (a, _d) = setup();
        put_key(&a, prov, key);
        let id = job(&a, prov);
        a.run_next().await.unwrap();
        let d = a.db({ let id = id.clone(); move |c| resume_server::api::job_detail(c, &id) }).await.unwrap().unwrap();
        assert_eq!(d["job"]["status"], "done", "{prov}/{key}: {}", d["job"]["error"]);
        let reqs = calls_for(key);
        assert_eq!(reqs.len(), if key.starts_with("retry") { 2 } else { 1 }, "{key}");
        assert!(reqs.iter().all(|r| !r.0.contains(key)));
        let t = d["tailored"].to_string();
        assert!(t.contains("Built Python services on Kubernetes for Globex Corp") && t.contains("Acme Robotics"), "payload restored: {t}");
        let out: String = a.conn.lock().unwrap().query_row("SELECT output_json FROM job_steps WHERE name='ai_tailor'", [], |r| r.get(0)).unwrap();
        let out: Value = serde_json::from_str(&out).unwrap();
        assert_eq!((out["tokens_in"].as_i64(), out["tokens_out"].as_i64(), out["cached"].as_bool()), (Some(11), Some(7), Some(false)));
        let (ti, to): (i64, i64) = a.conn.lock().unwrap().query_row("SELECT tokens_in,tokens_out FROM ai_cache", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!((ti, to), (11, 7));
        // the vendor never saw real details
        let body = reqs[0].2.to_string();
        assert!(!body.contains("Acme") && !body.contains("Globex"), "redaction holds");
    }
}

#[tokio::test]
async fn permanent_vendor_error_fails_job_and_missing_key_is_clear() {
    let _g = env_lock().await;
    let (a, _d) = setup();
    put_key(&a, "gemini", "bad-e2e-g");
    let id = job(&a, "gemini");
    a.run_next().await.unwrap();
    let st: (String, String) = a.conn.lock().unwrap().query_row("SELECT status,error FROM jobs WHERE id=?", [&id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!(st.0, "failed");
    assert!(!st.1.contains("bad-e2e-g"));
    assert_eq!(calls_for("bad-e2e-g").len(), 1, "no retry on 401");

    let e = a.provider_for("anthropic").await.err().unwrap();
    assert_eq!(e, ProviderError::Permanent("no API key configured for anthropic (set ANTHROPIC_API_KEY or add one in Settings)".into()));
    let id = job(&a, "anthropic");
    a.run_next().await.unwrap();
    let err: String = a.conn.lock().unwrap().query_row("SELECT error FROM jobs WHERE id=?", [&id], |r| r.get(0)).unwrap();
    assert!(err.contains("no API key configured for anthropic"), "{err}");
}

#[test]
fn vault_roundtrip_perms_and_ciphertext() {
    let (a, dir) = setup();
    let secret = "sk-ant-SUPER-SECRET-123";
    put_key(&a, "anthropic", secret);
    assert_eq!(std::fs::metadata(dir.join("master.key")).unwrap().permissions().mode() & 0o777, 0o600);
    let blob: Vec<u8> = a.conn.lock().unwrap().query_row("SELECT blob FROM keys WHERE provider='anthropic'", [], |r| r.get(0)).unwrap();
    assert!(!blob.windows(secret.len()).any(|w| w == secret.as_bytes()), "ciphertext hides plaintext");
    assert_eq!(keys::decrypt(&a.key_file, "anthropic", &blob).unwrap(), secret);
    assert!(keys::decrypt(&a.key_file, "gemini", &blob).is_err(), "provider is bound as AAD");
    // nonces are random
    assert_ne!(keys::encrypt(&a.key_file, "anthropic", secret).unwrap(), blob);
    // whole DB file (incl. WAL) never contains the plaintext
    for f in std::fs::read_dir(&dir).unwrap().flatten().filter(|f| f.file_name().to_string_lossy().starts_with("app.db")) {
        let b = std::fs::read(f.path()).unwrap();
        assert!(!b.windows(secret.len()).any(|w| w == secret.as_bytes()));
    }
    // group/other-readable master key is refused
    std::fs::set_permissions(dir.join("master.key"), std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(keys::decrypt(&a.key_file, "anthropic", &blob).is_err());
    assert!(keys::encrypt(&a.key_file, "anthropic", secret).is_err());
}

async fn call(app: &Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut r = Request::builder().method(method).uri(uri);
    let b = match body { Some(v) => { r = r.header("content-type", "application/json"); Body::from(v.to_string()) } None => Body::empty() };
    let resp = app.clone().oneshot(r.body(b).unwrap()).await.unwrap();
    let st = resp.status();
    let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    (st, serde_json::from_slice(&bytes).unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into())))
}

#[tokio::test]
async fn http_routes() {
    let _g = env_lock().await;
    let (a, dir) = setup();
    let app = router(a.clone());
    assert_eq!(call(&app, "GET", "/api/health", None).await, (StatusCode::OK, json!("ok")));

    let none = json!({"anthropic": {"configured": false, "source": null}, "gemini": {"configured": false, "source": null}});
    assert_eq!(call(&app, "GET", "/api/keys", None).await, (StatusCode::OK, none.clone()));
    assert_eq!(call(&app, "PUT", "/api/keys/gemini", Some(json!({"key": ""}))).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(call(&app, "PUT", "/api/keys/bogus", Some(json!({"key": "x"}))).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(call(&app, "PUT", "/api/keys/gemini", Some(json!({"key": "ok-route-g"}))).await.0, StatusCode::NO_CONTENT);
    let (_, k) = call(&app, "GET", "/api/keys", None).await;
    assert_eq!(k, json!({"anthropic": {"configured": false, "source": null}, "gemini": {"configured": true, "source": "stored"}}));
    assert!(!k.to_string().contains("ok-route-g"));
    assert_eq!(std::fs::metadata(dir.join("master.key")).unwrap().permissions().mode() & 0o777, 0o600);

    // job through the HTTP API, worked by the engine against the fake
    let (st, j) = call(&app, "POST", "/api/jobs", Some(json!({"jd_text": JD, "provider": "gemini"}))).await;
    assert_eq!(st, StatusCode::OK);
    let id = j["id"].as_str().unwrap().to_string();
    assert_eq!(call(&app, "GET", &format!("/api/jobs/{id}"), None).await.1["job"]["status"], "queued");
    a.run_next().await.unwrap();
    let (st, d) = call(&app, "GET", &format!("/api/jobs/{id}"), None).await;
    assert_eq!((st, d["job"]["status"].as_str()), (StatusCode::OK, Some("done")));
    assert_eq!(call(&app, "GET", "/api/jobs/nope", None).await.0, StatusCode::NOT_FOUND);

    let (st, s) = call(&app, "GET", "/api/stats", None).await;
    assert_eq!((st, s["tokens_in"].as_i64(), s["tokens_out"].as_i64(), s["by_status"]["done"].as_i64()), (StatusCode::OK, Some(11), Some(7), Some(1)));

    // files: real file served, traversal rejected
    assert_eq!(call(&app, "GET", &format!("/api/jobs/{id}/files/resume.pdf"), None).await.0, StatusCode::OK);
    for u in [format!("/api/jobs/..%2Fx/files/resume.pdf"), format!("/api/jobs/{id}/files/..%2Fx"), format!("/api/jobs/{id}/files/..%2F..%2Fapp.db"), "/api/jobs/../x/files/resume.pdf".into()] {
        assert!(!call(&app, "GET", &u, None).await.0.is_success(), "{u}");
    }

    assert_eq!(call(&app, "DELETE", "/api/keys/gemini", None).await.0, StatusCode::NO_CONTENT);
    assert_eq!(call(&app, "GET", "/api/keys", None).await.1, none);
    assert!(a.provider_for("gemini").await.is_err());
}

#[tokio::test]
async fn env_keys_are_used_and_stored_overrides() {
    let _g = env_lock().await;
    let (_, url) = fake();
    let (a, _d) = setup();
    let app = router(a.clone());
    std::env::set_var("ANTHROPIC_BASE_URL", "http://127.0.0.1:1"); // must never be consulted
    std::env::set_var("ANTHROPIC_API_KEY", "ok-env-anth");
    std::env::set_var("GOOGLE_API_KEY", "ok-env-goog");
    let (_, k) = call(&app, "GET", "/api/keys", None).await;
    assert_eq!(k, json!({"anthropic": {"configured": true, "source": "env"}, "gemini": {"configured": true, "source": "env"}}));
    assert!(!k.to_string().contains("ok-env"));
    // env key is used (against the fake, via RESUME_ANTHROPIC_URL only)
    let id = job(&a, "anthropic");
    a.run_next().await.unwrap();
    let st: String = a.conn.lock().unwrap().query_row("SELECT status FROM jobs WHERE id=?", [&id], |r| r.get(0)).unwrap();
    assert_eq!(st, "done");
    assert_eq!(calls_for("ok-env-anth").len(), 1);
    // GEMINI_API_KEY wins over GOOGLE_API_KEY
    std::env::set_var("GEMINI_API_KEY", "ok-env-gem");
    let id = job(&a, "gemini");
    a.run_next().await.unwrap();
    assert_eq!((calls_for("ok-env-gem").len(), calls_for("ok-env-goog").len()), (1, 0), "{id}");
    // stored overrides env
    put_key(&a, "anthropic", "ok-stored-anth");
    a.conn.lock().unwrap().execute("DELETE FROM ai_cache", []).unwrap(); // else the reply is reused
    assert_eq!(call(&app, "GET", "/api/keys", None).await.1["anthropic"], json!({"configured": true, "source": "stored"}));
    let id = job(&a, "anthropic");
    a.run_next().await.unwrap();
    assert_eq!((calls_for("ok-stored-anth").len(), calls_for("ok-env-anth").len()), (1, 1), "{id}");
    // DELETE removes only the stored key; env remains
    assert_eq!(call(&app, "DELETE", "/api/keys/anthropic", None).await.0, StatusCode::NO_CONTENT);
    assert_eq!(call(&app, "GET", "/api/keys", None).await.1["anthropic"], json!({"configured": true, "source": "env"}));
    std::env::remove_var("ANTHROPIC_API_KEY");
    std::env::remove_var("ANTHROPIC_BASE_URL");
    assert_eq!(call(&app, "GET", "/api/keys", None).await.1["anthropic"], json!({"configured": false, "source": null}));
    let _ = url;
}

/// Vendor tests count requests, so the (default-on) AI plan review is switched off here.
fn job(a: &App, provider: &str) -> String {
    let mut c = a.conn.lock().unwrap();
    let id = create_job(&mut c, JD, provider, &[], None).unwrap();
    c.execute("UPDATE jobs SET ai_review=0 WHERE id=?", [&id]).unwrap();
    id
}
