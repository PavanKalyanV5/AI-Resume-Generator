//! Realtime layer: job_events is the outbox (job_id NULL = system scope); WS /api/ws and SSE /api/events tail it by seq
//! cursor (replay and live are the same query, so no gaps or duplicates); notifications are the per-event "inbox".
use crate::{errors::AppError, now, App};
use axum::{extract::{ws::{Message, WebSocket, WebSocketUpgrade}, Query, State}, http::{header, HeaderMap, StatusCode}, response::{sse::{Event, KeepAlive, Sse}, IntoResponse, Response}, routing::{get, post}, Json, Router};
use rusqlite::{params, Connection};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::{collections::HashSet, convert::Infallible, sync::Arc, time::{Duration, Instant}};
use tokio::time::{interval, interval_at, timeout};

const POLL: Duration = Duration::from_millis(150); // ponytail: cursor poll; add a broadcast wake-up if latency/DB load matters
const PING: Duration = Duration::from_secs(20);
const DEAD: Duration = Duration::from_secs(60);
const SEND: Duration = Duration::from_secs(5); // a client that cannot take a frame in this long is dropped
const BATCH: i64 = 200;

pub fn migrate(c: &Connection) -> rusqlite::Result<()> {
    c.execute_batch("CREATE TABLE IF NOT EXISTS notifications(seq INTEGER PRIMARY KEY, read INTEGER NOT NULL DEFAULT 0);")?;
    // job_id was NOT NULL: rebuild once so system events can have none.
    if c.query_row("SELECT \"notnull\" FROM pragma_table_info('job_events') WHERE name='job_id'", [], |r| r.get::<_, bool>(0))? {
        c.execute_batch("BEGIN; ALTER TABLE job_events RENAME TO job_events_old;
            CREATE TABLE job_events(seq INTEGER PRIMARY KEY AUTOINCREMENT, job_id TEXT, ts INTEGER NOT NULL, step TEXT NOT NULL, level TEXT NOT NULL,
              message TEXT NOT NULL, local_only INTEGER NOT NULL, kind TEXT NOT NULL DEFAULT 'step', code TEXT, title TEXT, hint TEXT, retryable INTEGER, actions TEXT, done INTEGER, total INTEGER);
            INSERT INTO job_events(seq,job_id,ts,step,level,message,local_only) SELECT seq,job_id,ts,step,level,message,local_only FROM job_events_old;
            DROP TABLE job_events_old; COMMIT;")?;
    }
    Ok(())
}

#[derive(Default)]
pub struct Ev<'a> {
    pub job: Option<&'a str>,
    pub kind: &'a str,
    pub level: &'a str,
    pub step: &'a str,
    pub code: Option<&'a str>,
    pub title: &'a str,
    pub message: &'a str,
    pub hint: Option<&'a str>,
    pub retryable: Option<bool>,
    pub actions: Option<Value>,
    pub local: bool,
    /// Also becomes a notification.
    pub notify: bool,
}

pub fn emit(c: &Connection, e: &Ev) -> rusqlite::Result<i64> {
    c.execute("INSERT INTO job_events(job_id,ts,step,level,message,local_only,kind,code,title,hint,retryable,actions) VALUES(?,?,?,?,?,?,?,?,?,?,?,?)",
        params![e.job, now(), e.step, e.level, e.message, e.local, e.kind, e.code, e.title, e.hint, e.retryable, e.actions.as_ref().map(Value::to_string)])?;
    let seq = c.last_insert_rowid();
    if e.notify {
        c.execute("INSERT INTO notifications(seq) VALUES(?)", [seq])?;
    }
    Ok(seq)
}

/// Event carrying a structured error (step failure, grounding revert, fit warning).
pub fn emit_err(c: &Connection, job: &str, kind: &str, level: &str, e: &AppError, message: &str, local: bool, notify: bool) -> rusqlite::Result<i64> {
    emit(c, &Ev { job: Some(job), kind, level, step: &e.step, code: Some(&e.code), title: &e.title, message, hint: Some(&e.hint), retryable: Some(e.retryable), actions: Some(e.actions(Some(job))), local, notify })
}

/// done|failed|dead|cancelled: one job_status event + one notification with an open action for /jobs/<id>.
pub fn job_terminal(c: &Connection, job: &str, status: &str, err: Option<&AppError>) -> rusqlite::Result<()> {
    if let Some(e) = err {
        emit_err(c, job, "job_status", "error", e, &e.message, true, true)?;
        return Ok(());
    }
    let (level, title) = if status == "done" { ("success", "Resume ready") } else { ("warn", "Job cancelled") };
    emit(c, &Ev { job: Some(job), kind: "job_status", level, step: status, title, message: if status == "done" { "Your tailored resume is ready." } else { "The job was cancelled." },
        actions: Some(json!([{"label": "Open job", "action": "open", "target": format!("/jobs/{job}")}])), local: true, notify: true, ..Default::default() })?;
    Ok(())
}

impl App {
    /// System notice (model download, drive, ai...) that is also a notification.
    pub fn notify(&self, level: &str, code: &str, title: &str, message: &str) {
        let _ = emit(&self.conn.lock().unwrap(), &Ev { kind: "notice", level, code: Some(code), title, message, local: true, notify: true, ..Default::default() });
    }
}

fn obj(p: Vec<(&str, Value)>) -> Value {
    Value::Object(p.into_iter().filter(|(_, v)| !v.is_null()).map(|(k, v)| (k.to_string(), v)).collect::<Map<_, _>>())
}

pub fn head(c: &Connection) -> rusqlite::Result<i64> {
    c.query_row("SELECT coalesce(max(seq),0) FROM job_events", [], |r| r.get(0))
}

/// Event frames with seq > since, in order.
pub fn events_since(c: &Connection, since: i64, limit: i64) -> rusqlite::Result<Vec<Value>> {
    let mut s = c.prepare("SELECT seq,job_id,ts,step,level,message,local_only,kind,code,coalesce(title,step),hint,retryable,actions,done,total FROM job_events WHERE seq>? ORDER BY seq LIMIT ?")?;
    let r = s.query_map(params![since, limit], |r| {
        let job: Option<String> = r.get(1)?;
        let step: String = r.get(3)?;
        let actions: Option<String> = r.get(12)?;
        let (done, total): (Option<i64>, Option<i64>) = (r.get(13)?, r.get(14)?);
        Ok(obj(vec![("t", json!("event")), ("seq", json!(r.get::<_, i64>(0)?)), ("scope", json!(if job.is_some() { "job" } else { "system" })), ("job_id", json!(job)), ("kind", json!(r.get::<_, String>(7)?)),
            ("level", json!(r.get::<_, String>(4)?)), ("code", json!(r.get::<_, Option<String>>(8)?)), ("title", json!(r.get::<_, String>(9)?)), ("message", json!(r.get::<_, String>(5)?)), ("hint", json!(r.get::<_, Option<String>>(10)?)),
            ("retryable", json!(r.get::<_, Option<bool>>(11)?)), ("actions", actions.and_then(|a| serde_json::from_str(&a).ok()).unwrap_or(Value::Null)), ("step", json!(Some(step).filter(|s| !s.is_empty()))),
            ("local_only", json!(r.get::<_, bool>(6)?)), ("progress", match (done, total) { (Some(d), Some(t)) => json!({"done": d, "total": t}), _ => Value::Null }), ("ts", json!(r.get::<_, i64>(2)?))]))
    })?;
    r.collect()
}

fn origin_ok(h: &HeaderMap) -> bool {
    let Some(o) = h.get(header::ORIGIN) else { return true }; // non-browser clients send no Origin
    let o = o.to_str().unwrap_or("");
    matches!(o, "tauri://localhost" | "http://tauri.localhost" | "https://tauri.localhost")
        || ["http://", "https://"].iter().any(|p| o.strip_prefix(p).is_some_and(|r| matches!(r.split(':').next(), Some("localhost" | "127.0.0.1"))))
}

#[derive(Deserialize)]
struct Since {
    since: Option<i64>,
}

async fn ws(State(a): State<Arc<App>>, h: HeaderMap, au: Option<axum::Extension<crate::remote::Authed>>, Query(q): Query<Since>, up: WebSocketUpgrade) -> Response {
    // the remote gate already checked the token, so a remote Origin is fine there; locally keep the allow-list
    if au.is_none() && !origin_ok(&h) {
        return (StatusCode::FORBIDDEN, "origin not allowed").into_response();
    }
    // echo the `bearer.<token>` subprotocol the browser offered, or it drops the socket
    let protos: Vec<String> = h.get_all(header::SEC_WEBSOCKET_PROTOCOL).iter().filter_map(|v| v.to_str().ok()).flat_map(|v| v.split(',')).map(|p| p.trim().to_string()).filter(|p| p.starts_with("bearer.")).collect();
    up.protocols(protos).on_upgrade(move |s| serve(a, s, q.since, au.map(|e| e.0.cancel.clone())))
}

async fn send(s: &mut WebSocket, v: &Value) -> bool {
    matches!(timeout(SEND, s.send(Message::Text(v.to_string()))).await, Ok(Ok(())))
}

async fn serve(a: Arc<App>, mut s: WebSocket, since: Option<i64>, revoked: Option<tokio_util::sync::CancellationToken>) {
    let Ok(head) = a.db(|c| Ok(head(c)?)).await else { return };
    let mut cur = since.unwrap_or(head);
    if !send(&mut s, &json!({"t": "hello", "seq_head": head})).await {
        return;
    }
    let (mut poll, mut ping, mut last_pong) = (interval(POLL), interval_at(tokio::time::Instant::now() + PING, PING), Instant::now());
    let mut sub: Option<HashSet<String>> = None;
    loop {
        tokio::select! {
            _ = async { match &revoked { Some(c) => c.cancelled().await, None => std::future::pending().await } } => break,
            m = s.recv() => match m {
                Some(Ok(Message::Text(t))) => {
                    let v: Value = serde_json::from_str(&t).unwrap_or(Value::Null);
                    match v["t"].as_str() {
                        Some("pong") => last_pong = Instant::now(),
                        Some("sub") => sub = v["job_ids"].as_array().map(|a| a.iter().filter_map(|j| j.as_str().map(String::from)).collect()),
                        _ => {} // ack: the cursor is server-side, nothing to persist
                    }
                }
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                _ => {}
            },
            _ = poll.tick() => {
                let Ok(evs) = a.db(move |c| Ok(events_since(c, cur, BATCH)?)).await else { break };
                for e in evs {
                    cur = e["seq"].as_i64().unwrap_or(cur);
                    if sub.as_ref().is_some_and(|s| e["job_id"].as_str().is_some_and(|j| !s.contains(j))) { continue; }
                    if !send(&mut s, &e).await { return; }
                }
            }
            _ = ping.tick() => {
                if last_pong.elapsed() > DEAD || !send(&mut s, &json!({"t": "ping"})).await { break; }
            }
        }
    }
}

async fn sse(State(a): State<Arc<App>>, h: HeaderMap, Query(q): Query<Since>) -> Sse<impl futures_core::Stream<Item = Result<Event, Infallible>>> {
    let last = h.get("last-event-id").and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok());
    let s = async_stream::stream! {
        let Ok(mut cur) = a.db(|c| Ok(head(c)?)).await.map(|hd| q.since.or(last).unwrap_or(hd)) else { return };
        loop {
            let Ok(evs) = a.db(move |c| Ok(events_since(c, cur, BATCH)?)).await else { break };
            for e in evs {
                cur = e["seq"].as_i64().unwrap_or(cur);
                yield Ok(Event::default().id(cur.to_string()).data(e.to_string()));
            }
            tokio::time::sleep(POLL).await;
        }
    };
    Sse::new(s).keep_alive(KeepAlive::default())
}

async fn notifications(State(a): State<Arc<App>>, Query(q): Query<std::collections::HashMap<String, String>>) -> Result<Json<Value>, (StatusCode, String)> {
    let unread = q.get("unread").is_some_and(|v| v == "1" || v == "true");
    let v = a.db(move |c| {
        let mut s = c.prepare("SELECT e.seq,e.level,coalesce(e.title,e.step),e.message,e.hint,e.job_id,e.actions,e.ts,n.read FROM notifications n JOIN job_events e ON e.seq=n.seq WHERE ?1=0 OR n.read=0 ORDER BY e.seq DESC")?;
        let r = s.query_map([unread], |r| Ok(obj(vec![("seq", json!(r.get::<_, i64>(0)?)), ("level", json!(r.get::<_, String>(1)?)), ("title", json!(r.get::<_, String>(2)?)), ("message", json!(r.get::<_, String>(3)?)),
            ("hint", json!(r.get::<_, Option<String>>(4)?)), ("job_id", json!(r.get::<_, Option<String>>(5)?)), ("actions", r.get::<_, Option<String>>(6)?.and_then(|a| serde_json::from_str(&a).ok()).unwrap_or(Value::Null)),
            ("ts", json!(r.get::<_, i64>(7)?)), ("read", json!(r.get::<_, bool>(8)?))])))?;
        Ok(r.collect::<rusqlite::Result<Vec<_>>>()?)
    }).await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(json!(v)))
}

async fn mark_read(State(a): State<Arc<App>>, Json(b): Json<Value>) -> Result<StatusCode, (StatusCode, String)> {
    let upto = b["upto_seq"].as_i64().ok_or((StatusCode::BAD_REQUEST, "upto_seq required".to_string()))?;
    a.db(move |c| Ok(c.execute("UPDATE notifications SET read=1 WHERE seq<=?", [upto])?)).await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

pub fn routes() -> Router<Arc<App>> {
    Router::new().route("/api/ws", get(ws)).route("/api/events", get(sse)).route("/api/notifications", get(notifications)).route("/api/notifications/read", post(mark_read))
}
