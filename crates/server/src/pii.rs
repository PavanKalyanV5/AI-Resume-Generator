//! PII inspector API. Real values live only in the vault/always table (encrypted); every response except
//! /reveal carries masked text and a salted, non-reversible id. Nothing here logs a value.
use crate::{always_encrypt, always_rows, kind_from, App};
use anyhow::Result;
use axum::{extract::{Path, State}, http::{header, HeaderMap, HeaderValue, StatusCode}, middleware, response::Response, routing::{delete, get, post}, Json, Router};
use regex::Regex;
use resume_core::{redact::{Kind, Vault}, schema::Resume};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::{BTreeMap, BTreeSet, HashMap, HashSet}, sync::Arc};

type E = (StatusCode, String);
type S = State<Arc<App>>;
const AREAS: [&str; 6] = ["experience", "projects", "education", "skills", "certifications", "summary"];

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/pii", get(list))
        .route("/api/pii/reveal", post(reveal))
        .route("/api/pii/always", post(add_always))
        .route("/api/pii/always/:id", delete(del_always))
        .route("/api/pii/candidates/:id/accept", post(accept))
        .route("/api/pii/candidates/:id/dismiss", post(dismiss))
        .layer(middleware::map_response(|mut r: Response| async move {
            r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            r
        }))
}

fn ie(e: impl std::fmt::Display) -> E {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

// ---- masking -------------------------------------------------------------------------------------------------

fn dots(n: usize) -> String {
    "•".repeat(n.max(3))
}

fn mask_words(s: &str) -> String {
    s.split_whitespace()
        .map(|w| {
            let c: Vec<char> = w.chars().collect();
            if c.len() <= 2 { format!("{}{}", c[0], dots(3)) } else { format!("{}{}{}", c[0], dots(c.len() - 2), c[c.len() - 1]) }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn mask(kind: &str, v: &str) -> String {
    match kind {
        "email" => match v.split_once('@') {
            Some((l, d)) => {
                let (host, tld) = d.rsplit_once('.').unwrap_or((d, ""));
                let f = |s: &str| s.chars().next().map(String::from).unwrap_or_default();
                format!("{}{}@{}{}{}", f(l), dots(3), f(host), dots(3), if tld.is_empty() { String::new() } else { format!(".{tld}") })
            }
            None => mask_words(v),
        },
        "phone" => {
            let d: Vec<char> = v.chars().filter(char::is_ascii_digit).collect();
            let keep = if d.len() >= 9 { 3 } else { 2 }.min(d.len());
            format!("{}{}", dots(d.len() - keep), d[d.len() - keep..].iter().collect::<String>())
        }
        _ => mask_words(v),
    }
}

// ---- ids -----------------------------------------------------------------------------------------------------

fn digits(s: &str) -> String {
    s.chars().filter(char::is_ascii_digit).collect()
}
fn tail10(d: &str) -> String {
    d[d.len().saturating_sub(10)..].to_string()
}
fn nrm(kind: &str, v: &str) -> String {
    if kind == "phone" { tail10(&digits(v)) } else { v.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect() }
}
/// Salted hash of kind + normalised value, truncated: stable per install, not reversible without the salt.
fn pid(salt: &str, kind: &str, v: &str) -> String {
    let h = Sha256::digest(format!("{salt}\u{0}{kind}\u{0}{}", nrm(kind, v)).as_bytes());
    hex::encode(&h[..6])
}
fn kname(k: Kind) -> &'static str {
    match k { Kind::Person => "person", Kind::Email => "email", Kind::Phone => "phone", Kind::Org => "org", Kind::Client => "client" }
}

// ---- model ---------------------------------------------------------------------------------------------------

struct Ent { id: String, kind: String, token: String, value: String, source: &'static str, occ: Vec<(&'static str, usize)> }
struct Cand { id: String, kind: &'static str, value: String, wh: BTreeSet<&'static str>, count: usize }
struct Always { id: String, kind: String, value: String }
struct Model { entries: Vec<Ent>, candidates: Vec<Cand>, always: Vec<Always> }

fn strings(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => out.push(s.clone()),
        Value::Array(a) => a.iter().for_each(|x| strings(x, out)),
        Value::Object(o) => o.values().for_each(|x| strings(x, out)),
        _ => {}
    }
}
fn flat(v: &Value) -> String {
    let mut o = vec![];
    strings(v, &mut o);
    o.join("\n")
}

fn counter(kind: Kind, value: &str) -> Box<dyn Fn(&str) -> usize> {
    if kind == Kind::Phone {
        let t = tail10(&digits(value));
        let re = Regex::new(r"\+?\d[\d\s().-]{6,}\d").unwrap();
        return Box::new(move |text| re.find_iter(text).filter(|m| tail10(&digits(m.as_str())) == t).count());
    }
    let pat = if kind == Kind::Email { regex::escape(value) } else { value.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(regex::escape).collect::<Vec<_>>().join(r"[\W_]+") };
    let re = Regex::new(&format!(r"(?i)\b{pat}\b")).ok();
    Box::new(move |text| re.as_ref().map_or(0, |r| r.find_iter(text).count()))
}

fn detect(text: &str) -> Vec<(&'static str, String)> {
    let mut out = vec![];
    for m in Regex::new(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*\.[A-Za-z]{2,}").unwrap().find_iter(text) {
        out.push(("email", m.as_str().to_string()));
    }
    for m in Regex::new(r"\+?\d[\d\s().-]{6,}\d").unwrap().find_iter(text) {
        let d = digits(m.as_str()).len();
        if d >= 10 || (m.as_str().starts_with('+') && d >= 8) {
            out.push(("phone", m.as_str().trim().to_string()));
        }
    }
    for c in Regex::new(r"(?:^|[^\w@])(@[A-Za-z0-9_]{3,30})\b").unwrap().captures_iter(text) {
        out.push(("url_handle", c[1].to_string()));
    }
    for m in Regex::new(r"(?i)\b(?:linkedin\.com/in/|github\.com/)[A-Za-z0-9_-]{2,40}").unwrap().find_iter(text) {
        out.push(("url_handle", m.as_str().to_string()));
    }
    for c in Regex::new(r"(?i:\b(?:by|with|contact|manager)\b)[:\s]+([A-Z][a-z]+(?:[ \t]+[A-Z][a-z]+)+)").unwrap().captures_iter(text) {
        out.push(("name", c[1].to_string()));
    }
    out
}

async fn model(a: &Arc<App>) -> Result<Model> {
    let kf = a.key_file.clone();
    let (salt, resume, jds, extras, always, dismissed) = a.db(move |c| {
        c.execute("INSERT OR IGNORE INTO settings VALUES('pii_salt',?)", [hex::encode(rand::random::<[u8; 16]>())])?;
        let one = |k: &str| c.query_row("SELECT value FROM settings WHERE key=?", [k], |r| r.get::<_, String>(0)).ok();
        let salt = one("pii_salt").unwrap_or_default();
        let dismissed: Vec<String> = one("pii_dismissed").and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        let resume: Option<String> = c.query_row("SELECT json FROM resume WHERE id=1", [], |r| r.get(0)).ok();
        let mut s = c.prepare("SELECT jd_text,extra_redact_json FROM jobs ORDER BY created_at DESC LIMIT 20")?;
        let rows: Vec<(String, String)> = s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
        let jds: Vec<String> = rows.iter().map(|r| r.0.clone()).collect();
        let extras: Vec<String> = rows.iter().flat_map(|r| serde_json::from_str::<Vec<String>>(&r.1).unwrap_or_default()).collect();
        Ok((salt, resume, jds, extras, always_rows(c, &kf)?, dismissed))
    }).await?;
    let rv: Value = resume.as_deref().and_then(|s| serde_json::from_str(s).ok()).unwrap_or_else(|| json!(Resume::default()));
    let r: Resume = serde_json::from_value(rv.clone()).unwrap_or_default();
    let always: Vec<Always> = always.into_iter().map(|(id, kind, value)| Always { id, kind, value }).collect();
    let mut ex: Vec<(Kind, String)> = extras.iter().map(|s| (Kind::Org, s.clone())).collect();
    ex.extend(always.iter().filter_map(|x| Some((kind_from(&x.kind)?, x.value.clone()))));
    let view = Vault::from_resume(&r, &ex).entries_view();

    let an: HashMap<String, &Always> = always.iter().map(|x| (nrm(&x.kind, &x.value), x)).collect();
    let jn: HashSet<String> = extras.iter().map(|s| nrm("org", s)).collect();
    let mut texts: Vec<(&'static str, String)> = AREAS.iter().map(|k| (*k, flat(&rv[*k]))).collect();
    texts.push(("jd", jds.join("\n")));
    let entries = view.iter().map(|(k, tok, val)| {
        let n = nrm(kname(*k), val);
        let (kind, id, source) = match an.get(&n) {
            Some(x) => (x.kind.clone(), x.id.clone(), "always"),
            None => (kname(*k).to_string(), pid(&salt, kname(*k), val), if jn.contains(&n) { "job" } else if matches!(k, Kind::Org | Kind::Client) { "experience" } else { "profile" }),
        };
        let f = counter(*k, val);
        Ent { id, kind, token: tok.trim_matches(|c| c == '[' || c == ']').to_string(), value: val.clone(), source, occ: texts.iter().map(|(a, t)| (*a, f(t))).filter(|o| o.1 > 0).collect() }
    }).collect::<Vec<_>>();

    // candidates: detected in the resume or recent JDs, not already covered by the vault
    let norms: HashSet<String> = view.iter().map(|(k, _, v)| nrm(kname(*k), v)).collect();
    let phones: HashSet<String> = view.iter().filter(|e| e.0 == Kind::Phone).map(|e| tail10(&digits(&e.2))).collect();
    let covered = |kind: &str, v: &str| match kind {
        "phone" => phones.contains(&tail10(&digits(v))),
        "name" => norms.contains(&nrm("person", v)) || v.split_whitespace().all(|w| norms.contains(&nrm("person", w))),
        _ => norms.contains(&nrm("org", v)),
    };
    let mut cands: BTreeMap<String, Cand> = BTreeMap::new();
    let sources = [("resume", flat(&rv)), ("jd", jds.join("\n"))];
    for (wh, t) in &sources {
        for (kind, v) in detect(t) {
            let id = pid(&salt, kind, &v);
            if covered(kind, &v) || dismissed.contains(&id) {
                continue;
            }
            let c = cands.entry(id.clone()).or_insert(Cand { id, kind, value: v, wh: BTreeSet::new(), count: 0 });
            c.wh.insert(*wh);
            c.count += 1;
        }
    }
    Ok(Model { entries, candidates: cands.into_values().collect(), always })
}

fn cand_kind(k: &str) -> &'static str {
    match k { "name" => "person", "email" => "email", "phone" => "phone", _ => "custom" }
}

async fn list(State(a): S) -> Result<Json<Value>, E> {
    let m = model(&a).await.map_err(ie)?;
    let entries: Vec<Value> = m.entries.iter().map(|e| {
        let occurrences: Vec<Value> = e.occ.iter().map(|(area, count)| json!({"area": area, "count": count})).collect();
        json!({"id": e.id, "kind": e.kind, "token": e.token, "masked": mask(&e.kind, &e.value), "length": e.value.chars().count(), "source": e.source, "occurrences": occurrences, "total": e.occ.iter().map(|o| o.1).sum::<usize>()})
    }).collect();
    let mut cs: Vec<(f32, &Cand)> = m.candidates.iter().map(|c| (crate::mlsvc::pii_score(&a, &c.value, c.wh.contains("jd")), c)).collect();
    cs.sort_by(|x, y| y.0.total_cmp(&x.0)); // most likely PII first (learned from accept/dismiss)
    let candidates: Vec<Value> = cs.iter().map(|(s, c)| json!({"id": c.id, "kind": c.kind, "masked": mask(c.kind, &c.value), "where": c.wh.iter().cloned().collect::<Vec<_>>().join(", "), "count": c.count, "score": s})).collect();
    let always: Vec<Value> = m.always.iter().map(|x| json!({"id": x.id, "kind": x.kind, "masked": mask(&x.kind, &x.value)})).collect();
    Ok(Json(json!({"entries": entries, "candidates": candidates, "always": always})))
}

async fn reveal(State(a): S, h: HeaderMap, Json(b): Json<Value>) -> Result<Json<Value>, E> {
    if h.get("x-confirm").and_then(|v| v.to_str().ok()) != Some("reveal") {
        return Err((StatusCode::BAD_REQUEST, "missing x-confirm: reveal header".into()));
    }
    let id = b["id"].as_str().ok_or((StatusCode::BAD_REQUEST, "id required".to_string()))?;
    let m = model(&a).await.map_err(ie)?;
    let v = m.entries.iter().find(|e| e.id == id).map(|e| &e.value)
        .or_else(|| m.always.iter().find(|e| e.id == id).map(|e| &e.value))
        .or_else(|| m.candidates.iter().find(|e| e.id == id).map(|e| &e.value))
        .ok_or((StatusCode::NOT_FOUND, "no such entry".to_string()))?;
    Ok(Json(json!({"value": v})))
}

async fn insert_always(a: &Arc<App>, kind: &str, value: &str) -> Result<String, E> {
    let value = value.trim().to_string();
    if kind != "custom" && kind_from(kind).is_none() {
        return Err((StatusCode::BAD_REQUEST, "kind must be person|email|phone|org|client|custom".into()));
    }
    if value.chars().count() > 200 || nrm(kind, &value).chars().count() < 3 {
        return Err((StatusCode::BAD_REQUEST, "value must have at least 3 letters or digits (max 200 characters)".into()));
    }
    model(a).await.map_err(ie)?; // makes sure the salt exists
    let salt: String = a.setting("pii_salt").await.unwrap_or_default();
    let (id, kf, kind) = (pid(&salt, kind, &value), a.key_file.clone(), kind.to_string());
    let i = id.clone();
    a.db(move |c| {
        let blob = always_encrypt(&kf, &value)?;
        c.execute("INSERT OR IGNORE INTO pii_always(id,kind,value_encrypted,created_at) VALUES(?,?,?,?)", rusqlite::params![i, kind, blob, crate::now()])?;
        Ok(())
    }).await.map_err(ie)?;
    Ok(id)
}

async fn add_always(State(a): S, Json(b): Json<Value>) -> Result<Json<Value>, E> {
    let v = b["value"].as_str().ok_or((StatusCode::BAD_REQUEST, "value required".to_string()))?;
    Ok(Json(json!({"id": insert_always(&a, b["kind"].as_str().unwrap_or("custom"), v).await?})))
}

async fn del_always(State(a): S, Path(id): Path<String>) -> Result<StatusCode, E> {
    let n = a.db(move |c| Ok(c.execute("DELETE FROM pii_always WHERE id=?", [id])?)).await.map_err(ie)?;
    if n > 0 { Ok(StatusCode::NO_CONTENT) } else { Err((StatusCode::NOT_FOUND, "no such entry".into())) }
}

async fn accept(State(a): S, Path(id): Path<String>) -> Result<Json<Value>, E> {
    let m = model(&a).await.map_err(ie)?;
    let c = m.candidates.iter().find(|c| c.id == id).ok_or((StatusCode::NOT_FOUND, "no such candidate".to_string()))?;
    crate::mlsvc::pii_observe(&a, &c.value, c.wh.contains("jd"), true).await;
    Ok(Json(json!({"id": insert_always(&a, cand_kind(c.kind), &c.value).await?})))
}

async fn dismiss(State(a): S, Path(id): Path<String>) -> Result<StatusCode, E> {
    if let Some(c) = model(&a).await.map_err(ie)?.candidates.iter().find(|c| c.id == id) {
        crate::mlsvc::pii_observe(&a, &c.value, c.wh.contains("jd"), false).await;
    }
    a.db(move |c| {
        let mut v: Vec<String> = c.query_row("SELECT value FROM settings WHERE key='pii_dismissed'", [], |r| r.get::<_, String>(0)).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        if !v.contains(&id) { v.push(id); }
        c.execute("INSERT OR REPLACE INTO settings VALUES('pii_dismissed',?)", [serde_json::to_string(&v)?])?;
        Ok(())
    }).await.map_err(ie)?;
    Ok(StatusCode::NO_CONTENT)
}
