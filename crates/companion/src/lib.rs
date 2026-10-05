//! Phone-side companion proxy: serves the web UI on 127.0.0.1:<random port> and forwards /api/* to the paired desktop over
//! HTTPS, trusting exactly one certificate (SHA-256 pin from the pairing QR) and adding the stored bearer token.
use axum::{body::{to_bytes, Body}, extract::{Request, State}, http::{header, HeaderMap, StatusCode}, middleware::{from_fn, Next}, response::{IntoResponse, Response}, routing::{any, delete, get, post}, Json, Router};
use rustls::{client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier}, crypto::{ring::default_provider, CryptoProvider}, pki_types::{CertificateDer, ServerName, UnixTime}, DigitallySignedStruct, SignatureScheme};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{error::Error, path::PathBuf, sync::{Arc, Mutex}, time::Duration};

#[derive(Clone, Serialize, Deserialize)]
struct Cfg { hosts: Vec<String>, port: u16, fp: String, token: String, device_name: String }

struct St { dir: PathBuf, cfg: Mutex<Option<(Cfg, reqwest::Client)>>, share: Mutex<Option<String>> }
type S = State<Arc<St>>;

/// Accepts exactly one certificate: the one whose SHA-256 equals the pinned fingerprint. No CA trust.
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
    reqwest::Client::builder().use_preconfigured_tls(cfg).connect_timeout(Duration::from_secs(3)).build().unwrap()
}

fn err(c: StatusCode, code: &str, message: &str) -> Response {
    (c, Json(json!({"code": code, "message": message}))).into_response()
}
fn pin_failed() -> Response {
    err(StatusCode::BAD_GATEWAY, "net.pinning_failed", "The desktop's certificate does not match the one you paired with.")
}
fn unreachable() -> Response {
    err(StatusCode::SERVICE_UNAVAILABLE, "server.unreachable", "Cannot reach your desktop. Is it on and on the same network?")
}
fn is_pin_failure(e: &reqwest::Error) -> bool {
    let mut s: Option<&dyn Error> = Some(e);
    while let Some(x) = s {
        if x.to_string().contains("fingerprint mismatch") {
            return true;
        }
        s = x.source();
    }
    false
}

/// Owner-only file options: mode 0600 on Unix; on Windows the app-data folder's ACL already limits it to the user.
fn private_options() -> std::fs::OpenOptions {
    let mut o = std::fs::OpenOptions::new();
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
    o
}

impl St {
    fn file(&self) -> PathBuf {
        self.dir.join("companion.json")
    }
    fn save(&self, c: &Cfg) -> std::io::Result<()> {
        use std::io::Write;
        let tmp = self.dir.join("companion.json.tmp");
        let _ = std::fs::remove_file(&tmp);
        private_options().write(true).create_new(true).open(&tmp)?.write_all(serde_json::to_vec(c)?.as_slice())?;
        std::fs::rename(tmp, self.file())
    }
    fn get(&self) -> Option<(Cfg, reqwest::Client)> {
        self.cfg.lock().unwrap().clone()
    }
    /// Remember the host that answered by moving it to the front.
    fn mark_good(&self, host: &str) {
        let mut g = self.cfg.lock().unwrap();
        if let Some((c, _)) = g.as_mut().filter(|(c, _)| c.hosts.first().map(String::as_str) != Some(host)) {
            c.hosts.retain(|h| h != host);
            c.hosts.insert(0, host.into());
            let _ = self.save(c);
        }
    }
}

/// Start the proxy + UI. Returns the bound port and a fresh per-launch secret (open `/?k=<secret>` once); both are also
/// written (mode 0600) to `<config_dir>/companion.port` as "port\nsecret" for the Android share handler.
pub async fn serve(config_dir: impl Into<PathBuf>) -> anyhow::Result<(u16, String, tokio::task::JoinHandle<()>)> {
    let dir = config_dir.into();
    std::fs::create_dir_all(&dir)?;
    let cfg = std::fs::read(dir.join("companion.json")).ok().and_then(|b| serde_json::from_slice::<Cfg>(&b).ok()).map(|c| { let cl = pinned(&c.fp); (c, cl) });
    let mut raw = [0u8; 32];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut raw);
    let key = hex::encode(raw);
    let ck = hex::encode(Sha256::digest(format!("ck:{key}")));
    let st = Arc::new(St { dir: dir.clone(), cfg: Mutex::new(cfg), share: Mutex::new(None) });
    let r = Router::new()
        .route("/companion/status", get(status))
        .route("/companion/pair", post(pair))
        .route("/companion/unpair", delete(unpair))
        .route("/companion/share", get(share_get).post(share_set))
        .route("/api/*rest", any(proxy))
        .with_state(st);
    let l = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let port = l.local_addr()?.port();
    {
        use std::io::Write;
        let f = dir.join("companion.port");
        let _ = std::fs::remove_file(&f);
        private_options().write(true).create_new(true).open(f)?.write_all(format!("{port}\n{key}").as_bytes())?;
    }
    let k = key.clone();
    let h = tokio::spawn(async move {
        let _ = axum::serve(l, resume_ui::with_ui(r).layer(from_fn(move |req, next| local_only(k.clone(), ck.clone(), req, next)))).await;
    });
    Ok((port, key, h))
}

/// Only this device's loopback origin may talk to us (blocks DNS rebinding and cross-site requests from other pages).
/// Every request must also carry the per-launch secret: `?k=` (once; answered with a cookie + redirect that drops it),
/// the derived `ck` cookie, or the `x-companion-key` header (native share handler). Otherwise an empty 403.
async fn local_only(key: String, ck: String, req: Request, next: Next) -> Response {
    let h = req.headers();
    let host_ok = h.get(header::HOST).and_then(|v| v.to_str().ok()).is_none_or(|v| v.starts_with("127.0.0.1:") || v == "127.0.0.1");
    let origin_ok = h.get(header::ORIGIN).and_then(|v| v.to_str().ok()).is_none_or(|v| v.starts_with("http://127.0.0.1:"));
    if !(host_ok && origin_ok) {
        return err(StatusCode::FORBIDDEN, "companion.local_only", "local only");
    }
    let eq = |a: &str, b: &str| a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0, |x, (p, q)| x | (p ^ q)) == 0;
    let q = req.uri().query().unwrap_or("");
    if q.split('&').find_map(|p| p.strip_prefix("k=")).filter(|k| eq(k, &key)).is_some() {
        let rest: Vec<&str> = q.split('&').filter(|p| !p.starts_with("k=")).collect();
        let loc = if rest.is_empty() { req.uri().path().to_string() } else { format!("{}?{}", req.uri().path(), rest.join("&")) };
        return Response::builder().status(StatusCode::FOUND).header(header::LOCATION, loc).header(header::SET_COOKIE, format!("ck={ck}; HttpOnly; SameSite=Strict; Path=/")).body(Body::empty()).unwrap();
    }
    let cookie_ok = h.get_all(header::COOKIE).iter().filter_map(|v| v.to_str().ok()).flat_map(|v| v.split(';')).any(|c| c.trim().strip_prefix("ck=").is_some_and(|v| eq(v, &ck)));
    let hdr_ok = h.get("x-companion-key").and_then(|v| v.to_str().ok()).is_some_and(|v| eq(v, &key));
    if cookie_ok || hdr_ok { next.run(req).await } else { StatusCode::FORBIDDEN.into_response() }
}

async fn status(State(s): S) -> Json<Value> {
    Json(match s.get() {
        Some((c, _)) => json!({"paired": true, "host": c.hosts.first(), "device_name": c.device_name, "fingerprint_short": &c.fp[..16.min(c.fp.len())]}),
        None => json!({"paired": false, "host": null, "device_name": null, "fingerprint_short": null}),
    })
}

fn clean_host(h: &str) -> Option<String> {
    let h = h.trim();
    (!h.is_empty() && h.len() < 254 && h.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')).then(|| h.to_string())
}

/// Accepts the desktop's qr_payload ({v,hosts,port,fp,code}) or {host,port,fp,code}, plus device_name.
async fn pair(State(s): S, Json(b): Json<Value>) -> Response {
    let p = match &b["payload"] {
        Value::String(t) => serde_json::from_str(t.trim()).unwrap_or(Value::Null),
        Value::Object(_) => b["payload"].clone(),
        _ => b.clone(),
    };
    pair_with(s, p, &b).await
}

async fn pair_with(s: Arc<St>, p: Value, b: &Value) -> Response {
    let mut hosts: Vec<String> = p["hosts"].as_array().map(|a| a.iter().filter_map(|h| h.as_str()).filter_map(clean_host).collect()).unwrap_or_default();
    hosts.extend(p["host"].as_str().and_then(clean_host));
    let fp: String = p["fp"].as_str().unwrap_or("").chars().filter(|c| *c != ':' && !c.is_whitespace()).collect::<String>().to_lowercase();
    let port = p["port"].as_u64().or_else(|| p["port"].as_str().and_then(|x| x.trim().parse().ok())).and_then(|x| u16::try_from(x).ok()).filter(|x| *x > 0);
    let code = p["code"].as_str().unwrap_or("").trim().to_string();
    let name = b["device_name"].as_str().unwrap_or("").trim().to_string();
    let (Some(port), true, false, false) = (port, fp.len() == 64 && fp.chars().all(|c| c.is_ascii_hexdigit()), hosts.is_empty(), code.is_empty()) else {
        return err(StatusCode::BAD_REQUEST, "pair.bad_payload", "Pairing details are incomplete: need host, port, fingerprint and code.");
    };
    let client = pinned(&fp);
    let (mut pin, mut last) = (false, None);
    for h in &hosts {
        let r = client.post(format!("https://{h}:{port}/pair")).timeout(Duration::from_secs(10)).json(&json!({"code": code, "device_name": name})).send().await;
        match r {
            Err(e) => pin |= is_pin_failure(&e),
            Ok(r) if r.status().is_success() => {
                let Some(token) = r.json::<Value>().await.ok().and_then(|v| v["token"].as_str().map(String::from)) else { return err(StatusCode::BAD_GATEWAY, "pair.bad_reply", "The desktop sent an unreadable reply.") };
                let mut hs = vec![h.clone()];
                hs.extend(hosts.iter().filter(|x| *x != h).cloned());
                let c = Cfg { hosts: hs, port, fp: fp.clone(), token, device_name: if name.is_empty() { "Phone".into() } else { name } };
                if let Err(e) = s.save(&c) {
                    return err(StatusCode::INTERNAL_SERVER_ERROR, "pair.save_failed", &format!("Could not store the pairing: {e}"));
                }
                *s.cfg.lock().unwrap() = Some((c, client));
                return status(State(s)).await.into_response();
            }
            Ok(r) => { last = Some(r.status()); break } // the desktop answered: no point trying its other addresses
        }
    }
    match last {
        Some(StatusCode::FORBIDDEN) => err(StatusCode::FORBIDDEN, "pair.rejected", "The code is wrong or expired. Generate a new one on the desktop."),
        Some(StatusCode::TOO_MANY_REQUESTS) => err(StatusCode::TOO_MANY_REQUESTS, "pair.locked", "Too many attempts. Wait a few minutes and try again."),
        Some(c) => err(StatusCode::BAD_GATEWAY, "pair.failed", &format!("The desktop refused pairing ({c}).")),
        None if pin => pin_failed(),
        None => unreachable(),
    }
}

async fn unpair(State(s): S) -> StatusCode {
    *s.cfg.lock().unwrap() = None;
    let _ = std::fs::remove_file(s.file());
    StatusCode::NO_CONTENT
}

async fn share_set(State(s): S, Json(b): Json<Value>) -> StatusCode {
    *s.share.lock().unwrap() = b["text"].as_str().map(|t| t.trim().chars().take(20_000).collect::<String>()).filter(|t| !t.is_empty());
    StatusCode::NO_CONTENT
}

/// Pending shared text, handed out once.
async fn share_get(State(s): S) -> Json<Value> {
    Json(json!({"text": s.share.lock().unwrap().take()}))
}

async fn proxy(State(s): S, req: Request) -> Response {
    let Some((cfg, client)) = s.get() else { return err(StatusCode::UNAUTHORIZED, "companion.unpaired", "Not paired with a desktop yet.") };
    let (parts, body) = req.into_parts();
    let Ok(body) = to_bytes(body, 64 << 20).await else { return err(StatusCode::PAYLOAD_TOO_LARGE, "companion.too_large", "Request too large.") };
    let pq = parts.uri.path_and_query().map_or("/", |p| p.as_str()).to_string();
    let mut pin = false;
    for h in &cfg.hosts {
        let mut hs = HeaderMap::new();
        for (k, v) in &parts.headers {
            if !matches!(k.as_str(), "host" | "authorization" | "connection" | "content-length" | "transfer-encoding" | "origin" | "referer" | "cookie") {
                hs.append(k, v.clone());
            }
        }
        let r = client.request(parts.method.clone(), format!("https://{h}:{}{pq}", cfg.port)).headers(hs).bearer_auth(&cfg.token).body(body.clone()).send().await;
        match r {
            Err(e) => pin |= is_pin_failure(&e),
            Ok(r) => {
                s.mark_good(h);
                let mut b = Response::builder().status(r.status());
                for (k, v) in r.headers() {
                    if !matches!(k.as_str(), "connection" | "transfer-encoding" | "keep-alive") {
                        b = b.header(k, v);
                    }
                }
                return b.body(Body::from_stream(r.bytes_stream())).unwrap();
            }
        }
    }
    if pin { pin_failed() } else { unreachable() }
}
