//! Company/role metadata, library, display names, DB migration and Google Drive (against a local axum fake of Google).
use axum::{body::{to_bytes, Body}, extract::{Path as AxPath, Query, State}, http::{HeaderMap, Request, StatusCode}, response::IntoResponse, routing::{get, patch, post}, Form, Json, Router};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64, Engine};
use resume_server::{api::{job_detail, router}, create_job_with, display_name, providers::mock::MockProvider, retry_job, sanitize_name, App, Fault};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, path::PathBuf, sync::{Arc, Mutex}};
use tower::ServiceExt;

const JD: &str = "Senior Backend Engineer at Acme Robotics\nRequirements:\n- 5+ years of Python and Kubernetes\n- Strong C# and .NET experience\n";

fn resume() -> String {
    json!({"profile": {"name": "Jane Doe", "email": "jane.doe@acme-robotics.test", "phone": "+1 555 010 4477", "role": "Engineer", "location": "Springfield"},
        "summary": ["Engineer who ships."],
        "experience": [{"id": "x1", "role": "Backend Engineer", "organization": "Acme Robotics Pvt Ltd", "client": "Globex Corp", "dateLabel": "2020-2024",
            "bullets": ["Built Python services on Kubernetes for Globex Corp, cutting latency 40%", "Maintained C# and .NET billing code"]}],
        "skills": [{"label": "Cloud", "skills": ["Python", "Terraform", "Kubernetes"]}]}).to_string()
}

fn setup() -> (Arc<App>, PathBuf) {
    let dir = std::env::temp_dir().join(format!("rs-feat-{}", uuid::Uuid::new_v4()));
    let app = App::open(&dir.join("app.db"), &dir.join("out")).unwrap();
    app.conn.lock().unwrap().execute("INSERT INTO resume VALUES(1,?)", [resume()]).unwrap();
    *app.provider_override.lock().unwrap() = Some(Arc::new(MockProvider::default()));
    let app = Arc::try_unwrap(app).ok().map(|mut a| { a.backoff_ms = 1; Arc::new(a) }).unwrap();
    (app, dir)
}

async fn call(app: &Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let (st, _, b) = raw(app, method, uri, body).await;
    (st, serde_json::from_slice(&b).unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&b).into())))
}
async fn raw(app: &Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, HeaderMap, Vec<u8>) {
    let mut r = Request::builder().method(method).uri(uri);
    let b = match body { Some(v) => { r = r.header("content-type", "application/json"); Body::from(v.to_string()) } None => Body::empty() };
    let resp = app.clone().oneshot(r.body(b).unwrap()).await.unwrap();
    let (st, h) = (resp.status(), resp.headers().clone());
    (st, h, to_bytes(resp.into_body(), 16 << 20).await.unwrap().to_vec())
}

#[test]
fn display_names_are_sanitised() {
    assert_eq!(display_name("Acme Robotics Pvt Ltd", "Senior Engineer", "pdf"), "Acme Robotics - Senior Engineer - Resume.pdf");
    assert_eq!(display_name("Foo/Bar: \"Baz\" <Inc>", "Dev|Ops?\u{7}", "docx"), "FooBar Baz Inc - DevOps - Resume.docx");
    assert_eq!(display_name("A  B,  Inc.", "R", "pdf"), "A B - R - Resume.pdf");
    let long = display_name(&"x".repeat(300), "Role", "docx");
    assert_eq!(long.chars().count(), 120);
    assert!(long.ends_with(".docx"));
    assert_eq!(sanitize_name("a\\b*c  d", 50), "abc d");
}

#[tokio::test]
async fn company_role_library_and_content_disposition() {
    let (a, _d) = setup();
    let app = router(a.clone());
    // parsed from the JD; explicit values win
    let (_, j1) = call(&app, "POST", "/api/jobs", Some(json!({"jd_text": JD, "provider": "mock"}))).await;
    let (_, j2) = call(&app, "POST", "/api/jobs", Some(json!({"jd_text": "Dev\nstuff", "provider": "mock", "company": "Zed Ünï Co", "role": "Staff Dev"}))).await;
    let (_, j3) = call(&app, "POST", "/api/jobs", Some(json!({"jd_text": "Dev\nstuff", "provider": "mock"}))).await;
    let (id1, id2, id3) = (j1["id"].as_str().unwrap(), j2["id"].as_str().unwrap(), j3["id"].as_str().unwrap());
    assert_eq!(call(&app, "GET", "/api/library", None).await.1, json!([]), "only done jobs");
    for _ in 0..3 { a.run_next().await.unwrap(); }
    let (_, d1) = call(&app, "GET", &format!("/api/jobs/{id1}"), None).await;
    assert_eq!((d1["company"].as_str(), d1["role"].as_str()), (Some("Acme Robotics"), Some("Senior Backend Engineer")));
    assert_eq!(d1["display_names"]["resume.pdf"], "Acme Robotics - Senior Backend Engineer - Resume.pdf");
    assert_eq!(d1["drive"], Value::Null);
    let (_, d3) = call(&app, "GET", &format!("/api/jobs/{id3}"), None).await;
    assert_eq!(d3["company"], "Unknown");

    let (st, h, body) = raw(&app, "GET", &format!("/api/jobs/{id2}/files/resume.pdf"), None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(body.starts_with(b"%PDF"));
    let cd = h["content-disposition"].to_str().unwrap();
    assert!(cd.contains("filename*=UTF-8''Zed%20%C3%9Cn%C3%AF%20-%20Staff%20Dev%20-%20Resume.pdf"), "{cd}");
    assert!(cd.contains("filename=\"Zed"));
    assert_eq!(call(&app, "GET", &format!("/api/jobs/{id2}/files/other.pdf"), None).await.0, StatusCode::NOT_FOUND);

    let (_, lib) = call(&app, "GET", "/api/library", None).await;
    let lib = lib.as_array().unwrap();
    assert_eq!(lib.len(), 3);
    assert_eq!(lib[0]["id"], id3, "newest first");
    let e = lib.iter().find(|e| e["id"] == id2).unwrap();
    assert_eq!((e["company"].as_str(), e["role"].as_str(), e["drive"].clone()), (Some("Zed Ünï Co"), Some("Staff Dev"), Value::Null));
    assert_eq!(e["files"].as_array().unwrap().len(), 2);
    assert_eq!(e["files"][0]["name"], "resume.pdf");
    assert!(e["coverage"].is_number() && e["created_at"].is_number());
}

#[test]
fn old_database_is_migrated() {
    let dir = std::env::temp_dir().join(format!("rs-mig-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    {
        let c = rusqlite::Connection::open(dir.join("app.db")).unwrap();
        c.execute_batch("CREATE TABLE jobs(id TEXT PRIMARY KEY, status TEXT NOT NULL, provider TEXT NOT NULL, jd_text TEXT NOT NULL,
            extra_redact_json TEXT NOT NULL, max_tokens INTEGER, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, lease_owner TEXT, lease_until INTEGER, error TEXT);
            INSERT INTO jobs VALUES('old1','done','mock','Dev','[]',NULL,1,1,NULL,NULL,NULL);").unwrap();
    }
    for _ in 0..2 {
        let a = App::open(&dir.join("app.db"), &dir.join("out")).unwrap(); // idempotent
        let d = job_detail(&a.conn.lock().unwrap(), "old1").unwrap().unwrap();
        assert_eq!((d["company"].as_str(), d["job"]["upload_drive"].as_bool()), (Some("Unknown"), Some(false)));
    }
}

// ---------------------------------------------------------------- fake Google

#[derive(Default)]
struct G {
    codes: HashMap<String, String>, // code -> pkce challenge
    valid: String,
    n_tokens: u32,
    refreshes: u32,
    revoked: u32,
    fail500: u32,
    uploads: u32,
    files: Vec<Value>, // {id,name,parents,props,len,head}
    folders: Vec<Value>,
}
type F = Arc<Mutex<G>>;

fn authed(g: &G, h: &HeaderMap) -> bool {
    h.get("authorization").and_then(|v| v.to_str().ok()) == Some(format!("Bearer {}", g.valid).as_str())
}
fn between<'a>(s: &'a str, a: &str) -> &'a str {
    let i = s.find(a).unwrap() + a.len();
    &s[i..i + s[i..].find('\'').unwrap()]
}

async fn token(State(f): State<F>, Form(p): Form<HashMap<String, String>>) -> axum::response::Response {
    let mut g = f.lock().unwrap();
    match p["grant_type"].as_str() {
        "authorization_code" => {
            let ok = g.codes.get(&p["code"]).is_some_and(|ch| *ch == B64.encode(Sha256::digest(p["code_verifier"].as_bytes())));
            if !ok || p["client_id"] != "cid" || p["client_secret"] != "csec" || !p["redirect_uri"].ends_with("/api/drive/callback") { return (StatusCode::BAD_REQUEST, Json(json!({"error": "invalid_grant"}))).into_response(); }
        }
        "refresh_token" => {
            if p["refresh_token"] != "rt-1" { return (StatusCode::BAD_REQUEST, Json(json!({"error": "invalid_grant"}))).into_response(); }
            g.refreshes += 1;
        }
        _ => return StatusCode::BAD_REQUEST.into_response(),
    }
    g.n_tokens += 1;
    g.valid = format!("at-{}", g.n_tokens);
    let mut r = json!({"access_token": g.valid, "expires_in": 3600});
    if p["grant_type"] == "authorization_code" { r["refresh_token"] = json!("rt-1"); }
    Json(r).into_response()
}

async fn list(State(f): State<F>, h: HeaderMap, Query(q): Query<HashMap<String, String>>) -> axum::response::Response {
    let g = f.lock().unwrap();
    if !authed(&g, &h) { return StatusCode::UNAUTHORIZED.into_response(); }
    let qs = &q["q"];
    let hits: Vec<Value> = if qs.contains("google-apps.folder") {
        g.folders.iter().filter(|x| x["name"] == between(qs, "name='")).map(|x| json!({"id": x["id"]})).collect()
    } else {
        let (parent, job, kind) = (between(qs, "'"), between(qs, "key='job_id' and value='"), between(qs, "key='kind' and value='"));
        g.files.iter().filter(|x| x["parents"][0] == parent && x["props"]["job_id"] == job && x["props"]["kind"] == kind).map(|x| json!({"id": x["id"]})).collect()
    };
    Json(json!({"files": hits})).into_response()
}

async fn mkfolder(State(f): State<F>, h: HeaderMap, Json(b): Json<Value>) -> axum::response::Response {
    let mut g = f.lock().unwrap();
    if !authed(&g, &h) { return StatusCode::UNAUTHORIZED.into_response(); }
    let id = format!("folder-{}", g.folders.len() + 1);
    g.folders.push(json!({"id": id, "name": b["name"]}));
    Json(json!({"id": id})).into_response()
}

fn parse_multipart(ct: &str, body: &[u8]) -> (Value, Vec<u8>) {
    let b = format!("--{}", ct.split("boundary=").nth(1).unwrap());
    let s = body.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let json_end = body.windows(b.len()).enumerate().filter(|(_, w)| *w == b.as_bytes()).nth(1).unwrap().0;
    let meta: Value = serde_json::from_slice(&body[s + 4..json_end - 2]).unwrap();
    let rest = &body[json_end..];
    let fs = rest.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
    let fe = rest.len() - format!("\r\n{b}--").len();
    (meta, rest[fs..fe].to_vec())
}

async fn upload(State(f): State<F>, id: Option<AxPath<String>>, h: HeaderMap, body: axum::body::Bytes) -> axum::response::Response {
    let mut g = f.lock().unwrap();
    if !authed(&g, &h) { return StatusCode::UNAUTHORIZED.into_response(); }
    if g.fail500 > 0 { g.fail500 -= 1; return StatusCode::INTERNAL_SERVER_ERROR.into_response(); }
    let (meta, content) = parse_multipart(h["content-type"].to_str().unwrap(), &body);
    g.uploads += 1;
    let rec = |id: &str, old: Option<&Value>| json!({"id": id, "name": meta["name"], "parents": meta.get("parents").cloned().or(old.map(|o| o["parents"].clone())), "props": meta["appProperties"], "len": content.len(), "head": String::from_utf8_lossy(&content[..4])});
    let id = match id {
        Some(AxPath(id)) => {
            let i = g.files.iter().position(|x| x["id"] == id.as_str()).expect("update of unknown file");
            let new = rec(&id, Some(&g.files[i]));
            g.files[i] = new;
            id
        }
        None => { let id = format!("file-{}", g.files.len() + 1); let r = rec(&id, None); g.files.push(r); id }
    };
    Json(json!({"id": id, "webViewLink": format!("https://drive.example/{id}")})).into_response()
}

async fn about(State(f): State<F>, h: HeaderMap) -> axum::response::Response {
    if !authed(&f.lock().unwrap(), &h) { return StatusCode::UNAUTHORIZED.into_response(); }
    Json(json!({"user": {"emailAddress": "jane@example.test"}})).into_response()
}

/// Env vars are process-global, so Drive tests run one at a time, each with a fresh fake.
async fn start_fake() -> (F, tokio::sync::MutexGuard<'static, ()>) {
    static L: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let guard = L.lock().await;
    let f: F = Default::default();
    let app = Router::new()
        .route("/token", post(token))
        .route("/revoke", post(|State(f): State<F>| async move { f.lock().unwrap().revoked += 1; }))
        .route("/drive/v3/about", get(about))
        .route("/drive/v3/files", get(list).post(mkfolder))
        .route("/upload/drive/v3/files", post(|s: State<F>, h: HeaderMap, b: axum::body::Bytes| upload(s, None, h, b)))
        .route("/upload/drive/v3/files/:id", patch(|s: State<F>, i: AxPath<String>, h: HeaderMap, b: axum::body::Bytes| upload(s, Some(i), h, b)))
        .with_state(f.clone());
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.set_nonblocking(true).unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    std::thread::spawn(move || tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap().block_on(async move {
        axum::serve(tokio::net::TcpListener::from_std(l).unwrap(), app).await.unwrap();
    }));
    std::env::set_var("RESUME_GOOGLE_TOKEN_URL", format!("{url}/token"));
    for k in ["RESUME_GOOGLE_API_URL", "RESUME_GOOGLE_UPLOAD_URL"] { std::env::set_var(k, &url); }
    std::env::set_var("RESUME_GOOGLE_AUTH_URL", format!("{url}/auth"));
    std::env::set_var("NO_PROXY", "127.0.0.1");
    std::env::set_var("PORT", "8787");
    for k in ["GOOGLE_OAUTH_CLIENT_ID", "GOOGLE_OAUTH_CLIENT_SECRET", "RESUME_GOOGLE_REVOKE_URL"] { std::env::remove_var(k); }
    (f, guard)
}

/// Full browser-less OAuth dance; returns the auth_url that was handed to the UI.
async fn connect(app: &Router, f: &F) -> reqwest::Url {
    assert_eq!(call(app, "PUT", "/api/drive/oauth", Some(json!({"client_id": "cid", "client_secret": "csec"}))).await.0, StatusCode::NO_CONTENT);
    let (st, c) = call(app, "POST", "/api/drive/connect", None).await;
    assert_eq!(st, StatusCode::OK);
    let url = reqwest::Url::parse(c["auth_url"].as_str().unwrap()).unwrap();
    let q: HashMap<_, _> = url.query_pairs().into_owned().collect();
    f.lock().unwrap().codes.insert("code-1".into(), q["code_challenge"].clone());
    let (st, page) = call(app, "GET", &format!("/api/drive/callback?code=code-1&state={}", q["state"]), None).await;
    assert_eq!(st, StatusCode::OK, "{page}");
    assert!(page.as_str().unwrap().contains("Connected, you can close this tab"));
    url
}

fn drive_job(a: &App) -> String {
    create_job_with(&mut a.conn.lock().unwrap(), JD, "mock", &[], None, None, None, true).unwrap()
}
fn state(a: &App, id: &str, step: &str) -> (String, String) {
    a.conn.lock().unwrap().query_row("SELECT (SELECT status FROM jobs WHERE id=?1),(SELECT status FROM job_steps WHERE job_id=?1 AND name=?2)", [id, step], |r| Ok((r.get(0)?, r.get(1)?))).unwrap()
}

#[tokio::test]
async fn drive_oauth_pkce_upload_and_idempotency() {
    let (f, _g) = start_fake().await;
    let (a, dir) = setup();
    let app = router(a.clone());
    let (_, s) = call(&app, "GET", "/api/drive", None).await;
    assert_eq!(s, json!({"configured": false, "connected": false, "folder_name": "Tailored Resumes", "account_email": null, "redirect_uri": "http://127.0.0.1:8787/api/drive/callback"}));
    assert_eq!(call(&app, "POST", "/api/drive/connect", None).await.0, StatusCode::BAD_REQUEST, "needs a client id");

    let url = connect(&app, &f).await;
    let q: HashMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!((q["scope"].as_str(), q["code_challenge_method"].as_str(), q["client_id"].as_str(), q["response_type"].as_str()), ("https://www.googleapis.com/auth/drive.file", "S256", "cid", "code"));
    assert_eq!(q["redirect_uri"], "http://127.0.0.1:8787/api/drive/callback");
    assert_eq!(q["code_challenge"].len(), 43, "base64url sha256, unpadded");
    // state is single use
    let again = call(&app, "GET", &format!("/api/drive/callback?code=code-1&state={}", q["state"]), None).await;
    assert_eq!(again.0, StatusCode::BAD_REQUEST);
    assert_eq!(call(&app, "GET", "/api/drive/callback?code=x&state=forged", None).await.0, StatusCode::BAD_REQUEST);
    let (_, s) = call(&app, "GET", "/api/drive", None).await;
    assert_eq!((s["configured"].as_bool(), s["connected"].as_bool(), s["account_email"].as_str()), (Some(true), Some(true), Some("jane@example.test")));
    assert!(!s.to_string().contains("csec") && !s.to_string().contains("rt-1"));
    // secrets are encrypted at rest
    let blobs: Vec<Vec<u8>> = a.conn.lock().unwrap().prepare("SELECT blob FROM keys").unwrap().query_map([], |r| r.get(0)).unwrap().map(|r| r.unwrap()).collect();
    assert_eq!(blobs.len(), 2);
    assert!(blobs.iter().all(|b| !b.windows(4).any(|w| w == b"csec" || w == b"rt-1")));
    assert_eq!(call(&app, "PUT", "/api/drive", Some(json!({"folder_name": "My: Résumés"}))).await.0, StatusCode::NO_CONTENT);

    let id = drive_job(&a);
    a.run_next().await.unwrap();
    assert_eq!(state(&a, &id, "upload_drive"), ("done".into(), "done".into()));
    {
        let g = f.lock().unwrap();
        assert_eq!(g.folders.len(), 1);
        assert_eq!(g.folders[0]["name"], "My Résumés");
        let mut names: Vec<_> = g.files.iter().map(|x| x["name"].as_str().unwrap().to_string()).collect();
        names.sort();
        assert_eq!(names, ["Acme Robotics - Senior Backend Engineer - Resume.docx", "Acme Robotics - Senior Backend Engineer - Resume.pdf"]);
        assert!(g.files.iter().all(|x| x["props"]["job_id"] == id.as_str() && x["parents"][0] == "folder-1" && x["len"].as_u64().unwrap() > 1000));
        assert!(g.files.iter().any(|x| x["head"] == "%PDF") && g.files.iter().any(|x| x["head"] == "PK\u{3}\u{4}"));
    }
    let (_, d) = call(&app, "GET", &format!("/api/jobs/{id}"), None).await;
    assert_eq!(d["drive"].as_array().unwrap().len(), 2);
    assert!(d["drive"][0]["url"].as_str().unwrap().starts_with("https://drive.example/file-"));
    let evs = resume_server::events_after(&a.conn.lock().unwrap(), &id, 0).unwrap();
    let up = evs.iter().find(|e| e["message"].as_str().unwrap().starts_with("Uploading 2 files")).unwrap();
    assert_eq!((up["message"].as_str(), up["local_only"].as_bool()), (Some("Uploading 2 files to Google Drive folder \"My Résumés\" (your account)"), Some(false)));
    let (_, lib) = call(&app, "GET", "/api/library", None).await;
    assert_eq!(lib[0]["drive"].as_array().unwrap().len(), 2);

    // retry from the upload step updates in place: no duplicates, folder reused
    retry_job(&mut a.conn.lock().unwrap(), &id, "upload_drive").unwrap();
    a.run_next().await.unwrap();
    assert_eq!(state(&a, &id, "upload_drive").0, "done");
    {
        let g = f.lock().unwrap();
        assert_eq!((g.files.len(), g.folders.len(), g.uploads), (2, 1, 4));
    }
    // a second job lands in the same folder
    let id2 = drive_job(&a);
    a.run_next().await.unwrap();
    let (nf, nd) = { let g = f.lock().unwrap(); (g.files.len(), g.folders.len()) };
    assert_eq!((state(&a, &id2, "upload_drive").0, nf, nd), ("done".to_string(), 4, 1));

    // jobs without upload_drive never touch Google
    let before = f.lock().unwrap().uploads;
    let id3 = create_job_with(&mut a.conn.lock().unwrap(), JD, "mock", &[], None, None, None, false).unwrap();
    a.run_next().await.unwrap();
    assert_eq!((state(&a, &id3, "upload_drive").0, f.lock().unwrap().uploads), ("done".to_string(), before));
    assert!(dir.join("out").join(&id3).join("resume.pdf").exists());

    // disconnect: tokens gone, revoke attempted, env/other settings untouched
    std::env::set_var("RESUME_GOOGLE_REVOKE_URL", format!("{}/revoke", std::env::var("RESUME_GOOGLE_API_URL").unwrap()));
    assert_eq!(call(&app, "POST", "/api/drive/disconnect", None).await.0, StatusCode::NO_CONTENT);
    let (_, s) = call(&app, "GET", "/api/drive", None).await;
    assert_eq!((s["connected"].as_bool(), s["configured"].as_bool(), s["account_email"].clone()), (Some(false), Some(true), Value::Null));
    assert_eq!(f.lock().unwrap().revoked, 1);
}

#[tokio::test]
async fn drive_bad_pkce_verifier_is_rejected() {
    let (f, _g) = start_fake().await;
    let (a, _d) = setup();
    let app = router(a.clone());
    call(&app, "PUT", "/api/drive/oauth", Some(json!({"client_id": "cid", "client_secret": "csec"}))).await;
    let (_, c) = call(&app, "POST", "/api/drive/connect", None).await;
    let q: HashMap<_, _> = reqwest::Url::parse(c["auth_url"].as_str().unwrap()).unwrap().query_pairs().into_owned().collect();
    // the "server" remembers a different challenge: exchange must fail and nothing is stored
    f.lock().unwrap().codes.insert("code-1".into(), B64.encode(Sha256::digest(b"other")));
    let (st, _) = call(&app, "GET", &format!("/api/drive/callback?code=code-1&state={}", q["state"]), None).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(call(&app, "GET", "/api/drive", None).await.1["connected"], false);
    // RFC 7636 appendix B vector
    assert_eq!(resume_server::drive::pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
}

#[tokio::test]
async fn drive_refresh_on_401_retry_on_500_and_crash_resume() {
    let (f, _g) = start_fake().await;
    let (a, _d) = setup();
    let app = router(a.clone());
    connect(&app, &f).await;
    // expire the access token server-side: the cached one 401s, gets refreshed once, and 500s are retried
    { let mut g = f.lock().unwrap(); g.valid = "expired".into(); g.fail500 = 2; }
    let id = drive_job(&a);
    a.run_next().await.unwrap();
    assert_eq!(state(&a, &id, "upload_drive").0, "done");
    { let g = f.lock().unwrap(); assert_eq!((g.refreshes, g.files.len()), (1, 2)); }
    let evs = resume_server::events_after(&a.conn.lock().unwrap(), &id, 0).unwrap();
    assert!(evs.iter().any(|e| e["message"].as_str().unwrap().contains("Transient error")));

    // crash after the files were uploaded but before the step committed (index 8 = upload_drive)
    let id2 = drive_job(&a);
    *a.fault.lock().unwrap() = Some(Fault::Mid(8));
    assert!(a.run_next().await.is_err());
    assert_eq!(f.lock().unwrap().files.len(), 4);
    a.conn.lock().unwrap().execute("UPDATE jobs SET lease_until=0 WHERE status='running'", []).unwrap();
    resume_server::sweep(&mut a.conn.lock().unwrap(), false).unwrap();
    a.run_next().await.unwrap();
    assert_eq!(state(&a, &id2, "upload_drive").0, "done");
    assert_eq!(f.lock().unwrap().files.len(), 4, "resume updated in place, no duplicates");
    let (_, d) = call(&app, "GET", &format!("/api/jobs/{id2}"), None).await;
    assert_eq!(d["drive"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn drive_not_connected_fails_step_but_files_stay_and_retry_works() {
    let (f, _g) = start_fake().await;
    let (a, _d) = setup();
    let app = router(a.clone());
    let id = drive_job(&a);
    a.run_next().await.unwrap();
    assert_eq!(state(&a, &id, "upload_drive"), ("failed".into(), "failed".into()));
    let (_, d) = call(&app, "GET", &format!("/api/jobs/{id}"), None).await;
    assert!(d["job"]["error"]["message"].as_str().unwrap().contains("Google Drive is not connected"));
    assert_eq!(d["files"].as_array().unwrap().len(), 2);
    assert_eq!(call(&app, "GET", &format!("/api/jobs/{id}/files/resume.docx"), None).await.0, StatusCode::OK);
    connect(&app, &f).await;
    assert_eq!(call(&app, "POST", &format!("/api/jobs/{id}/retry?from=upload_drive"), None).await.0, StatusCode::NO_CONTENT);
    a.run_next().await.unwrap();
    assert_eq!(state(&a, &id, "upload_drive").0, "done");
    assert_eq!(f.lock().unwrap().files.len(), 2);
}
