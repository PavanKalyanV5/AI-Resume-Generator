//! Wires `resume_core::ml` into the server: the bundle lives in settings ('ml_bundle'), every usage signal
//! (rewrite outcomes, bullet feedback, PII accept/dismiss, JD labels, applications, renders) trains it.
use crate::{api::job_detail_env, now, App, Ctx, DetailEnv};
use anyhow::Result;
use axum::{extract::{Path, State}, http::StatusCode, routing::{get, post}, Json, Router};
use resume_core::{
    embed::{cosine, Embedder},
    jd::{term_counts, Jd, ReqKind},
    latex::Layout,
    ml::{
        core::now_iso,
        fit_model::{pdf_line_counts, ContentStats, FitModel},
        jd_classifier::{FAMILIES, SENIORITIES},
        neighbors::{jd_vector, similar_jobs},
        outcome::{funnel, Application, ApplicationFeatures, Stage},
        pii_candidate::Candidate,
        ranker::{blend, normalise, Action, BulletFeatures, BulletRanker, Feedback},
        recommend::{JdTerms, SkillRecommender},
        rewrite_need::{RewriteFeatures, RewriteNeed, RewriteOutcome},
        jd_classifier::JdClassifier, ModelBundle,
    },
    profile_model::{build_facts, extract_metrics, verb_class, ProfileFacts, VerbClass},
    schema::Resume,
    select::{bm25, Selection},
    seniority::{kind_of, now_month, RoleKind},
};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::sync::Arc;

type E = (StatusCode, String);
type S = State<Arc<App>>;
fn ie(e: impl std::fmt::Display) -> E {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}
fn bad(e: impl std::fmt::Display) -> E {
    (StatusCode::BAD_REQUEST, e.to_string())
}

pub const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS ml_feedback(id INTEGER PRIMARY KEY AUTOINCREMENT, job_id TEXT NOT NULL, bullet_id TEXT NOT NULL, action TEXT NOT NULL, features_json TEXT NOT NULL, ts INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS jd_corpus(job_id TEXT PRIMARY KEY, terms_json TEXT NOT NULL, ts TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS job_ml(job_id TEXT PRIMARY KEY, vec_json TEXT, vec_kind TEXT, label_json TEXT);
CREATE TABLE IF NOT EXISTS applications(job_id TEXT PRIMARY KEY, stage TEXT NOT NULL, note TEXT, features_json TEXT NOT NULL, applied_at TEXT NOT NULL, updated_at TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS application_history(id INTEGER PRIMARY KEY AUTOINCREMENT, job_id TEXT NOT NULL, stage TEXT NOT NULL, note TEXT, ts TEXT NOT NULL);";

/// Missing or corrupt => cold-start defaults.
pub fn load(c: &Connection) -> ModelBundle {
    c.query_row("SELECT value FROM settings WHERE key='ml_bundle'", [], |r| r.get::<_, String>(0)).ok().and_then(|s| ModelBundle::from_json(&s).ok()).unwrap_or_default()
}

/// One SQL statement, so the stored bundle is always whole.
pub fn save(c: &Connection, b: &ModelBundle) -> Result<()> {
    c.execute("INSERT OR REPLACE INTO settings VALUES('ml_bundle',?)", [b.to_json()])?;
    Ok(())
}

impl App {
    /// Mutate the bundle and persist it. Lock order is always conn then ml.
    pub async fn ml_do<R: Send + 'static>(self: &Arc<Self>, f: impl FnOnce(&mut ModelBundle) -> R + Send + 'static) -> Result<R> {
        let me = self.clone();
        self.db(move |c| {
            let mut b = me.ml.lock().unwrap();
            let r = f(&mut b);
            save(c, &b)?;
            Ok(r)
        }).await
    }

    /// Refit every model that has data, in the background; saved when done.
    pub fn retrain_bg(self: &Arc<Self>) {
        let me = self.clone();
        tokio::spawn(async move {
            let r = me.ml_do(|b| {
                if b.rewrite.n() > 0 { b.rewrite.refit(); }
                if b.ranker.n() > 0 { b.ranker.refit(); }
                if b.pii.n() > 0 { b.pii.refit(); }
                b.outcome.refit();
            }).await;
            match r {
                Ok(()) => me.notify("info", "ml.retrained", "Models updated", "Local models were retrained from your usage."),
                Err(e) => me.notify("warn", "ml.retrain_failed", "Model retrain failed", &e.to_string()),
            }
        });
    }

    /// parse_jd hook: neighbour vector (embedder or hashed, kind recorded) and recommender corpus row.
    pub async fn record_jd(self: &Arc<Self>, id: &str, text: &str, jd: &Jd) -> Result<()> {
        let (v, kind) = vec_of(text, self.embedder().as_deref());
        let terms = serde_json::to_string(&jd.requirements.iter().map(|q| q.term.clone()).collect::<Vec<_>>())?;
        let id = id.to_string();
        self.db(move |c| {
            c.execute("INSERT INTO job_ml(job_id,vec_json,vec_kind) VALUES(?1,?2,?3) ON CONFLICT(job_id) DO UPDATE SET vec_json=?2,vec_kind=?3", params![id, serde_json::to_string(&v)?, kind])?;
            c.execute("INSERT OR REPLACE INTO jd_corpus VALUES(?,?,?)", params![id, terms, now_iso()])?;
            Ok(())
        }).await
    }

    /// restore hook: sent bullets that changed (grounding reverts leave them unchanged) were accepted.
    pub async fn rw_observe(self: &Arc<Self>, cx: &Ctx, sent: &Value, full: &Resume) {
        let obs: Vec<(RewriteFeatures, bool)> = sent.as_object().into_iter().flatten().filter_map(|(id, x)| {
            let (i, j) = id.strip_prefix('e')?.split_once(".b")?;
            let (i, j): (usize, usize) = (i.parse().ok()?, j.parse().ok()?);
            Some((serde_json::from_value(x.clone()).ok()?, full.experience.get(i)?.bullets.get(j)? != cx.resume.experience.get(i)?.bullets.get(j)?))
        }).collect();
        if obs.is_empty() {
            return;
        }
        let _ = self.ml_do(move |b| obs.iter().for_each(|(x, a)| b.rewrite.observe(x, RewriteOutcome { accepted: *a }))).await;
        self.retrain_bg();
    }

    /// render_pdf hook: learn real per-page line counts for the layout actually used.
    pub async fn fit_observe(self: &Arc<Self>, id: &str, r: &Resume, level: usize) {
        let path = self.out.join(id).join("resume.pdf");
        let Ok(Ok(lines)) = tokio::task::spawn_blocking(move || pdf_line_counts(&path)).await else { return };
        let (s, l) = (ContentStats::from_resume(r), levels().swap_remove(level.min(Layout::levels().len())));
        let _ = self.ml_do(move |b| b.fit.observe_pages(&s, &l, &lines)).await;
    }
}

fn levels() -> Vec<Layout> {
    std::iter::once(Layout::default()).chain(Layout::levels()).collect()
}

/// Starting layout level when the fit model is confident, else None (existing heuristic applies).
pub fn suggest_level(m: &FitModel, full: &Resume, sel: &Selection, target: u8) -> Option<usize> {
    let s = ContentStats::from_resume(&sel.apply(full));
    let c: Vec<_> = levels().into_iter().map(|l| (s.clone(), l)).collect();
    m.suggest_start(&c, target).filter(|s| s.confidence >= 0.6).map(|s| s.index)
}

fn vec_of(t: &str, e: Option<&dyn Embedder>) -> (Vec<f32>, String) {
    match e.and_then(|e| e.embed(&[t.to_string()]).ok()).and_then(|mut v| v.pop()).filter(|v| !v.is_empty()) {
        Some(v) => { let k = format!("embedder:{}", v.len()); (v, k) }
        None => (jd_vector(t, None), "hashed".into()),
    }
}

// ---- rewrite_need --------------------------------------------------------------------------------------------

fn rw_feats(t: &str, jd: &Jd, f: &ProfileFacts) -> RewriteFeatures {
    let c = term_counts(t, jd);
    RewriteFeatures {
        potential_gain: jd.requirements.iter().zip(&c).filter(|(q, n)| **n == 0 && q.kind == ReqKind::Skill && f.skill(&q.term).is_some()).map(|(q, _)| q.weight).sum(),
        length: t.chars().count(),
        has_metric: !extract_metrics(t).is_empty(),
        verb_strength: match verb_class(t) { VerbClass::Strong => 1.0, VerbClass::Medium => 0.6, VerbClass::Weak => 0.2, VerbClass::None => 0.0 },
        keyword_hits: c.iter().filter(|n| **n > 0).count() as f32,
    }
}

/// Selection without the experience bullets not worth a rewrite (ids keep their original indices, so a
/// reply covering only the sent subset parses as-is), the features of the sent ones, skipped count, tokens saved.
pub fn prune(m: &RewriteNeed, cx: &Ctx) -> (Selection, Value, usize, usize) {
    let (mut sel, mut sent, mut chars, mut n) = (cx.sel.clone(), serde_json::Map::new(), 0, 0);
    for (i, idx) in sel.bullets.iter_mut().enumerate() {
        idx.retain(|&j| {
            let Some(t) = cx.resume.experience.get(i).and_then(|e| e.bullets.get(j)) else { return true };
            let x = rw_feats(t, &cx.jd, &cx.facts);
            let keep = m.should_rewrite(&x).0;
            if keep { sent.insert(format!("e{i}.b{j}"), json!(x)); } else { n += 1; chars += t.len() + 24; }
            keep
        });
    }
    (sel, Value::Object(sent), n, chars / 4)
}

pub fn saved_note(n: usize, saved: usize) -> String {
    if n == 0 { String::new() } else { format!("; Skipping {n} bullets already strong, saved ~{saved} tokens") }
}

// ---- ranker --------------------------------------------------------------------------------------------------

/// (id `e{i}.b{j}`, features, raw bm25) for every experience bullet, in resume order.
pub fn bullet_feats(r: &Resume, jd: &Jd, f: &ProfileFacts, emb: Option<&dyn Embedder>) -> Vec<(String, BulletFeatures, f32)> {
    let docs: Vec<String> = r.experience.iter().flat_map(|e| e.bullets.iter().cloned()).collect();
    let raw = bm25(&docs, jd);
    let norm = normalise(&raw);
    let total: f32 = jd.requirements.iter().map(|q| q.weight).sum::<f32>().max(f32::MIN_POSITIVE);
    let cos: Option<Vec<f32>> = emb.and_then(|e| {
        let reqs: Vec<String> = jd.requirements.iter().map(|q| q.term.clone()).collect();
        if reqs.is_empty() { return None; }
        let (rv, dv) = (e.embed(&reqs).ok()?, e.embed(&docs).ok()?);
        Some(dv.iter().map(|d| rv.iter().map(|q| cosine(d, q)).fold(0.0, f32::max).max(0.0)).collect())
    });
    let mut out = vec![];
    for (i, e) in r.experience.iter().enumerate() {
        for (j, t) in e.bullets.iter().enumerate() {
            let k = out.len();
            let kw: f32 = term_counts(t, jd).iter().zip(&jd.requirements).filter(|(n, _)| **n > 0).map(|(_, q)| q.weight).sum();
            out.push((format!("e{i}.b{j}"), BulletFeatures {
                bm25_norm: norm[k],
                max_cos: cos.as_ref().map_or(0.0, |c| c[k]),
                kw_hit: kw / total,
                skill_prof: f.strength_of(t),
                recency: 1.0 / (1.0 + i as f32),
                has_metric: !extract_metrics(t).is_empty(),
                verb_class: match verb_class(t) { VerbClass::Strong => 2, VerbClass::Medium => 1, _ => 0 },
                length: t.chars().count(),
                position: j as f32 / e.bullets.len().max(1) as f32,
                role_kind: match kind_of(e) { RoleKind::FullTime => 0, RoleKind::Internship => 1, RoleKind::Other => 2 },
            }, raw[k]));
        }
    }
    out
}

/// Re-pick which bullets each kept role shows (same counts) by blend(normalised bm25, ranker, confidence); no-op while confidence is 0.
pub fn rerank(cx: &mut Ctx, rk: &BulletRanker) {
    let conf = rk.confidence();
    if conf <= 0.0 {
        return;
    }
    let f = bullet_feats(&cx.resume, &cx.jd, &cx.facts, cx.emb.as_deref());
    let ml = rk.rank(&f.iter().map(|x| x.1.clone()).collect::<Vec<_>>());
    let sel = normalise(&f.iter().map(|x| x.2).collect::<Vec<_>>());
    let score: Vec<f32> = (0..f.len()).map(|k| blend(sel[k], ml[k], conf)).collect();
    let mut off = 0;
    for (i, e) in cx.resume.experience.iter().enumerate() {
        let n = cx.sel.bullets.get(i).map_or(0, Vec::len);
        if n > 0 {
            let mut idx: Vec<usize> = (0..e.bullets.len()).collect();
            idx.sort_by(|&a, &b| score[off + b].total_cmp(&score[off + a]).then(a.cmp(&b)));
            idx.truncate(n);
            idx.sort();
            cx.sel.bullets[i] = idx;
        }
        off += e.bullets.len();
    }
}

// ---- pii_candidate -------------------------------------------------------------------------------------------

pub fn pii_score(a: &App, value: &str, in_jd: bool) -> f32 {
    a.ml.lock().unwrap().pii.score(&Candidate { token: value, prev: None, in_jd })
}

pub async fn pii_observe(a: &Arc<App>, value: &str, in_jd: bool, accepted: bool) {
    let v = value.to_string();
    let _ = a.ml_do(move |b| {
        b.pii.observe(&Candidate { token: &v, prev: None, in_jd }, accepted);
        b.pii.refit();
    }).await;
}

// ---- job detail: jd class, neighbours, outcome ---------------------------------------------------------------

fn stage_of(s: &str) -> Option<Stage> {
    Some(match s { "applied" => Stage::Applied, "response" => Stage::Response, "interview" => Stage::Interview, "offer" => Stage::Offer, "rejected" => Stage::Rejected, "ghosted" => Stage::Ghosted, _ => return None })
}

/// Outcome features from a job-detail document.
fn app_feats(v: &Value, jd: &JdClassifier) -> ApplicationFeatures {
    let cls = jd.classify(v["job"]["jd_text"].as_str().unwrap_or(""));
    let (cov, len) = (&v["coverage"], |k: &str| v["coverage"][k].as_array().map_or(0, Vec::len) as f32);
    let years = v["facts_summary"]["years"].as_f64().unwrap_or(0.0) as f32;
    let have = SENIORITIES[[2.0, 5.0, 9.0].iter().filter(|t| years >= **t).count()];
    let need = cls.seniority.first().map_or(0.0, |s| match s.0.as_str() { "junior" => 1.0, "mid" => 3.0, "senior" => 6.0, _ => 9.0 });
    ApplicationFeatures {
        coverage: cov["score"].as_f64().unwrap_or(0.0) as f32,
        semantic_share: len("semantic") / len("covered").max(1.0),
        required_missing: len("missing"),
        years_gap: need - years,
        pages: v["fit"]["pages"].as_f64().or(v["decisions"]["target_pages"].as_f64()).unwrap_or(1.0) as f32,
        family_match_prob: cls.family.first().map_or(0.0, |f| f.1),
        seniority_match: cls.seniority.iter().find(|s| s.0 == have).map_or(0.0, |s| s.1),
    }
}

/// Adds `jd_class`, `similar_jobs` and `outcome` to a job-detail document.
pub async fn extend_detail(a: &Arc<App>, v: &mut Value) -> Result<()> {
    let id = v["job"]["id"].as_str().unwrap_or_default().to_string();
    let (cls, outcome) = {
        let m = a.ml.lock().unwrap();
        (m.jd.classify(v["job"]["jd_text"].as_str().unwrap_or("")), m.outcome.predict(&app_feats(v, &m.jd)))
    };
    let (label, sim) = a.db(move |c| {
        let label: Option<String> = c.query_row("SELECT label_json FROM job_ml WHERE job_id=?", [&id], |r| r.get(0)).optional()?.flatten();
        let mine: Option<(String, String)> = c.query_row("SELECT vec_json,vec_kind FROM job_ml WHERE job_id=? AND vec_json IS NOT NULL", [&id], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
        let mut sim = vec![];
        if let Some((q, kind)) = mine {
            let q: Vec<f32> = serde_json::from_str(&q)?;
            let mut s = c.prepare("SELECT m.job_id,m.vec_json,coalesce(j.company,'Unknown'),coalesce(j.role,'Role') FROM job_ml m JOIN jobs j ON j.id=m.job_id WHERE m.vec_kind=? AND m.job_id!=? AND m.vec_json IS NOT NULL")?;
            let rows: Vec<(String, String, String, String)> = s.query_map(params![kind, id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect::<rusqlite::Result<_>>()?;
            let past: Vec<(String, Vec<f32>)> = rows.iter().filter_map(|r| Some((r.0.clone(), serde_json::from_str(&r.1).ok()?))).collect();
            for (jid, s) in similar_jobs(&q, &past, 5, 0.15) {
                let r = rows.iter().find(|r| r.0 == jid).unwrap();
                sim.push(json!({"id": jid, "company": r.2, "role": r.3, "similarity": s}));
            }
        }
        Ok((label.and_then(|l| serde_json::from_str::<Value>(&l).ok()), sim))
    }).await?;
    v["jd_class"] = json!({"family": cls.family, "seniority": cls.seniority, "evidence": cls.evidence, "confirmed": label});
    v["similar_jobs"] = json!(sim);
    v["outcome"] = json!(outcome);
    Ok(())
}

// ---- endpoints -----------------------------------------------------------------------------------------------

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/ml/cards", get(cards))
        .route("/api/ml/retrain", post(retrain))
        .route("/api/ml/recommendations", get(recommendations))
        .route("/api/jobs/:id/feedback", post(feedback))
        .route("/api/jobs/:id/jd-class", post(jd_class))
        .route("/api/applications", post(app_upsert).get(app_list))
        .route("/api/applications/funnel", get(app_funnel))
}

async fn cards(State(a): S) -> Result<Json<Value>, E> {
    let me = a.clone();
    let v = a.db(move |c| {
        let n = |t: &str| -> Result<i64> { Ok(c.query_row(&format!("SELECT count(*) FROM {t}"), [], |r| r.get(0))?) };
        let b = me.ml.lock().unwrap();
        Ok(json!({"cards": b.cards(), "signals": {"feedback": n("ml_feedback")?, "applications": n("applications")?, "jd_corpus": n("jd_corpus")?, "job_vectors": n("job_ml")?,
            "rewrite_samples": b.rewrite.n(), "ranker_samples": b.ranker.n(), "pii_samples": b.pii.n(), "fit_observations": b.fit.n_obs(), "outcome_labelled": b.outcome.n_labelled(), "jd_corrections": b.jd.corrections}}))
    }).await.map_err(ie)?;
    Ok(Json(v))
}

async fn retrain(State(a): S) -> StatusCode {
    a.retrain_bg();
    StatusCode::ACCEPTED
}

async fn recommendations(State(a): S) -> Result<Json<Value>, E> {
    let v = a.db(|c| {
        let r: Resume = c.query_row("SELECT json FROM resume WHERE id=1", [], |r| r.get::<_, String>(0)).optional()?.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        let m = now_month();
        let facts = build_facts(&r, (m.div_euclid(12), m.rem_euclid(12) as u32 + 1));
        let mut s = c.prepare("SELECT ts,terms_json FROM jd_corpus ORDER BY ts")?;
        let corpus: Vec<JdTerms> = s.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?.into_iter().map(|(ts, t)| JdTerms { ts, terms: serde_json::from_str(&t).unwrap_or_default() }).collect();
        let rec = SkillRecommender::from_facts(&facts, &corpus);
        Ok(json!({"learn_next": rec.learn_next(10), "trends": rec.trends(10), "resume_additions": rec.resume_additions()}))
    }).await.map_err(ie)?;
    Ok(Json(v))
}

async fn feedback(State(a): S, Path(id): Path<String>, Json(b): Json<Value>) -> Result<Json<Value>, E> {
    let bid = b["bullet_id"].as_str().ok_or_else(|| bad("bullet_id required"))?.to_string();
    let act = b["action"].as_str().unwrap_or_default().to_string();
    let action = match act.as_str() { "keep" => Action::Keep, "remove" => Action::Remove, "add_back" => Action::AddBack, "edit" => Action::Edit, _ => return Err(bad("action must be keep|remove|add_back|edit")) };
    let (env, me) = (DetailEnv::of(&a), a.clone());
    let v = a.db(move |c| {
        let row: Option<(Option<String>, String, String)> = c.query_row("SELECT (SELECT json FROM resume WHERE id=1),jd_text,extra_redact_json FROM jobs WHERE id=?", [&id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).optional()?;
        let Some((rj, jd, extra)) = row else { return Ok(None) };
        let cx = env.ctx(c, &id, &rj.ok_or_else(|| anyhow::anyhow!("no resume stored"))?, &jd, &serde_json::from_str::<Vec<String>>(&extra)?, None)?;
        let feats = bullet_feats(&cx.resume, &cx.jd, &cx.facts, cx.emb.as_deref());
        let Some((_, x, _)) = feats.iter().find(|f| f.0 == bid) else { anyhow::bail!("unknown bullet_id (use e<role>.b<n>)") };
        c.execute("INSERT INTO ml_feedback(job_id,bullet_id,action,features_json,ts) VALUES(?,?,?,?,?)", params![id, bid, act, serde_json::to_string(x)?, now()])?;
        let i = crate::learn::info(&me, c, &id)?;
        crate::learn::record(c, &id, &i, "bullets", match act.as_str() { "add_back" => "add", a => a }, &bid, &json!({"kind": "bullet", "features": x}), &Value::Null, &Value::Null, 1.0)?;
        let mut m = me.ml.lock().unwrap();
        m.ranker.observe(&Feedback { job_id: id, bullet_id: bid, action }, x);
        m.ranker.refit();
        save(c, &m)?;
        Ok(Some(json!({"samples": m.ranker.n(), "confidence": m.ranker.confidence()})))
    }).await.map_err(bad)?;
    v.map(Json).ok_or((StatusCode::NOT_FOUND, "no such job".into()))
}

async fn jd_class(State(a): S, Path(id): Path<String>, Json(b): Json<Value>) -> Result<StatusCode, E> {
    let pick = |k: &str, ok: &[&str]| b[k].as_str().filter(|s| ok.contains(s)).map(String::from).ok_or_else(|| bad(format!("{k} must be one of {ok:?}")));
    let (fam, sen) = (pick("family", &FAMILIES)?, pick("seniority", &SENIORITIES)?);
    let (i, label) = (id.clone(), json!({"family": fam, "seniority": sen}).to_string());
    let text: Option<String> = a.db(move |c| {
        c.execute("INSERT INTO job_ml(job_id,label_json) SELECT id,? FROM jobs WHERE id=? ON CONFLICT(job_id) DO UPDATE SET label_json=excluded.label_json", params![label, i])?;
        Ok(c.query_row("SELECT jd_text FROM jobs WHERE id=?", [&i], |r| r.get(0)).optional()?)
    }).await.map_err(ie)?;
    let text = text.ok_or((StatusCode::NOT_FOUND, "no such job".to_string()))?;
    a.ml_do(move |m| m.jd.correct(&text, &fam, &sen)).await.map_err(ie)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn app_upsert(State(a): S, Json(b): Json<Value>) -> Result<Json<Value>, E> {
    let job = b["job_id"].as_str().ok_or_else(|| bad("job_id required"))?.to_string();
    let stage = b["stage"].as_str().unwrap_or_default().to_string();
    if stage_of(&stage).is_none() {
        return Err(bad("stage must be applied|response|interview|offer|rejected|ghosted"));
    }
    let note = b["note"].as_str().map(String::from);
    let (env, me) = (DetailEnv::of(&a), a.clone());
    let ok = a.db(move |c| {
        let have = c.query_row("SELECT 1 FROM applications WHERE job_id=?", [&job], |_| Ok(())).optional()?.is_some();
        let (ts, feats) = (now_iso(), if have { String::new() } else {
            let Some(v) = job_detail_env(c, &job, &env)? else { return Ok(false) };
            serde_json::to_string(&app_feats(&v, &me.ml.lock().unwrap().jd))?
        });
        c.execute("INSERT INTO applications VALUES(?1,?2,?3,?4,?5,?5) ON CONFLICT(job_id) DO UPDATE SET stage=?2,note=coalesce(?3,note),updated_at=?5", params![job, stage, note, feats, ts])?;
        c.execute("INSERT INTO application_history(job_id,stage,note,ts) VALUES(?,?,?,?)", params![job, stage, note, ts])?;
        let rows: Vec<(String, String)> = c.prepare("SELECT stage,features_json FROM applications ORDER BY applied_at,rowid")?.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
        let mut m = me.ml.lock().unwrap();
        m.outcome.samples.clear(); // rebuilt from the table so stage updates never double count
        for (s, f) in rows {
            if let (Some(s), Ok(f)) = (stage_of(&s), serde_json::from_str::<ApplicationFeatures>(&f)) { m.outcome.observe(&f, s); }
        }
        m.outcome.refit();
        save(c, &m)?;
        Ok(true)
    }).await.map_err(bad)?;
    if ok { Ok(Json(json!({"ok": true}))) } else { Err((StatusCode::NOT_FOUND, "no such job".into())) }
}

async fn app_list(State(a): S) -> Result<Json<Value>, E> {
    let v = a.db(|c| {
        let mut h = c.prepare("SELECT stage,note,ts FROM application_history WHERE job_id=? ORDER BY id")?;
        let mut s = c.prepare("SELECT a.job_id,coalesce(j.company,'Unknown'),coalesce(j.role,'Role'),a.stage,a.note,a.applied_at,a.updated_at FROM applications a LEFT JOIN jobs j ON j.id=a.job_id ORDER BY a.applied_at DESC,a.rowid DESC")?;
        let rows: Vec<(String, String, String, String, Option<String>, String, String)> = s.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)))?.collect::<rusqlite::Result<_>>()?;
        rows.into_iter().map(|r| {
            let hist: Vec<Value> = h.query_map([&r.0], |x| Ok(json!({"stage": x.get::<_, String>(0)?, "note": x.get::<_, Option<String>>(1)?, "ts": x.get::<_, String>(2)?})))?.collect::<rusqlite::Result<_>>()?;
            Ok(json!({"job_id": r.0, "company": r.1, "role": r.2, "stage": r.3, "note": r.4, "applied_at": r.5, "updated_at": r.6, "history": hist}))
        }).collect::<Result<Vec<_>>>()
    }).await.map_err(ie)?;
    Ok(Json(json!(v)))
}

async fn app_funnel(State(a): S) -> Result<Json<Value>, E> {
    let v = a.db(|c| {
        let apps: Vec<Application> = c.prepare("SELECT job_id,applied_at,stage FROM applications")?.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))?.collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter().filter_map(|(job_id, applied_at, s)| Some(Application { job_id, applied_at, stage: stage_of(&s)? })).collect();
        Ok(json!(funnel(&apps)))
    }).await.map_err(ie)?;
    Ok(Json(v))
}
