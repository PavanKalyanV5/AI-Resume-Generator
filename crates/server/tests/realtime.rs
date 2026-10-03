use futures_util::{SinkExt, StreamExt};
use resume_server::{api::router, create_job, errors::AppError, fetch::FetchErr, providers::{mock::MockProvider, ProviderError}, realtime::{emit, Ev}, App};
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};
use tokio_tungstenite::{connect_async, tungstenite::{client::IntoClientRequest, Message}};

const JD: &str = "Senior Backend Engineer\nRequirements:\n- 5+ years of Python and Kubernetes\n";

fn setup(p: Arc<MockProvider>) -> Arc<App> {
    let dir = std::env::temp_dir().join(format!("rs-rt-{}", uuid::Uuid::new_v4()));
    let app = App::open(&dir.join("app.db"), &dir.join("out")).unwrap();
    let r = json!({"profile": {"name": "Jane Doe", "email": "jane.doe@acme-robotics.test", "phone": "+1 555 010 4477", "role": "Engineer", "location": "Springfield"}, "summary": ["Engineer."],
        "experience": [{"id": "x1", "role": "Backend Engineer", "organization": "Acme Robotics", "dateLabel": "2020-2024", "bullets": ["Built Python services on Kubernetes"]}],
        "skills": [{"label": "Cloud", "skills": ["Python", "Kubernetes"]}]});
    app.conn.lock().unwrap().execute("INSERT INTO resume VALUES(1,?)", [r.to_string()]).unwrap();
    *app.provider_override.lock().unwrap() = Some(p);
    app
}

async fn serve(a: &Arc<App>) -> String {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let r = router(a.clone());
    tokio::spawn(async move { axum::serve(l, r).await.unwrap() });
    format!("127.0.0.1:{}", addr.port())
}

async fn run_job(a: &Arc<App>) -> String {
    let id = create_job(&mut a.conn.lock().unwrap(), JD, "mock", &[], None).unwrap();
    a.run_next().await.unwrap();
    id
}

async fn ws(host: &str, since: i64, origin: Option<&str>) -> Result<tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>, tokio_tungstenite::tungstenite::Error> {
    let mut req = format!("ws://{host}/api/ws?since={since}").into_client_request().unwrap();
    if let Some(o) = origin {
        req.headers_mut().insert("origin", o.parse().unwrap());
    }
    connect_async(req).await.map(|r| r.0)
}

async fn next<S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin>(s: &mut S) -> Value {
    loop {
        match tokio::time::timeout(Duration::from_secs(5), s.next()).await.expect("frame in time").expect("open").unwrap() {
            Message::Text(t) => return serde_json::from_str(&t).unwrap(),
            _ => {}
        }
    }
}

#[tokio::test]
async fn ws_hello_replay_live_and_client_frames() {
    std::env::set_var("RESUME_PDF_ENGINE", "native");
    let a = setup(Arc::new(MockProvider::default()));
    let host = serve(&a).await;
    let id = run_job(&a).await;
    let total: i64 = a.conn.lock().unwrap().query_row("SELECT count(*) FROM job_events", [], |r| r.get(0)).unwrap();
    let mut c = ws(&host, 0, Some("tauri://localhost")).await.unwrap();
    let hello = next(&mut c).await;
    assert_eq!((hello["t"].as_str(), hello["seq_head"].as_i64()), (Some("hello"), Some(total)));
    let mut seqs = vec![];
    for _ in 0..total {
        let e = next(&mut c).await;
        assert_eq!(e["t"], "event");
        assert_eq!(e["scope"], "job");
        assert_eq!(e["job_id"], id.as_str());
        seqs.push(e["seq"].as_i64().unwrap());
        if e["kind"] == "job_status" {
            assert_eq!((e["level"].as_str(), e["actions"][0]["target"].as_str()), (Some("success"), Some(format!("/jobs/{id}").as_str())));
        }
    }
    assert_eq!(seqs, (1..=total).collect::<Vec<_>>(), "in order, no gaps");
    c.send(Message::Text(json!({"t": "pong"}).to_string().into())).await.unwrap();
    c.send(Message::Text(json!({"t": "ack", "seq": total}).to_string().into())).await.unwrap();
    c.send(Message::Text(json!({"t": "sub", "job_ids": ["other"]}).to_string().into())).await.unwrap();
    a.notify("info", "model.ready", "Model ready", "Local model is ready");
    let e = next(&mut c).await; // not a duplicate of the replay
    assert_eq!((e["seq"].as_i64(), e["scope"].as_str(), e["kind"].as_str(), e["code"].as_str()), (Some(total + 1), Some("system"), Some("notice"), Some("model.ready")));
    assert!(e.get("job_id").is_none());
    // since=N skips what the client already has
    let mut c2 = ws(&host, total, None).await.unwrap();
    assert_eq!(next(&mut c2).await["t"], "hello");
    assert_eq!(next(&mut c2).await["seq"], total + 1);
}

#[tokio::test]
async fn ws_rejects_foreign_origin() {
    let a = setup(Arc::new(MockProvider::default()));
    let host = serve(&a).await;
    for o in ["http://evil.example", "http://localhost.evil.example", "null"] {
        assert!(ws(&host, 0, Some(o)).await.is_err(), "{o}");
    }
    for o in ["http://localhost:5173", "http://127.0.0.1:8787", "http://tauri.localhost"] {
        assert!(ws(&host, 0, Some(o)).await.is_ok(), "{o}");
    }
}

#[tokio::test]
async fn slow_client_is_disconnected() {
    let a = setup(Arc::new(MockProvider::default()));
    let host = serve(&a).await;
    let mut c = ws(&host, 0, None).await.unwrap(); // connected, then never read
    let big = "x".repeat(50_000);
    {
        let g = a.conn.lock().unwrap();
        for _ in 0..800 {
            emit(&g, &Ev { kind: "notice", level: "info", title: "t", message: &big, ..Default::default() }).unwrap();
        }
    }
    tokio::time::sleep(Duration::from_secs(8)).await; // > send timeout
    let mut got = 0;
    let ended = tokio::time::timeout(Duration::from_secs(20), async { while let Some(Ok(_)) = c.next().await { got += 1; } }).await;
    assert!(ended.is_ok() && got < 801, "server dropped the client instead of blocking (got {got})");
}

#[tokio::test]
async fn sse_since_and_notifications() {
    std::env::set_var("RESUME_PDF_ENGINE", "native");
    let a = setup(Arc::new(MockProvider::default()));
    let host = serve(&a).await;
    let id = run_job(&a).await;
    let total: i64 = a.conn.lock().unwrap().query_row("SELECT count(*) FROM job_events", [], |r| r.get(0)).unwrap();
    let mut r = reqwest::get(format!("http://{host}/api/events?since=3")).await.unwrap();
    let mut txt = String::new();
    while !txt.contains(&format!("id: {total}\n")) {
        txt += &String::from_utf8_lossy(&tokio::time::timeout(Duration::from_secs(5), r.chunk()).await.unwrap().unwrap().unwrap());
    }
    assert!(txt.starts_with("id: 4\n") && !txt.contains("id: 3\n"), "{txt}");
    assert!(txt.contains(r#""kind":"job_status""#));

    let get = |q: &str| reqwest::get(format!("http://{host}/api/notifications{q}"));
    let n: Vec<Value> = get("?unread=1").await.unwrap().json().await.unwrap();
    assert_eq!(n.len(), 1);
    assert_eq!((n[0]["job_id"].as_str(), n[0]["read"].as_bool(), n[0]["level"].as_str()), (Some(id.as_str()), Some(false), Some("success")));
    assert_eq!(n[0]["actions"][0]["action"], "open");
    a.notify("warn", "drive.notice", "Drive", "Drive needs attention");
    let seq = get("?unread=1").await.unwrap().json::<Vec<Value>>().await.unwrap()[0]["seq"].as_i64().unwrap();
    let res = reqwest::Client::new().post(format!("http://{host}/api/notifications/read")).json(&json!({"upto_seq": seq - 1})).send().await.unwrap();
    assert_eq!(res.status(), 204);
    let unread: Vec<Value> = get("?unread=1").await.unwrap().json().await.unwrap();
    assert_eq!((unread.len(), unread[0]["seq"].as_i64()), (1, Some(seq)));
    assert_eq!(get("").await.unwrap().json::<Vec<Value>>().await.unwrap().len(), 2);
}

#[test]
fn error_mapping() {
    let c = |step: &str, m: &str| AppError::classify(step, m);
    let l = c("build_payload", "payload leaks redacted values: Person:Jane Doe");
    assert_eq!((l.code.as_str(), l.retryable), ("redact.leak", false));
    assert!(!l.message.contains("Jane") && !serde_json::to_string(&l).unwrap().contains("Jane"));
    assert!(l.actions(Some("j")).as_array().unwrap().iter().any(|a| a["action"] == "open" && a["target"] == "/pii"));
    let cases = [("ai_tailor", "anthropic: HTTP 429", "ai.rate_limited"), ("ai_tailor", "gemini: HTTP 401", "ai.auth"), ("ai_tailor", "no API key configured for anthropic", "ai.auth"),
        ("ai_tailor", "payload ~9000 tokens exceeds job budget", "ai.budget_exceeded"), ("ai_tailor", "anthropic: network error (timeout)", "ai.timeout"), ("ai_tailor", "anthropic: no text in response", "ai.bad_reply"),
        ("restore", "reply is not valid JSON", "ai.bad_reply"), ("render_pdf", "tectonic failed: x", "render.tex_failed"), ("render_docx", "layout could not fit", "render.fit_failed"),
        ("upload_drive", "Google Drive is not connected: connect it in Settings", "drive.not_connected"), ("upload_drive", "Google Drive: HTTP 403", "drive.quota"),
        ("ai_tailor", "anthropic: network error (connect/request)", "net.offline"), ("finalize", "boom", "server.unreachable")];
    for (s, m, code) in cases {
        let e = c(s, m);
        assert_eq!(e.code, code, "{m}");
        assert!(!e.title.is_empty() && !e.hint.is_empty());
        let first = &e.actions(None)[0];
        match code {
            "ai.auth" | "drive.not_connected" => assert_eq!(first["action"], "settings"),
            _ => assert_eq!(first["action"], "retry"),
        }
    }
    for (e, code) in [(FetchErr::Unusable("login".into()), "fetch.login_wall"), (FetchErr::Upstream("the site answered HTTP 403".into()), "fetch.blocked"), (FetchErr::Upstream("could not connect".into()), "net.offline")] {
        assert_eq!(AppError::from_fetch(&e).code, code);
    }
    for code in ["grounding.reverted", "import.low_confidence", "server.unreachable"] {
        assert_eq!(AppError::new(code, "m", "s").code, code);
    }
}

#[tokio::test]
async fn failed_job_has_structured_error_and_notification() {
    std::env::set_var("RESUME_PDF_ENGINE", "native");
    let a = setup(Arc::new(MockProvider::with_failures(vec![ProviderError::Permanent("anthropic: HTTP 401".into())])));
    let host = serve(&a).await;
    let id = run_job(&a).await;
    let d: Value = reqwest::get(format!("http://{host}/api/jobs/{id}")).await.unwrap().json().await.unwrap();
    let e = &d["job"]["error"];
    assert_eq!((e["code"].as_str(), e["step"].as_str(), e["retryable"].as_bool()), (Some("ai.auth"), Some("ai_tailor"), Some(false)));
    let n: Vec<Value> = reqwest::get(format!("http://{host}/api/notifications")).await.unwrap().json().await.unwrap();
    assert_eq!((n.len(), n[0]["level"].as_str(), n[0]["job_id"].as_str()), (1, Some("error"), Some(id.as_str())));
    let acts: Vec<&str> = n[0]["actions"].as_array().unwrap().iter().map(|a| a["action"].as_str().unwrap()).collect();
    assert_eq!(acts, ["settings", "open"]);
    let evs = reqwest::get(format!("http://{host}/api/events?since=0")).await.unwrap();
    let mut evs = evs;
    let mut txt = String::new();
    while !txt.contains("job_status") {
        txt += &String::from_utf8_lossy(&tokio::time::timeout(Duration::from_secs(5), evs.chunk()).await.unwrap().unwrap().unwrap());
    }
    assert!(txt.contains(r#""code":"ai.auth""#) && txt.contains(r#""retryable":false"#));
    // old rows keep a plain string
    a.conn.lock().unwrap().execute("UPDATE jobs SET error='old failure' WHERE id=?", [&id]).unwrap();
    let d: Value = reqwest::get(format!("http://{host}/api/jobs/{id}")).await.unwrap().json().await.unwrap();
    assert_eq!(d["job"]["error"], "old failure");
}
