use crate::{create_job_pages, display_name, events_after, full_sel, retry_job, cancel_job, restored, sel_json, DetailEnv, App, STEPS};
use anyhow::Result;
use axum::{extract::{Path, Query, State}, http::{header, HeaderMap, HeaderValue, Method, StatusCode}, response::{sse::{Event, KeepAlive, Sse}, IntoResponse}, routing::{get, post, put}, Json, Router};
use resume_core::{cluster::{cluster, near_duplicates}, graph::{build_graph_with, gaps, NodeKind}, schema::Resume, select::{ats_coverage_with, Coverage}};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{collections::HashMap, convert::Infallible, sync::Arc, time::Duration};
use tower_http::cors::CorsLayer;

type E = (StatusCode, String);
type S = State<Arc<App>>;
fn ie(e: impl std::fmt::Display) -> E {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}
fn bad(s: &str) -> E {
    (StatusCode::BAD_REQUEST, s.into())
}

pub fn router(app: Arc<App>) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(["http://localhost:5173".parse::<HeaderValue>().unwrap(), "http://127.0.0.1:5173".parse().unwrap()])
        .allow_methods([Method::GET, Method::PUT, Method::POST, Method::DELETE])
        .allow_headers([header::CONTENT_TYPE, "last-event-id".parse().unwrap(), "x-filename".parse().unwrap(), "x-confirm".parse().unwrap()]);
    api_routes().merge(crate::remote::admin_routes()).layer(cors).with_state(app)
}

/// Every API route except the remote admin ones, so the remote listener can reuse them.
pub fn api_routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/health", get(|| async { "ok" }))
        .route("/api/resume", get(get_resume).put(put_resume))
        .route("/api/jobs", post(new_job).get(list_jobs))
        .route("/api/jobs/:id", get(get_job))
        .route("/api/jobs/:id/retry", post(retry))
        .route("/api/jobs/:id/cancel", post(cancel))
        .route("/api/jobs/:id/events", get(events))
        .route("/api/jobs/:id/files/:name", get(file))
        .route("/api/library", get(library))
        .route("/api/jd/fetch", post(jd_fetch))
        .route("/api/keys", get(get_keys))
        .route("/api/keys/:provider", put(put_key).delete(del_key))
        .route("/api/stats", get(stats))
        .route("/api/import", post(import).layer(axum::extract::DefaultBodyLimit::max(10 * 1024 * 1024)))
        .route("/api/settings", get(get_settings).put(put_settings))
        .route("/api/profile/facts", get(profile_facts))
        .merge(crate::realtime::routes())
        .merge(crate::drive::routes())
        .merge(crate::pii::routes())
        .merge(crate::mlsvc::routes())
        .merge(crate::review::routes())
        .merge(crate::learn::routes())
        .merge(crate::heur::routes())
}

async fn get_resume(State(a): S) -> Result<Json<Value>, E> {
    let r = a.db(|c| Ok(c.query_row("SELECT json FROM resume WHERE id=1", [], |r| r.get::<_, String>(0)).optional()?)).await.map_err(ie)?;
    let r = r.ok_or((StatusCode::NOT_FOUND, "no resume".to_string()))?;
    Ok(Json(serde_json::from_str(&r).map_err(ie)?))
}

async fn put_resume(State(a): S, Json(r): Json<Resume>) -> Result<StatusCode, E> {
    let s = serde_json::to_string(&r).map_err(ie)?;
    a.db(move |c| Ok(c.execute("INSERT INTO resume(id,json) VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET json=?1", [s])?)).await.map_err(ie)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct NewJob {
    jd_text: String,
    provider: String,
    #[serde(default)]
    extra_redact: Vec<String>,
    max_tokens: Option<i64>,
    company: Option<String>,
    role: Option<String>,
    #[serde(default)]
    upload_drive: bool,
    /// 1 or 2: overrides the automatic page decision.
    pages: Option<u8>,
    /// AI plan review; default on for real providers, off for mock.
    ai_review: Option<bool>,
}

async fn new_job(State(a): S, Json(n): Json<NewJob>) -> Result<Json<Value>, E> {
    if n.pages.is_some_and(|p| !(1..=2).contains(&p)) {
        return Err(bad("pages must be 1 or 2"));
    }
    let id = a.db(move |c| {
        if c.query_row("SELECT 1 FROM resume WHERE id=1", [], |_| Ok(())).optional()?.is_none() {
            anyhow::bail!("upload a resume first");
        }
        let id = create_job_pages(c, &n.jd_text, &n.provider, &n.extra_redact, n.max_tokens, n.company.as_deref(), n.role.as_deref(), n.upload_drive, n.pages)?;
        if let Some(r) = n.ai_review {
            c.execute("UPDATE jobs SET ai_review=? WHERE id=?", params![r, id])?;
        }
        Ok(id)
    }).await.map_err(|e| bad(&e.to_string()))?;
    Ok(Json(json!({"id": id})))
}

async fn list_jobs(State(a): S) -> Result<Json<Value>, E> {
    let v = a.db(|c| {
        let mut s = c.prepare("SELECT id,status,provider,created_at,updated_at,error,company,role FROM jobs ORDER BY created_at DESC, rowid DESC")?;
        let r = s.query_map([], |r| Ok(json!({"id": r.get::<_, String>(0)?, "status": r.get::<_, String>(1)?, "provider": r.get::<_, String>(2)?, "created_at": r.get::<_, i64>(3)?, "updated_at": r.get::<_, i64>(4)?, "error": crate::errors::err_json(r.get::<_, Option<String>>(5)?), "company": r.get::<_, Option<String>>(6)?, "role": r.get::<_, Option<String>>(7)?})))?;
        Ok(r.collect::<rusqlite::Result<Vec<_>>>()?)
    }).await.map_err(ie)?;
    Ok(Json(json!(v)))
}

fn cov(c: &Coverage) -> Value {
    json!({"score": c.score, "covered": c.covered, "missing": c.missing, "semantic": c.semantic})
}

/// Without an embedder/heuristics/key file (they default); `job_detail_env` is what the API uses.
pub fn job_detail(c: &Connection, id: &str) -> Result<Option<Value>> {
    job_detail_env(c, id, &DetailEnv::default())
}

fn clip(s: &str) -> String {
    let t: String = s.chars().take(60).collect();
    if t.len() < s.len() { format!("{t}...") } else { t }
}

pub fn job_detail_env(c: &Connection, id: &str, env: &DetailEnv) -> Result<Option<Value>> {
    let job = c.query_row("SELECT id,status,provider,jd_text,extra_redact_json,max_tokens,created_at,updated_at,error,coalesce(company,'Unknown'),coalesce(role,'Role'),upload_drive FROM jobs WHERE id=?", [id], |r| {
        Ok((json!({"id": r.get::<_, String>(0)?, "status": r.get::<_, String>(1)?, "provider": r.get::<_, String>(2)?, "jd_text": r.get::<_, String>(3)?, "max_tokens": r.get::<_, Option<i64>>(5)?, "created_at": r.get::<_, i64>(6)?, "updated_at": r.get::<_, i64>(7)?, "error": crate::errors::err_json(r.get::<_, Option<String>>(8)?), "company": r.get::<_, String>(9)?, "role": r.get::<_, String>(10)?, "upload_drive": r.get::<_, bool>(11)?}), r.get::<_, String>(3)?, r.get::<_, String>(4)?))
    }).optional()?;
    let Some((job, jd_text, extra)) = job else { return Ok(None) };
    let mut steps = vec![];
    let mut outs: HashMap<String, Value> = HashMap::new();
    let mut s = c.prepare("SELECT name,status,attempts,error,started_at,finished_at,output_json FROM job_steps WHERE job_id=?")?;
    let rows: Vec<_> = s.query_map([id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?, r.get::<_, Option<String>>(3)?, r.get::<_, Option<i64>>(4)?, r.get::<_, Option<i64>>(5)?, r.get::<_, Option<String>>(6)?)))?.collect::<rusqlite::Result<_>>()?;
    for n in STEPS {
        if let Some((_, st, at, er, sa, fa, out)) = rows.iter().find(|r| r.0 == n) {
            steps.push(json!({"name": n, "status": st, "attempts": at, "error": er, "started_at": sa, "finished_at": fa}));
            if st == "done" {
                outs.insert(n.into(), serde_json::from_str(out.as_deref().unwrap_or("null"))?);
            }
        }
    }
    let files: Vec<&str> = [("render_docx", "resume.docx"), ("render_pdf", "resume.pdf")].iter().filter(|(s, _)| outs.contains_key(*s)).map(|x| x.1).collect();
    let (company, role) = (job["company"].as_str().unwrap_or("Unknown").to_string(), job["role"].as_str().unwrap_or("Role").to_string());
    let display_names: serde_json::Map<String, Value> = files.iter().map(|f| (f.to_string(), json!(display_name(&company, &role, f.rsplit('.').next().unwrap())))).collect();
    let drive = drive_links(outs.get("upload_drive"));
    let audit = outs.get("build_payload").map(|p| json!({"redacted_payload": {"system": p["system"], "user": p["user"]}, "token_counts": p["token_counts"]}));
    let resume: Option<String> = c.query_row("SELECT json FROM resume WHERE id=1", [], |r| r.get(0)).optional()?;
    let (mut coverage, mut gp, mut graph, mut original, mut tailored, mut selection, mut after) = (Value::Null, Value::Null, Value::Null, Value::Null, Value::Null, Value::Null, Value::Null);
    let (mut clusters, mut near, mut facts_summary) = (Value::Null, json!([]), Value::Null);
    let (mut pool, mut review, mut arrangement) = (Value::Null, Value::Null, Value::Null);
    if let Some(rj) = resume {
        let extra: Vec<String> = serde_json::from_str(&extra)?;
        let mut cx = env.ctx(c, id, &rj, &jd_text, &extra, outs.get("select"))?;
        if let Some(o) = outs.get("ai_review") {
            crate::apply_select(&mut cx, o);
        }
        (pool, review) = crate::review::detail(c, id, &cx, &outs)?;
        let emb = cx.emb.as_deref();
        let g = build_graph_with(&cx.resume, Some(&cx.jd), emb);
        gp = json!(gaps(&g));
        if let Some(e) = emb {
            let items: Vec<(String, String, String)> = g.nodes.iter().filter(|n| matches!(n.kind, NodeKind::Bullet | NodeKind::Skill)).map(|n| (n.id.clone(), clip(&n.label), n.label.clone())).collect();
            clusters = json!(cluster(&items, e));
            let bullets: Vec<_> = items.iter().filter(|i| i.0.starts_with("bullet:")).cloned().collect();
            near = json!(near_duplicates(&bullets, e, 0.92).into_iter().map(|(a, b, s)| json!({"a": a, "b": b, "similarity": s})).collect::<Vec<_>>());
        }
        graph = json!(g);
        coverage = cov(&ats_coverage_with(&cx.resume, &cx.sel, &cx.jd, emb));
        original = json!(cx.sel.apply(&cx.resume));
        selection = sel_json(&cx.sel);
        facts_summary = json!({"years": cx.facts.years.total_effective, "top_skills": cx.facts.skills.iter().take(8).map(|s| json!({"canonical": s.canonical, "level": s.level, "months_used": s.months_used})).collect::<Vec<_>>()});
        if outs.contains_key("restore") {
            let t = restored(&cx, &outs.iter().map(|(k, v)| (k.as_str(), v.clone())).collect()).map_err(|e| anyhow::anyhow!("{e:?}"))?;
            after = cov(&ats_coverage_with(&t, &full_sel(&t), &cx.jd, emb));
            let a = resume_core::arrange::arrange_resume(&t, Some(&cx.jd), Some(&cx.facts));
            arrangement = json!({"reasons": a.reasons, "skill_rows_order": a.skill_rows_order.iter().map(|&i| &t.skills[i].label).collect::<Vec<_>>()});
            tailored = json!(t);
        }
    }
    let (corrections, rules_applied, approved) = crate::learn::detail(c, id, &outs)?;
    let decisions = outs.get("select").map(|s| s["decisions"].clone()).unwrap_or(Value::Null);
    let fit = outs.get("render_pdf").or(outs.get("render_docx")).map(|r| r["fit"].clone()).unwrap_or(Value::Null);
    let grounding = outs.get("restore").map(|r| r["grounding"].clone()).unwrap_or(Value::Null);
    let redaction_summary = outs.get("build_payload").map(|p| json!({"counts": p["token_counts"], "total": p["token_counts"].as_object().map_or(0, |m| m.values().filter_map(Value::as_u64).sum::<u64>())})).unwrap_or(Value::Null);
    Ok(Some(json!({"decisions": decisions, "fit": fit, "grounding": grounding, "clusters": clusters, "near_duplicates": near, "facts_summary": facts_summary, "redaction_summary": redaction_summary, "job": job, "steps": steps, "coverage": coverage, "coverage_after": after, "gaps": gp, "graph": graph, "selection": selection, "original": original, "tailored": tailored, "arrangement": arrangement, "audit": audit, "pool": pool, "review": review, "files": files, "company": company, "role": role, "display_names": display_names, "drive": drive, "corrections": corrections, "rules_applied": rules_applied, "approved": approved})))
}

fn drive_links(o: Option<&Value>) -> Value {
    match o.and_then(Value::as_array) {
        Some(a) => json!(a.iter().map(|f| json!({"name": f["name"], "url": f["url"]})).collect::<Vec<_>>()),
        None => Value::Null,
    }
}

/// Finished jobs, newest first.
async fn library(State(a): S) -> Result<Json<Value>, E> {
    let v = a.db(|c| {
        let mut s = c.prepare("SELECT id,coalesce(company,'Unknown'),coalesce(role,'Role'),created_at,
            (SELECT json_extract(output_json,'$.coverage') FROM job_steps WHERE job_id=jobs.id AND name='select'),
            (SELECT output_json FROM job_steps WHERE job_id=jobs.id AND name='upload_drive' AND status='done'),
            (SELECT group_concat(name) FROM job_steps WHERE job_id=jobs.id AND status='done' AND name IN('render_docx','render_pdf'))
            FROM jobs WHERE status='done' ORDER BY created_at DESC, rowid DESC")?;
        let r = s.query_map([], |r| {
            let (company, role) = (r.get::<_, String>(1)?, r.get::<_, String>(2)?);
            let have = r.get::<_, Option<String>>(6)?.unwrap_or_default();
            let files: Vec<Value> = [("render_pdf", "resume.pdf"), ("render_docx", "resume.docx")].iter().filter(|(s, _)| have.contains(s)).map(|(_, n)| json!({"name": n, "display_name": display_name(&company, &role, n.rsplit('.').next().unwrap())})).collect();
            let drive = r.get::<_, Option<String>>(5)?.and_then(|o| serde_json::from_str::<Value>(&o).ok());
            Ok(json!({"id": r.get::<_, String>(0)?, "company": company, "role": role, "created_at": r.get::<_, i64>(3)?, "files": files, "drive": drive_links(drive.as_ref()), "coverage": r.get::<_, Option<f64>>(4)?}))
        })?;
        Ok(r.collect::<rusqlite::Result<Vec<_>>>()?)
    }).await.map_err(ie)?;
    Ok(Json(json!(v)))
}

async fn jd_fetch(Json(b): Json<Value>) -> Result<Json<Value>, E> {
    use crate::fetch::{fetch_jd, public_only, FetchErr};
    let url = b["url"].as_str().map(str::trim).filter(|u| !u.is_empty()).ok_or_else(|| bad("url required"))?;
    let allow = std::env::var("RESUME_ALLOW_PRIVATE_FETCH").is_ok_and(|v| v == "1"); // tests only
    let r = if allow { fetch_jd(url, &|_, _| true).await } else { fetch_jd(url, &public_only).await };
    match r {
        Ok(f) => Ok(Json(f.json())),
        Err(FetchErr::Bad(m)) => Err(bad(&m)),
        Err(FetchErr::Unusable(m)) => Err((StatusCode::UNPROCESSABLE_ENTITY, m)),
        Err(FetchErr::Upstream(m)) => Err((StatusCode::BAD_GATEWAY, format!("Could not fetch that page: {m}"))),
    }
}

async fn get_job(State(a): S, Path(id): Path<String>) -> Result<Json<Value>, E> {
    let env = DetailEnv::of(&a);
    let mut v = a.db(move |c| job_detail_env(c, &id, &env)).await.map_err(ie)?;
    if let Some(v) = v.as_mut() {
        crate::mlsvc::extend_detail(&a, v).await.map_err(ie)?;
    }
    v.map(Json).ok_or((StatusCode::NOT_FOUND, "no such job".into()))
}

#[derive(Deserialize)]
struct From {
    from: Option<String>,
}

async fn retry(State(a): S, Path(id): Path<String>, Query(q): Query<From>) -> Result<StatusCode, E> {
    a.db(move |c| retry_job(c, &id, q.from.as_deref().unwrap_or("parse_jd"))).await.map_err(|e| bad(&e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn cancel(State(a): S, Path(id): Path<String>) -> Result<StatusCode, E> {
    let ok = a.db(move |c| cancel_job(c, &id)).await.map_err(ie)?;
    if ok { Ok(StatusCode::NO_CONTENT) } else { Err((StatusCode::CONFLICT, "job not cancellable".into())) }
}

async fn events(State(a): S, Path(id): Path<String>, h: HeaderMap) -> Sse<impl futures_core::Stream<Item = Result<Event, Infallible>>> {
    let mut last = h.get("last-event-id").and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<i64>().ok()).unwrap_or(0);
    let s = async_stream::stream! {
        loop {
            let i = id.clone();
            let Ok((term, evs)) = a.db(move |c| {
                let st: Option<String> = c.query_row("SELECT status FROM jobs WHERE id=?", [&i], |r| r.get(0)).optional()?;
                Ok((st.map_or(true, |s| !matches!(s.as_str(), "queued" | "running")), events_after(c, &i, last)?))
            }).await else { break };
            for e in &evs {
                last = e["seq"].as_i64().unwrap_or(last);
                yield Ok(Event::default().id(last.to_string()).data(e.to_string()));
            }
            if term && evs.is_empty() { break; }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    };
    Sse::new(s).keep_alive(KeepAlive::default())
}

async fn file(State(a): S, Path((id, name)): Path<(String, String)>) -> Result<impl IntoResponse, E> {
    let ct = match name.as_str() {
        "resume.pdf" => "application/pdf",
        "resume.docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        _ => return Err((StatusCode::NOT_FOUND, "no such file".into())),
    };
    if !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(bad("bad id"));
    }
    let b = tokio::fs::read(a.out.join(&id).join(&name)).await.map_err(|_| (StatusCode::NOT_FOUND, "no such file".to_string()))?;
    let jid = id.clone();
    let (company, role) = a.db(move |c| Ok(c.query_row("SELECT coalesce(company,'Unknown'),coalesce(role,'Role') FROM jobs WHERE id=?", [jid], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))).optional()?)).await.map_err(ie)?.unwrap_or(("Unknown".into(), "Role".into()));
    let dn = display_name(&company, &role, name.rsplit('.').next().unwrap());
    let ascii: String = dn.chars().map(|c| if c.is_ascii() && c != '"' && c != '\\' && c != '%' { c } else { '_' }).collect();
    let enc: String = dn.bytes().map(|b| if b.is_ascii_alphanumeric() || b"!#$&+-.^_`|~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect();
    Ok(([(header::CONTENT_TYPE, ct.to_string()), (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{ascii}\"; filename*=UTF-8''{enc}"))], b))
}

async fn put_key(State(a): S, Path(p): Path<String>, Json(b): Json<Value>) -> Result<StatusCode, E> {
    let key_file = a.key_file.clone();
    let k = b["key"].as_str().filter(|k| !k.is_empty()).ok_or_else(|| bad("key required"))?.to_string();
    if !matches!(p.as_str(), "anthropic" | "gemini") {
        return Err(bad("unknown provider"));
    }
    a.db(move |c| {
        let blob = crate::keys::encrypt(&key_file, &p, &k)?;
        Ok(c.execute("INSERT OR REPLACE INTO keys VALUES(?,?)", params![p, blob])?)
    }).await.map_err(ie)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn del_key(State(a): S, Path(p): Path<String>) -> Result<StatusCode, E> {
    a.db(move |c| Ok(c.execute("DELETE FROM keys WHERE provider=?", [p])?)).await.map_err(ie)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn get_keys(State(a): S) -> Result<Json<Value>, E> {
    let stored = a.db(|c| Ok(c.prepare("SELECT provider FROM keys")?.query_map([], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?)).await.map_err(ie)?;
    Ok(Json(["anthropic", "gemini"].iter().map(|p| {
        let source = if stored.iter().any(|s| s == p) { Some("stored") } else if crate::keys::env_key(p).is_some() { Some("env") } else { None };
        (p.to_string(), json!({"configured": source.is_some(), "source": source}))
    }).collect::<serde_json::Map<_, _>>().into()))
}

async fn stats(State(a): S) -> Result<Json<Value>, E> {
    let v = a.db(|c| {
        let q = |sql: &str| -> Result<Vec<Value>> { Ok(c.prepare(sql)?.query_map([], |r| Ok(json!([r.get::<_, String>(0)?, r.get::<_, i64>(1)?])))?.collect::<rusqlite::Result<_>>()?) };
        let pairs = |v: Vec<Value>| -> Value { v.into_iter().map(|p| (p[0].as_str().unwrap().to_string(), p[1].clone())).collect::<serde_json::Map<_, _>>().into() };
        let by_day = pairs(q("SELECT date(created_at/1000,'unixepoch'),count(*) FROM jobs GROUP BY 1 ORDER BY 1")?);
        let by_status = pairs(q("SELECT status,count(*) FROM jobs GROUP BY 1")?);
        let (ti, to): (i64, i64) = c.query_row("SELECT coalesce(sum(json_extract(output_json,'$.tokens_in')),0),coalesce(sum(json_extract(output_json,'$.tokens_out')),0) FROM job_steps WHERE name='ai_tailor' AND status='done' AND json_extract(output_json,'$.cached')=0", [], |r| Ok((r.get(0)?, r.get(1)?)))?;
        let avg: Option<f64> = c.query_row("SELECT avg(json_extract(output_json,'$.coverage')) FROM job_steps WHERE name='select' AND status='done'", [], |r| r.get(0))?;
        Ok(json!({"jobs_by_day": by_day, "tokens_in": ti, "tokens_out": to, "avg_coverage": avg, "by_status": by_status}))
    }).await.map_err(ie)?;
    Ok(Json(v))
}

/// Parse an uploaded resume (.docx/.pdf/.json); does not touch the stored resume.
/// A PDF's page count is remembered as `prior_resume_pages` (a page-length signal for later jobs).
async fn import(State(a): S, h: HeaderMap, body: axum::body::Bytes) -> Result<Json<resume_core::import::ImportResult>, E> {
    let name = h.get("x-filename").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let (r, pages) = tokio::task::spawn_blocking(move || {
        let r = resume_core::import::import_resume(&body, &name)?;
        let pdf = name.to_lowercase().ends_with(".pdf") || body.starts_with(b"%PDF");
        Ok::<_, anyhow::Error>((r, pdf.then(|| lopdf::Document::load_mem(&body).ok().map(|d| d.get_pages().len())).flatten()))
    })
    .await
    .map_err(ie)?
    .map_err(|e| bad(&format!("{e:#}")))?;
    if let Some(n) = pages.filter(|n| (1..=9).contains(n)) {
        a.set_setting("prior_resume_pages", Some(n.to_string())).await.map_err(ie)?;
    }
    Ok(Json(r))
}

async fn get_settings(State(a): S) -> Json<Value> {
    Json(json!({"prior_resume_pages": a.setting("prior_resume_pages").await.and_then(|v| v.parse::<u8>().ok())}))
}

async fn put_settings(State(a): S, Json(b): Json<Value>) -> Result<StatusCode, E> {
    if let Some(v) = b.get("prior_resume_pages") {
        let n = match v {
            Value::Null => None,
            v => Some(v.as_u64().filter(|n| (1..=9).contains(n)).ok_or_else(|| bad("prior_resume_pages must be 1-9 or null"))?),
        };
        a.set_setting("prior_resume_pages", n.map(|n| n.to_string())).await.map_err(ie)?;
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Localhost-only data derived from the stored resume; never sent to a provider.
async fn profile_facts(State(a): S) -> Result<impl IntoResponse, E> {
    use resume_core::profile_model::{build_facts, skill_matrix, skill_timeline};
    let r = a.db(|c| Ok(c.query_row("SELECT json FROM resume WHERE id=1", [], |r| r.get::<_, String>(0)).optional()?)).await.map_err(ie)?;
    let r: Resume = serde_json::from_str(&r.ok_or((StatusCode::NOT_FOUND, "no resume".to_string()))?).map_err(ie)?;
    let m = resume_core::seniority::now_month();
    let f = build_facts(&r, (m.div_euclid(12), m.rem_euclid(12) as u32 + 1));
    let mut v = json!(f);
    v["skill_timeline"] = json!(skill_timeline(&f));
    v["skill_matrix"] = json!(skill_matrix(&f).into_iter().map(|(skill, proficiency)| json!({"skill": skill, "proficiency": proficiency})).collect::<Vec<_>>());
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(v)))
}
