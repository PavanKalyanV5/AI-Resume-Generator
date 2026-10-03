//! Remote access for the mobile companion: optional TLS listener (pinned self-signed cert), one-time pairing codes,
//! per-device bearer tokens (stored as sha256 only) and a default-deny scope allow-list. Admin routes live only on the
//! localhost router (`admin_routes`); the remote listener serves `router`, which never includes them.
use crate::{now, sanitize_name, App};
use anyhow::Result;
use axum::{body::Body, extract::{ConnectInfo, Path, Query, Request, State}, http::{header, HeaderValue, Method, StatusCode}, middleware::{from_fn, from_fn_with_state, Next}, response::{IntoResponse, Response}, routing::{delete, get, post}, Json, Router};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use futures_util::StreamExt;
use rand::RngCore;
use rusqlite::params;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, net::{IpAddr, Ipv4Addr, SocketAddr}, path::PathBuf, sync::{Arc, Mutex}};
use tokio_util::sync::CancellationToken;

pub const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS remote_devices(id TEXT PRIMARY KEY, name TEXT NOT NULL, token_hash TEXT NOT NULL UNIQUE, created_at INTEGER NOT NULL, last_seen INTEGER, revoked INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS device_audit(ts INTEGER NOT NULL, device_id TEXT NOT NULL, method TEXT NOT NULL, path_template TEXT NOT NULL, status INTEGER NOT NULL);";

const CODE_ALPHABET: &[u8; 32] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789"; // no I, O, 0, 1
const CODE_TTL_MS: i64 = 5 * 60_000;
const RATE_PER_MIN: u32 = 300;

/// Inserted into request extensions once a bearer token checked out.
#[derive(Clone)]
pub struct Authed {
    pub cancel: CancellationToken,
}

pub struct Remote {
    dir: PathBuf,
    listener: Mutex<Option<(axum_server::Handle<SocketAddr>, u16)>>,
    pub code: Mutex<Option<([u8; 32], i64)>>,
    /// ip -> (window start ms, attempts, locked until ms)
    tries: Mutex<HashMap<IpAddr, (i64, u32, i64)>>,
    rate: Mutex<HashMap<String, (i64, u32)>>,
    live: Mutex<HashMap<String, CancellationToken>>,
}

fn sha(s: &str) -> [u8; 32] {
    Sha256::digest(s.as_bytes()).into()
}
fn bad(c: StatusCode, m: &str) -> Response {
    (c, m.to_string()).into_response()
}
fn ie(e: impl std::fmt::Display) -> Response {
    bad(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string())
}

/// IPv4 addresses worth advertising: (ip, "lan" | "tailscale"). Tailscale = CGNAT 100.64.0.0/10.
pub fn addresses() -> Vec<(Ipv4Addr, &'static str)> {
    let mut v: Vec<_> = if_addrs::get_if_addrs().unwrap_or_default().into_iter().filter_map(|i| match i.ip() {
        IpAddr::V4(a) if !a.is_loopback() && !a.is_link_local() => Some((a, if a.octets()[0] == 100 && a.octets()[1] & 0xC0 == 0x40 { "tailscale" } else { "lan" })),
        _ => None,
    }).collect();
    v.sort();
    v.dedup();
    v
}

impl Remote {
    pub fn new(dir: PathBuf) -> Self {
        Remote { dir, listener: Mutex::new(None), code: Mutex::new(None), tries: Mutex::new(HashMap::new()), rate: Mutex::new(HashMap::new()), live: Mutex::new(HashMap::new()) }
    }
    fn cert(&self) -> PathBuf {
        self.dir.join("remote-cert.pem")
    }
    fn key(&self) -> PathBuf {
        self.dir.join("remote-key.pem")
    }
    pub fn port(&self) -> Option<u16> {
        self.listener.lock().unwrap().as_ref().map(|l| l.1)
    }

    /// Generate the self-signed cert once (825 days); it stays put so the fingerprint stays valid.
    fn ensure_cert(&self, a: &App, force: bool) -> Result<()> {
        if !force && self.cert().exists() && self.key().exists() {
            return Ok(());
        }
        use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair, SanType};
        let ips = addresses();
        let mut p = CertificateParams::default();
        p.distinguished_name = DistinguishedName::new();
        p.distinguished_name.push(DnType::CommonName, "Resume companion");
        let host = hostname::get().ok().and_then(|h| h.into_string().ok()).unwrap_or_else(|| "localhost".into());
        for n in [host.as_str(), "localhost"] {
            if let Ok(n) = n.to_string().try_into() {
                p.subject_alt_names.push(SanType::DnsName(n));
            }
        }
        p.subject_alt_names.push(SanType::IpAddress(Ipv4Addr::LOCALHOST.into()));
        p.subject_alt_names.extend(ips.iter().map(|(i, _)| SanType::IpAddress((*i).into())));
        let t = time::OffsetDateTime::now_utc();
        (p.not_before, p.not_after) = (t - time::Duration::days(1), t + time::Duration::days(825));
        let kp = KeyPair::generate()?;
        let cert = p.self_signed(&kp)?;
        std::fs::create_dir_all(&self.dir)?;
        std::fs::write(self.cert(), cert.pem())?;
        {
            use std::os::unix::fs::OpenOptionsExt;
            use std::io::Write;
            let _ = std::fs::remove_file(self.key());
            std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(self.key())?.write_all(kp.serialize_pem().as_bytes())?;
        }
        let list = json!(ips.iter().map(|(i, _)| i.to_string()).collect::<Vec<_>>()).to_string();
        a.conn.lock().unwrap().execute("INSERT INTO settings(key,value) VALUES('remote_cert_ips',?1) ON CONFLICT(key) DO UPDATE SET value=?1", [list])?;
        Ok(())
    }

    /// SHA-256 of the DER certificate, hex; None until the cert exists.
    pub fn fingerprint(&self) -> Option<String> {
        use rustls::pki_types::{pem::PemObject, CertificateDer};
        CertificateDer::from_pem_file(self.cert()).ok().map(|d| hex::encode(Sha256::digest(d.as_ref())))
    }

    fn tls(&self) -> Result<axum_server::tls_rustls::RustlsConfig> {
        use rustls::pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer};
        let certs = CertificateDer::pem_file_iter(self.cert())?.collect::<Result<Vec<_>, _>>()?;
        let key = PrivateKeyDer::from_pem_file(self.key())?;
        let mut c = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider())).with_safe_default_protocol_versions()?.with_no_client_auth().with_single_cert(certs, key)?;
        c.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
        Ok(axum_server::tls_rustls::RustlsConfig::from_config(Arc::new(c)))
    }

    /// Start the TLS listener on 0.0.0.0:$REMOTE_PORT (default 8788); returns the bound port.
    pub async fn start(&self, a: &Arc<App>) -> Result<u16> {
        if let Some(p) = self.port() {
            return Ok(p);
        }
        self.ensure_cert(a, false)?;
        let cfg = self.tls()?;
        let port: u16 = std::env::var("REMOTE_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(8788);
        let l = std::net::TcpListener::bind(("0.0.0.0", port))?;
        l.set_nonblocking(true)?;
        let port = l.local_addr()?.port();
        let h = axum_server::Handle::<SocketAddr>::new();
        let srv = axum_server::from_tcp_rustls(l, cfg)?.handle(h.clone());
        let svc = router(a.clone()).into_make_service_with_connect_info::<SocketAddr>();
        tokio::spawn(async move {
            let _ = srv.serve(svc).await;
        });
        *self.listener.lock().unwrap() = Some((h, port));
        Ok(port)
    }

    pub fn stop(&self) {
        if let Some((h, _)) = self.listener.lock().unwrap().take() {
            h.shutdown();
        }
        *self.code.lock().unwrap() = None;
    }

    fn live(&self, dev: &str) -> CancellationToken {
        self.live.lock().unwrap().entry(dev.into()).or_default().clone()
    }

    /// 5 pairing attempts per minute per IP, then a 10 minute lockout.
    fn pair_allowed(&self, ip: IpAddr) -> bool {
        let t = now();
        let mut m = self.tries.lock().unwrap();
        let e = m.entry(ip).or_insert((t, 0, 0));
        if e.2 > t {
            return false;
        }
        if t - e.0 >= 60_000 {
            *e = (t, 0, 0);
        }
        e.1 += 1;
        if e.1 > 5 {
            e.2 = t + 600_000;
            return false;
        }
        true
    }

    fn device_allowed(&self, dev: &str) -> bool {
        let t = now();
        let mut m = self.rate.lock().unwrap();
        let e = m.entry(dev.into()).or_insert((t, 0));
        if t - e.0 >= 60_000 {
            *e = (t, 0);
        }
        e.1 += 1;
        e.1 <= RATE_PER_MIN
    }
}

/// Start the listener at boot if the owner left it on.
pub async fn autostart(a: &Arc<App>) {
    if a.setting("remote_enabled").await.as_deref() == Some("1") {
        match a.remote.start(a).await {
            Ok(p) => eprintln!("remote listener on 0.0.0.0:{p} (TLS)"),
            Err(e) => eprintln!("remote listener failed: {e}"),
        }
    }
}

/// Default-deny scope: returns the audit path template when (method, path) may be called by a paired phone.
fn allowed(m: &Method, path: &str) -> Option<String> {
    let s: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    let (g, p) = (m == Method::GET, m == Method::POST);
    let t = match s.as_slice() {
        ["api", "health"] if g => "/api/health".into(),
        ["api", "jobs"] if g || p => "/api/jobs".into(),
        ["api", "jobs", _] if g => "/api/jobs/:id".into(),
        ["api", "jobs", _, "events"] if g => "/api/jobs/:id/events".into(),
        ["api", "jobs", _, "corrections"] if g => "/api/jobs/:id/corrections".into(),
        ["api", "jobs", _, "files", _] if g => "/api/jobs/:id/files/:name".into(),
        ["api", "jobs", _, op @ ("overrides" | "feedback" | "edit" | "approve" | "retry" | "cancel" | "jd-class")] if p => format!("/api/jobs/:id/{op}"),
        ["api", "jd", "fetch"] if p => "/api/jd/fetch".into(),
        ["api", "library"] if g => "/api/library".into(),
        ["api", "notifications"] if g => "/api/notifications".into(),
        ["api", "notifications", "read"] if p => "/api/notifications/read".into(),
        ["api", "ws"] if g => "/api/ws".into(),
        ["api", "events"] if g => "/api/events".into(),
        ["api", "applications"] if g || p => "/api/applications".into(),
        ["api", "applications", "funnel"] if g => "/api/applications/funnel".into(),
        ["api", "playbook"] if g => "/api/playbook".into(),
        ["api", "playbook", "rules", _, op @ ("approve" | "reject" | "disable" | "enable")] if p => format!("/api/playbook/rules/:id/{op}"),
        ["api", "corrections", _, "reason"] if p => "/api/corrections/:id/reason".into(),
        ["api", "learning", "metrics"] if g => "/api/learning/metrics".into(),
        ["api", "regression", "latest"] if g => "/api/regression/latest".into(),
        _ => return None,
    };
    Some(t)
}

fn bearer(h: &axum::http::HeaderMap, path: &str) -> Option<String> {
    if let Some(t) = h.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer ")) {
        return Some(t.trim().to_string());
    }
    // Browsers cannot set headers on WebSockets: accept `Sec-WebSocket-Protocol: bearer.<token>` there (never a URL token).
    (path == "/api/ws").then(|| h.get_all(header::SEC_WEBSOCKET_PROTOCOL).iter().filter_map(|v| v.to_str().ok()).flat_map(|v| v.split(',')).find_map(|p| p.trim().strip_prefix("bearer.").map(String::from))).flatten()
}

fn headers(res: &mut Response, api: bool) {
    let h = res.headers_mut();
    if api {
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    h.entry(header::CONTENT_SECURITY_POLICY).or_insert(HeaderValue::from_static("frame-ancestors 'none'"));
}

/// Everything the remote listener serves: the API subset behind `gate`, /pair, and the built web UI.
pub fn router(app: Arc<App>) -> Router {
    let r = crate::api::api_routes().route("/pair", post(pair)).with_state(app.clone());
    resume_ui::with_ui(r).layer(from_fn_with_state(app, gate))
}

async fn gate(State(a): State<Arc<App>>, ConnectInfo(addr): ConnectInfo<SocketAddr>, mut req: Request, next: Next) -> Response {
    let (m, path) = (req.method().clone(), req.uri().path().to_string());
    let api = path.starts_with("/api");
    let mut res = if path.starts_with("/api/remote") {
        StatusCode::NOT_FOUND.into_response() // admin API does not exist here
    } else if (m == Method::POST && path == "/pair") || (!api && matches!(m, Method::GET | Method::HEAD)) {
        if path == "/pair" && !a.remote.pair_allowed(addr.ip()) {
            bad(StatusCode::TOO_MANY_REQUESTS, "too many attempts; try again later")
        } else {
            next.run(req).await
        }
    } else {
        let Some(tok) = bearer(req.headers(), &path) else { return headers_on(bad(StatusCode::UNAUTHORIZED, "token required"), true) };
        let h = hex::encode(sha(&tok));
        let dev = a.db(move |c| {
            let id: Option<String> = c.query_row("SELECT id FROM remote_devices WHERE token_hash=?1 AND revoked=0", [&h], |r| r.get(0)).ok();
            if id.is_some() {
                c.execute("UPDATE remote_devices SET last_seen=?2 WHERE token_hash=?1", params![h, now()])?;
            }
            Ok(id)
        }).await.ok().flatten();
        let Some(dev) = dev else { return headers_on(bad(StatusCode::UNAUTHORIZED, "unknown or revoked token"), true) };
        let tpl = allowed(&m, &path);
        let cancel = a.remote.live(&dev);
        let mut res = if !a.remote.device_allowed(&dev) {
            bad(StatusCode::TOO_MANY_REQUESTS, "slow down")
        } else if tpl.is_none() {
            bad(StatusCode::FORBIDDEN, "not available from the phone")
        } else {
            req.extensions_mut().insert(Authed { cancel: cancel.clone() });
            next.run(req).await
        };
        if res.headers().get(header::CONTENT_TYPE).is_some_and(|v| v.as_bytes().starts_with(b"text/event-stream")) {
            // best effort: a revoke ends live SSE streams
            let (p, body) = res.into_parts();
            let mut s = body.into_data_stream();
            res = Response::from_parts(p, Body::from_stream(async_stream::stream! {
                loop {
                    tokio::select! { _ = cancel.cancelled() => break, x = s.next() => match x { Some(x) => yield x, None => break } }
                }
            }));
        }
        let (tpl, st, m) = (tpl.unwrap_or_else(|| "(denied)".into()), res.status().as_u16(), m.to_string());
        let _ = a.db(move |c| Ok(c.execute("INSERT INTO device_audit(ts,device_id,method,path_template,status) VALUES(?,?,?,?,?)", params![now(), dev, m, tpl, st])?)).await;
        res
    };
    headers(&mut res, api);
    res
}

fn headers_on(mut r: Response, api: bool) -> Response {
    headers(&mut r, api);
    r
}

async fn pair(State(a): State<Arc<App>>, Json(b): Json<Value>) -> Response {
    let code = b["code"].as_str().unwrap_or("").trim().to_uppercase();
    let name = sanitize_name(b["device_name"].as_str().unwrap_or(""), 40);
    let ok = {
        let mut g = a.remote.code.lock().unwrap();
        let hit = g.as_ref().is_some_and(|(h, exp)| *exp > now() && h.iter().zip(sha(&code)).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0);
        if hit {
            *g = None; // single use
        }
        hit
    };
    if !ok {
        return bad(StatusCode::FORBIDDEN, "invalid or expired code");
    }
    let (id, mut raw) = (uuid::Uuid::new_v4().to_string(), [0u8; 32]);
    rand::thread_rng().fill_bytes(&mut raw);
    let token = URL_SAFE_NO_PAD.encode(raw);
    let name = if name.is_empty() { "Phone".to_string() } else { name };
    let (i, n, h) = (id.clone(), name.clone(), hex::encode(sha(&token)));
    if let Err(e) = a.db(move |c| Ok(c.execute("INSERT INTO remote_devices(id,name,token_hash,created_at) VALUES(?,?,?,?)", params![i, n, h, now()])?)).await {
        return ie(e);
    }
    a.notify("info", "remote.paired", "Phone paired", &format!("{name} can now use the companion app."));
    Json(json!({"device_id": id, "token": token})).into_response()
}

// ---- admin (localhost router only) ----

pub fn admin_routes() -> Router<Arc<App>> {
    Router::new().route("/api/remote", get(status).post(toggle)).route("/api/remote/pair-code", post(pair_code)).route("/api/remote/rotate", post(rotate))
        .route("/api/remote/devices", get(devices)).route("/api/remote/devices/:id", delete(revoke)).route("/api/remote/audit", get(audit))
        .layer(from_fn(local_only))
}

/// Admin answers only to a localhost Host header (DNS-rebinding guard) and is never cached.
async fn local_only(req: Request, next: Next) -> Response {
    let ok = req.headers().get(header::HOST).and_then(|h| h.to_str().ok()).is_none_or(|h| {
        let host = if h.starts_with('[') { h.split_inclusive(']').next().unwrap_or("") } else { h.split(':').next().unwrap_or("") };
        matches!(host, "localhost" | "127.0.0.1" | "[::1]")
    });
    let mut r = if ok { next.run(req).await } else { bad(StatusCode::FORBIDDEN, "local only") };
    headers(&mut r, true);
    r
}

async fn status_json(a: &Arc<App>) -> Value {
    let ips = addresses();
    let have = a.setting("remote_cert_ips").await.and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok()).unwrap_or_default();
    let n: i64 = a.db(|c| Ok(c.query_row("SELECT count(*) FROM remote_devices WHERE revoked=0", [], |r| r.get(0))?)).await.unwrap_or(0);
    let fp = a.remote.fingerprint();
    json!({"enabled": a.remote.port().is_some(), "port": a.remote.port(), "fingerprint": fp, "hostname": hostname::get().ok().and_then(|h| h.into_string().ok()),
        "addresses": ips.iter().map(|(i, k)| json!({"ip": i.to_string(), "kind": k})).collect::<Vec<_>>(), "devices": n,
        "cert_stale": fp.is_some() && ips.iter().any(|(i, _)| !have.contains(&i.to_string()))})
}

async fn status(State(a): State<Arc<App>>) -> Json<Value> {
    Json(status_json(&a).await)
}

async fn toggle(State(a): State<Arc<App>>, Json(b): Json<Value>) -> Response {
    let Some(on) = b["enabled"].as_bool() else { return bad(StatusCode::BAD_REQUEST, "enabled (bool) required") };
    if on {
        if let Err(e) = a.remote.start(&a).await {
            return ie(e);
        }
    } else {
        a.remote.stop();
    }
    let _ = a.set_setting("remote_enabled", on.then(|| "1".to_string())).await;
    Json(status_json(&a).await).into_response()
}

/// Regenerate the cert (use when the IP set changed) and revoke every device. Needs `x-confirm: rotate`.
async fn rotate(State(a): State<Arc<App>>, h: axum::http::HeaderMap) -> Response {
    if h.get("x-confirm").and_then(|v| v.to_str().ok()) != Some("rotate") {
        return bad(StatusCode::BAD_REQUEST, "missing x-confirm: rotate header");
    }
    let was = a.remote.port().is_some();
    a.remote.stop();
    let r = async {
        a.remote.ensure_cert(&a, true)?;
        a.db(|c| Ok(c.execute("UPDATE remote_devices SET revoked=1", [])?)).await?;
        a.remote.live.lock().unwrap().drain().for_each(|(_, t)| t.cancel());
        if was {
            a.remote.start(&a).await?;
        }
        Ok::<_, anyhow::Error>(())
    }.await;
    match r {
        Ok(()) => Json(status_json(&a).await).into_response(),
        Err(e) => ie(e),
    }
}

async fn pair_code(State(a): State<Arc<App>>) -> Response {
    let Some(port) = a.remote.port() else { return bad(StatusCode::CONFLICT, "turn on remote access first") };
    let mut rb = [0u8; 8];
    rand::thread_rng().fill_bytes(&mut rb);
    let code: String = rb.iter().map(|b| CODE_ALPHABET[(*b & 31) as usize] as char).collect();
    let exp = now() + CODE_TTL_MS;
    *a.remote.code.lock().unwrap() = Some((sha(&code), exp));
    let mut hosts: Vec<String> = addresses().iter().map(|(i, _)| i.to_string()).collect();
    hosts.extend(hostname::get().ok().and_then(|h| h.into_string().ok()));
    Json(json!({"code": code, "expires_at": exp, "qr_payload": {"v": 1, "hosts": hosts, "port": port, "fp": a.remote.fingerprint(), "code": code}})).into_response()
}

async fn devices(State(a): State<Arc<App>>) -> Response {
    let r = a.db(|c| {
        let mut s = c.prepare("SELECT id,name,created_at,last_seen FROM remote_devices WHERE revoked=0 ORDER BY created_at DESC")?;
        let v = s.query_map([], |r| Ok(json!({"id": r.get::<_, String>(0)?, "name": r.get::<_, String>(1)?, "created_at": r.get::<_, i64>(2)?, "last_seen": r.get::<_, Option<i64>>(3)?, "revoked": false})))?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(v)
    }).await;
    r.map(|v| Json(json!(v)).into_response()).unwrap_or_else(ie)
}

async fn revoke(State(a): State<Arc<App>>, Path(id): Path<String>) -> Response {
    let i = id.clone();
    match a.db(move |c| Ok(c.execute("UPDATE remote_devices SET revoked=1 WHERE id=? AND revoked=0", [i])?)).await {
        Ok(0) => StatusCode::NOT_FOUND.into_response(),
        Ok(_) => {
            if let Some(t) = a.remote.live.lock().unwrap().remove(&id) {
                t.cancel(); // closes its live WS/SSE
            }
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => ie(e),
    }
}

async fn audit(State(a): State<Arc<App>>, Query(q): Query<HashMap<String, String>>) -> Response {
    let lim = q.get("limit").and_then(|l| l.parse::<i64>().ok()).unwrap_or(50).clamp(1, 500);
    let r = a.db(move |c| {
        let mut s = c.prepare("SELECT a.ts,a.device_id,coalesce(d.name,''),a.method,a.path_template,a.status FROM device_audit a LEFT JOIN remote_devices d ON d.id=a.device_id ORDER BY a.rowid DESC LIMIT ?")?;
        let v = s.query_map([lim], |r| Ok(json!({"ts": r.get::<_, i64>(0)?, "device_id": r.get::<_, String>(1)?, "device": r.get::<_, String>(2)?, "method": r.get::<_, String>(3)?, "path": r.get::<_, String>(4)?, "status": r.get::<_, i64>(5)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(v)
    }).await;
    r.map(|v| Json(json!(v)).into_response()).unwrap_or_else(ie)
}
