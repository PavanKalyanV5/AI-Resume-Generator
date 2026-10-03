use resume_server::{api::router, App};
use rustls::{client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier}, crypto::{ring::default_provider, CryptoProvider}, pki_types::{CertificateDer, ServerName, UnixTime}, DigitallySignedStruct, SignatureScheme};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{os::unix::fs::PermissionsExt as _, sync::Arc};

/// Accepts exactly one certificate: the one whose SHA-256 matches the fingerprint the phone got in the QR code.
#[derive(Debug)]
struct Pin(String, Arc<CryptoProvider>);
impl ServerCertVerifier for Pin {
    fn verify_server_cert(&self, e: &CertificateDer<'_>, _: &[CertificateDer<'_>], _: &ServerName<'_>, _: &[u8], _: UnixTime) -> Result<ServerCertVerified, rustls::Error> {
        if hex::encode(Sha256::digest(e.as_ref())) == self.0 { Ok(ServerCertVerified::assertion()) } else { Err(rustls::Error::General("fingerprint mismatch".into())) }
    }
    fn verify_tls12_signature(&self, m: &[u8], c: &CertificateDer<'_>, d: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(m, c, d, &self.1.signature_verification_algorithms)
    }
    fn verify_tls13_signature(&self, m: &[u8], c: &CertificateDer<'_>, d: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(m, c, d, &self.1.signature_verification_algorithms)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.1.signature_verification_algorithms.supported_schemes()
    }
}

fn pinned(fp: &str) -> reqwest::Client {
    let p = Arc::new(default_provider());
    let cfg = rustls::ClientConfig::builder_with_provider(p.clone()).with_safe_default_protocol_versions().unwrap().dangerous().with_custom_certificate_verifier(Arc::new(Pin(fp.into(), p))).with_no_client_auth();
    reqwest::Client::builder().use_preconfigured_tls(cfg).build().unwrap()
}

struct Env {
    app: Arc<App>,
    local: String,
    remote: String,
    fp: String,
    c: reqwest::Client,
    http: reqwest::Client,
}

async fn local(app: &Arc<App>) -> String {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let a = l.local_addr().unwrap();
    let r = router(app.clone());
    tokio::spawn(async move { axum::serve(l, r).await.unwrap() });
    format!("http://127.0.0.1:{}", a.port())
}

async fn enable(app: Arc<App>) -> Env {
    std::env::set_var("REMOTE_PORT", "0");
    let http = reqwest::Client::new();
    let local = local(&app).await;
    let s: Value = http.post(format!("{local}/api/remote")).json(&json!({"enabled": true})).send().await.unwrap().json().await.unwrap();
    assert_eq!(s["enabled"], true);
    let fp = s["fingerprint"].as_str().unwrap().to_string();
    let c = pinned(&fp);
    Env { remote: format!("https://127.0.0.1:{}", s["port"]), local, fp, c, http, app }
}

fn fresh() -> (Arc<App>, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("rs-remote-{}", uuid::Uuid::new_v4()));
    (App::open(&dir.join("app.db"), &dir.join("out")).unwrap(), dir)
}

impl Env {
    async fn code(&self) -> String {
        let r: Value = self.http.post(format!("{}/api/remote/pair-code", self.local)).send().await.unwrap().json().await.unwrap();
        r["code"].as_str().unwrap().to_string()
    }
    async fn pair_with(&self, code: &str) -> reqwest::Response {
        self.c.post(format!("{}/pair", self.remote)).json(&json!({"code": code, "device_name": "Test phone"})).send().await.unwrap()
    }
    async fn pair(&self) -> (String, String) {
        let r: Value = self.pair_with(&self.code().await).await.json().await.unwrap();
        (r["device_id"].as_str().unwrap().into(), r["token"].as_str().unwrap().into())
    }
    async fn get(&self, path: &str, tok: &str) -> reqwest::StatusCode {
        self.c.get(format!("{}{path}", self.remote)).bearer_auth(tok).send().await.unwrap().status()
    }
}

#[tokio::test]
async fn pair_then_authorised_call_and_code_rules() {
    let e = enable(fresh().0).await;
    let qr: Value = e.http.post(format!("{}/api/remote/pair-code", e.local)).send().await.unwrap().json().await.unwrap();
    let code = qr["code"].as_str().unwrap();
    assert!(code.len() == 8 && code.chars().all(|c| "ABCDEFGHJKLMNPQRSTUVWXYZ23456789".contains(c)));
    assert_eq!((qr["qr_payload"]["v"].as_i64(), qr["qr_payload"]["fp"].as_str()), (Some(1), Some(e.fp.as_str())));
    assert!(qr["expires_at"].as_i64().unwrap() > resume_server::now());
    assert_eq!(e.pair_with("WRONGCOD").await.status(), 403);
    let r = e.pair_with(code).await;
    assert_eq!(r.status(), 200);
    let tok = r.json::<Value>().await.unwrap()["token"].as_str().unwrap().to_string();
    assert_eq!(e.pair_with(code).await.status(), 403, "single use");
    assert_eq!(e.get("/api/health", &tok).await, 200);
    assert_eq!(e.get("/api/library", &tok).await, 200);
    let h = e.c.get(format!("{}/api/health", e.remote)).bearer_auth(&tok).send().await.unwrap();
    assert_eq!(h.headers()["cache-control"], "no-store");
    assert_eq!(h.headers()["x-content-type-options"], "nosniff");
    assert_eq!(e.c.get(format!("{}/api/health", e.remote)).send().await.unwrap().status(), 401);
    assert_eq!(e.get("/api/health", "bogus").await, 401);
    // expired code
    let code = e.code().await;
    e.app.remote.code.lock().unwrap().as_mut().unwrap().1 = resume_server::now() - 1;
    assert_eq!(e.pair_with(&code).await.status(), 403);
    // a plain (unpinned) client cannot talk to the listener at all
    assert!(reqwest::Client::new().get(format!("{}/api/health", e.remote)).send().await.is_err());
}

#[tokio::test]
async fn pairing_is_rate_limited_with_lockout() {
    let e = enable(fresh().0).await;
    let good = e.code().await;
    for _ in 0..5 {
        assert_eq!(e.pair_with("WRONGCOD").await.status(), 403);
    }
    assert_eq!(e.pair_with("WRONGCOD").await.status(), 429);
    assert_eq!(e.pair_with(&good).await.status(), 429, "locked out even with the right code");
}

#[tokio::test]
async fn revoke_scope_and_admin_isolation() {
    let e = enable(fresh().0).await;
    let (id, tok) = e.pair().await;
    for (m, p) in [("POST", "/api/pii/reveal"), ("GET", "/api/keys"), ("PUT", "/api/resume"), ("POST", "/api/drive/connect"), ("GET", "/api/heuristics"), ("POST", "/api/ml/retrain"), ("POST", "/api/regression/run"), ("POST", "/api/import"), ("GET", "/api/settings"), ("GET", "/api/profile/facts"), ("GET", "/api/stats")] {
        let r = e.c.request(m.parse().unwrap(), format!("{}{p}", e.remote)).bearer_auth(&tok).send().await.unwrap();
        assert_eq!(r.status(), 403, "{m} {p}");
    }
    for (m, p) in [("GET", "/api/remote"), ("POST", "/api/remote"), ("GET", "/api/remote/devices"), ("POST", "/api/remote/pair-code"), ("GET", "/api/remote/audit")] {
        for t in [Some(&tok), None] {
            let mut r = e.c.request(m.parse().unwrap(), format!("{}{p}", e.remote));
            if let Some(t) = t {
                r = r.bearer_auth(t);
            }
            assert_eq!(r.send().await.unwrap().status(), 404, "{m} {p}");
        }
    }
    assert_eq!(e.http.get(format!("{}/api/remote", e.local)).send().await.unwrap().status(), 200);
    // DNS-rebinding guard on the local admin API
    assert_eq!(e.http.get(format!("{}/api/remote", e.local)).header("host", "evil.example").send().await.unwrap().status(), 403);
    // websocket: token required, never in the URL
    assert_eq!(e.c.get(format!("{}/api/ws", e.remote)).send().await.unwrap().status(), 401);
    assert_eq!(e.c.get(format!("{}/api/ws?token={tok}", e.remote)).send().await.unwrap().status(), 401);
    assert_eq!(e.get("/api/health", &tok).await, 200);
    assert_eq!(e.http.delete(format!("{}/api/remote/devices/{id}", e.local)).send().await.unwrap().status(), 204);
    assert_eq!(e.get("/api/health", &tok).await, 401, "revoked");
    let d: Vec<Value> = e.http.get(format!("{}/api/remote/devices", e.local)).send().await.unwrap().json().await.unwrap();
    assert!(d.is_empty());
}

#[tokio::test]
async fn audit_has_no_bodies_or_tokens_and_tokens_are_hashed() {
    let e = enable(fresh().0).await;
    let (id, tok) = e.pair().await;
    e.get("/api/jobs/some-secret-job-id", &tok).await;
    e.get("/api/keys", &tok).await;
    let a: Vec<Value> = e.http.get(format!("{}/api/remote/audit?limit=10", e.local)).send().await.unwrap().json().await.unwrap();
    let paths: Vec<_> = a.iter().map(|r| r["path"].as_str().unwrap()).collect();
    assert_eq!(paths, ["(denied)", "/api/jobs/:id"]);
    assert_eq!(a[0]["status"], 403);
    let (dump, hash): (String, String) = {
        let c = e.app.conn.lock().unwrap();
        let rows: Vec<String> = c.prepare("SELECT id||name||token_hash||coalesce(last_seen,'') FROM remote_devices UNION ALL SELECT ts||device_id||method||path_template||status FROM device_audit").unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
        (rows.join("\n"), c.query_row("SELECT token_hash FROM remote_devices WHERE id=?", [&id], |r| r.get(0)).unwrap())
    };
    assert!(!dump.contains(&tok) && !dump.contains("some-secret-job-id"));
    assert_eq!(hash, hex::encode(Sha256::digest(tok.as_bytes())));
}

#[tokio::test]
async fn fingerprint_survives_restart_and_rotate_revokes_devices() {
    let (app, dir) = fresh();
    let e = enable(app).await;
    let (_, tok) = e.pair().await;
    e.app.remote.stop();
    let app2 = App::open(&dir.join("app.db"), &dir.join("out")).unwrap();
    let e2 = enable(app2).await;
    assert_eq!(e2.fp, e.fp);
    assert_eq!(e2.get("/api/health", &tok).await, 200, "token still valid after restart");
    assert!(std::fs::metadata(dir.join("remote-key.pem")).unwrap().permissions().mode() & 0o077 == 0);
    assert_eq!(e2.http.post(format!("{}/api/remote/rotate", e2.local)).send().await.unwrap().status(), 400, "needs confirm");
    let s: Value = e2.http.post(format!("{}/api/remote/rotate", e2.local)).header("x-confirm", "rotate").send().await.unwrap().json().await.unwrap();
    assert_ne!(s["fingerprint"].as_str().unwrap(), e.fp);
    assert_eq!(s["devices"], 0);
    let c = pinned(s["fingerprint"].as_str().unwrap());
    let r = c.get(format!("https://127.0.0.1:{}/api/health", s["port"])).bearer_auth(&tok).send().await.unwrap();
    assert_eq!(r.status(), 401);
}

