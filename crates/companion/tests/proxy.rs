use resume_server::{api::router, App};
use serde_json::{json, Value};
use std::sync::Arc;

struct Desk { local: String, qr: Value, app: Arc<App>, http: reqwest::Client }

async fn desk() -> Desk {
    std::env::set_var("REMOTE_PORT", "0");
    let dir = std::env::temp_dir().join(format!("rc-desk-{}", uuid::Uuid::new_v4()));
    let app = App::open(&dir.join("app.db"), &dir.join("out")).unwrap();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local = format!("http://127.0.0.1:{}", l.local_addr().unwrap().port());
    let r = router(app.clone());
    tokio::spawn(async move { axum::serve(l, r).await.unwrap() });
    let http = reqwest::Client::new();
    http.post(format!("{local}/api/remote")).json(&json!({"enabled": true})).send().await.unwrap();
    let mut d = Desk { local, qr: Value::Null, app, http };
    d.new_code().await;
    d
}

impl Desk {
    async fn new_code(&mut self) {
        let r: Value = self.http.post(format!("{}/api/remote/pair-code", self.local)).send().await.unwrap().json().await.unwrap();
        self.qr = r["qr_payload"].clone();
        self.qr["hosts"] = json!(["nonexistent.invalid", "127.0.0.1"]); // first one: DNS failure, must fall through
    }
}

struct Phone { base: String, key: String, c: reqwest::Client, dir: std::path::PathBuf }
async fn phone(dir: Option<std::path::PathBuf>) -> Phone {
    let dir = dir.unwrap_or_else(|| std::env::temp_dir().join(format!("rc-phone-{}", uuid::Uuid::new_v4())));
    let (port, key, _) = resume_companion::serve(&dir).await.unwrap();
    let base = format!("http://127.0.0.1:{port}");
    let raw = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    let r = raw.get(format!("{base}/?k={key}")).send().await.unwrap();
    assert_eq!(r.status(), 302);
    let mut h = reqwest::header::HeaderMap::new(); // what the WebView's cookie jar does
    h.insert("cookie", r.headers()["set-cookie"].to_str().unwrap().split(';').next().unwrap().parse().unwrap());
    let c = reqwest::Client::builder().default_headers(h).build().unwrap();
    Phone { base, key, c, dir }
}
impl Phone {
    async fn pair(&self, payload: Value) -> reqwest::Response {
        self.c.post(format!("{}/companion/pair", self.base)).json(&json!({"payload": payload, "device_name": "Test phone"})).send().await.unwrap()
    }
    async fn status(&self) -> Value {
        self.c.get(format!("{}/companion/status", self.base)).send().await.unwrap().json().await.unwrap()
    }
}

#[tokio::test]
async fn pair_proxy_status_persist_unpair() {
    let d = desk().await;
    let p = phone(None).await;
    assert_eq!(p.status().await["paired"], false);
    assert_eq!(p.c.get(format!("{}/api/jobs", p.base)).send().await.unwrap().status(), 401, "unpaired");
    let r = p.pair(d.qr.clone()).await;
    assert_eq!(r.status(), 200);
    let s = p.status().await;
    assert_eq!((s["paired"].clone(), s["host"].clone(), s["device_name"].clone()), (json!(true), json!("127.0.0.1"), json!("Test phone")));
    assert_eq!(s["fingerprint_short"].as_str().unwrap(), &d.qr["fp"].as_str().unwrap()[..16]);
    assert!(!s.to_string().contains("token"));
    let r = p.c.get(format!("{}/api/jobs", p.base)).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert!(r.json::<Value>().await.unwrap().is_array());
    assert_eq!(p.c.get(format!("{}/api/settings", p.base)).send().await.unwrap().status(), 403, "scope enforced by the desktop");
    // the UI fallback still answers, and the file is private
    assert_ne!(p.c.get(format!("{}/", p.base)).send().await.unwrap().status(), 500);
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(p.dir.join("companion.json")).unwrap().permissions().mode() & 0o077, 0);
    // survives a restart of the proxy
    let p2 = phone(Some(p.dir.clone())).await;
    assert_eq!(p2.status().await["paired"], true);
    assert_eq!(p2.c.get(format!("{}/api/health", p2.base)).send().await.unwrap().status(), 200);
    assert_eq!(p2.c.delete(format!("{}/companion/unpair", p2.base)).send().await.unwrap().status(), 204);
    assert_eq!(p2.status().await["paired"], false);
    assert!(!p.dir.join("companion.json").exists());
    // foreign Host / Origin are refused
    assert_eq!(p2.c.get(format!("{}/companion/status", p2.base)).header("origin", "https://evil.example").send().await.unwrap().status(), 403);
    assert_eq!(p2.c.get(format!("{}/companion/status", p2.base)).header("host", "evil.example").send().await.unwrap().status(), 403);
}

#[tokio::test]
async fn pairing_errors() {
    let d = desk().await;
    let p = phone(None).await;
    let mut bad = d.qr.clone();
    bad["fp"] = json!("0".repeat(64));
    let r = p.pair(bad).await;
    assert_eq!((r.status().as_u16(), r.json::<Value>().await.unwrap()["code"].clone()), (502, json!("net.pinning_failed")));
    let mut bad = d.qr.clone();
    bad["code"] = json!("WRONGCOD");
    assert_eq!(p.pair(bad).await.status(), 403);
    assert_eq!(p.pair(json!({"hosts": ["127.0.0.1"]})).await.status(), 400);
    let mut down = d.qr.clone();
    down["hosts"] = json!(["nonexistent.invalid"]);
    let r = p.pair(down).await;
    assert_eq!((r.status().as_u16(), r.json::<Value>().await.unwrap()["code"].clone()), (503, json!("server.unreachable")));
    assert_eq!(p.status().await["paired"], false);
    // manual form + pasted text both work
    let m = json!({"host": "127.0.0.1", "port": d.qr["port"], "fp": d.qr["fp"], "code": d.qr["code"], "device_name": "Manual"});
    assert_eq!(p.c.post(format!("{}/companion/pair", p.base)).json(&m).send().await.unwrap().status(), 200);
    assert_eq!(p.status().await["device_name"], "Manual");
}

#[tokio::test]
async fn revoked_sse_and_unreachable() {
    let d = desk().await;
    let p = phone(None).await;
    assert_eq!(p.pair(json!(d.qr.to_string())).await.status(), 200, "pasted JSON text");
    let r = p.c.get(format!("{}/api/events", p.base)).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert!(r.headers()["content-type"].to_str().unwrap().starts_with("text/event-stream"));
    drop(r);
    // revoke -> 401 surfaces
    let devs: Vec<Value> = d.http.get(format!("{}/api/remote/devices", d.local)).send().await.unwrap().json().await.unwrap();
    d.http.delete(format!("{}/api/remote/devices/{}", d.local, devs[0]["id"].as_str().unwrap())).send().await.unwrap();
    assert_eq!(p.c.get(format!("{}/api/jobs", p.base)).send().await.unwrap().status(), 401);
    // desktop goes away -> 503
    d.app.remote.stop();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let r = p.c.get(format!("{}/api/jobs", p.base)).send().await.unwrap();
    assert_eq!((r.status().as_u16(), r.json::<Value>().await.unwrap()["code"].clone()), (503, json!("server.unreachable")));
}

#[tokio::test]
async fn share_is_consumed_once() {
    let p = phone(None).await;
    let get = || async { p.c.get(format!("{}/companion/share", p.base)).send().await.unwrap().json::<Value>().await.unwrap()["text"].clone() };
    assert_eq!(get().await, Value::Null);
    assert_eq!(p.c.post(format!("{}/companion/share", p.base)).json(&json!({"text": "https://example.com/job/1"})).send().await.unwrap().status(), 204);
    assert_eq!(get().await, json!("https://example.com/job/1"));
    assert_eq!(get().await, Value::Null);
}

#[tokio::test]
async fn secret_required() {
    let d = desk().await;
    let p = phone(None).await;
    let raw = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    for path in ["/", "/companion/status", "/api/jobs"] {
        let r = raw.get(format!("{}{path}", p.base)).send().await.unwrap();
        assert_eq!(r.status(), 403, "{path}");
        assert!(r.headers().get("access-control-allow-origin").is_none() && r.bytes().await.unwrap().is_empty());
    }
    assert_eq!(raw.get(format!("{}/?k=wrong", p.base)).send().await.unwrap().status(), 403);
    let r = raw.get(format!("{}/companion/status?k={}&x=1", p.base, p.key)).send().await.unwrap();
    assert_eq!((r.status().as_u16(), r.headers()["location"].to_str().unwrap()), (302, "/companion/status?x=1"));
    let ck = r.headers()["set-cookie"].to_str().unwrap().to_string();
    assert!(ck.starts_with("ck=") && ck.contains("HttpOnly") && ck.contains("SameSite=Strict") && !ck.contains(&p.key));
    let cookie = ck.split(';').next().unwrap();
    assert_eq!(raw.get(format!("{}/companion/status", p.base)).header("cookie", cookie).send().await.unwrap().status(), 200);
    assert_eq!(raw.get(format!("{}/companion/status", p.base)).header("cookie", "ck=nope").send().await.unwrap().status(), 403);
    // cookie then works through the proxy
    assert_eq!(p.pair(d.qr.clone()).await.status(), 200);
    assert_eq!(raw.get(format!("{}/api/jobs", p.base)).header("cookie", cookie).send().await.unwrap().status(), 200);
    // share POST: header only
    let post = || raw.post(format!("{}/companion/share", p.base)).json(&json!({"text": "x"}));
    assert_eq!(post().send().await.unwrap().status(), 403);
    assert_eq!(post().header("x-companion-key", "bad").send().await.unwrap().status(), 403);
    assert_eq!(post().header("x-companion-key", &p.key).send().await.unwrap().status(), 204);
    // the secret file is private
    use std::os::unix::fs::PermissionsExt;
    let f = p.dir.join("companion.port");
    assert_eq!(std::fs::metadata(&f).unwrap().permissions().mode() & 0o077, 0);
    assert!(std::fs::read_to_string(f).unwrap().ends_with(&p.key));
}
