//! AI + lint review of the tailoring plan (saga step `ai_review`) and the owner's overrides.
use crate::{apply_select, ktok, mlsvc, now, retry_job, sel_json, sha, DetailEnv, perm, App, Ctx, JobRow, StepErr};
use anyhow::Result;
use axum::{extract::{Path, State}, http::StatusCode, routing::post, Json, Router};
use resume_core::{ml::{neighbors::similar_jobs, ranker::{Action, Feedback}}};
use resume_core::review::{self as rv, Fix, Overrides};
use resume_core::schema::Resume;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{collections::HashMap, sync::Arc};

pub const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS job_prefs(job_id TEXT PRIMARY KEY, jd_vec_kind TEXT, overrides_json TEXT NOT NULL);";

#[allow(clippy::too_many_arguments)]
fn issue(v: &mut Vec<Value>, source: &str, area: &str, sev: &str, msg: &str, change: Option<Value>, status: &str, reason: Option<String>) {
    let id = format!("{source}-{}", v.len() + 1);
    v.push(json!({"id": id, "area": area, "severity": sev, "message": msg, "source": source, "change": change, "status": status, "reason": reason}));
}

fn area_of(part: &str) -> &'static str {
    match part { "projects" => "projects", "certs" => "certs", "roles" => "experience", _ => "skills" }
}

/// Draft-stage lint (after restore) as issue JSON; only the experience re-sort is applied automatically at this point.
pub fn lint_json(full: &Resume, cx: &Ctx) -> Value {
    let mut v = vec![];
    for i in rv::lint(full, &cx.sel, &cx.jd, &cx.decisions, cx.emb.as_deref(), Some(&cx.facts), true) {
        let applied = i.auto_fix == Some(Fix::SortExperience);
        issue(&mut v, "lint", i.area, i.severity, &i.message, i.auto_fix.as_ref().filter(|_| applied).map(Fix::change), if applied { "applied" } else { "suggested" }, None);
    }
    json!(v)
}

impl App {
    /// Local lint fixes, optional AI review, then the owner's overrides (which win). Provider trouble never fails the job.
    pub(crate) async fn ai_review(self: &Arc<Self>, id: &str, j: &JobRow, cx: &Ctx) -> Result<(Value, String), StepErr> {
        let mut sel = cx.sel.clone();
        let mut is: Vec<Value> = vec![];
        for i in rv::lint(&cx.resume, &sel.clone(), &cx.jd, &cx.decisions, cx.emb.as_deref(), Some(&cx.facts), false) {
            let fix = i.auto_fix.clone();
            if let Some(f) = &fix {
                f.apply(&mut sel);
            }
            issue(&mut is, "lint", i.area, i.severity, &i.message, fix.as_ref().map(Fix::change), if fix.is_some() { "applied" } else { "suggested" }, None);
        }
        let (mut ran, mut ti, mut to, mut conf, mut applied, mut sent, mut cached, mut skipped) = (false, 0i64, 0i64, Value::Null, false, 0usize, false, None::<String>);
        let mut summary = String::new();
        if j.ai_review {
            let (mut sys, user) = rv::prompt(&cx.resume, &sel, &cx.jd, &cx.decisions, &cx.vault, cx.emb.as_deref(), Some(&cx.facts)).map_err(perm)?; // fail closed on a leak
            sys += &crate::learn::review_section(cx);
            cx.vault.guard(&sys).map_err(perm)?;
            sent = (sys.len() + user.len()) / 4;
            let got;
            (got, cached) = self.complete_cached(j, &sys, &user, j.max_tokens.is_some_and(|m| sent as i64 > m)).await?;
            match got.and_then(|(t, a, b)| rv::parse_ai(&t).map(|r| (r, a, b))) {
                Err(why) => {
                    let (jid, m) = (id.to_string(), format!("AI review skipped: {}", why.chars().take(80).collect::<String>()));
                    self.db(move |c| Ok(crate::ev(c, &jid, "ai_review", "warn", &m, false)?)).await?;
                    skipped = Some(why);
                }
                Ok((r, a, b)) => {
                    (ran, ti, to, conf) = (true, a, b, json!(r.confidence.clamp(0.0, 1.0)));
                    let mut parts = vec![];
                    for x in rv::apply_overrides(&cx.resume, &cx.jd, &cx.decisions, &mut sel, &r.overrides, false) {
                        if !x.ok {
                            issue(&mut is, "ai", area_of(x.part), "warn", &format!("AI proposed an invalid {} change", x.part), None, "rejected", x.reason);
                        } else if !(x.added.is_empty() && x.removed.is_empty()) {
                            applied = true;
                            let n = x.added.len().max(x.removed.len());
                            parts.push(match (x.part, n) { ("projects", 1) => "1 project".into(), ("projects", n) => format!("{n} projects"), ("certs", 1) => "1 certification".into(), ("certs", n) => format!("{n} certifications"), (p, n) => format!("{n} {p}") });
                            issue(&mut is, "ai", area_of(x.part), "info", &format!("AI changed {}: added {}, removed {}", x.part, x.added.join(" "), x.removed.join(" ")), Some(json!({"added": x.added, "removed": x.removed})), "applied", None);
                        }
                    }
                    for i in r.issues {
                        let area = ["projects", "certs", "skills", "experience", "summary", "pages", "order"].into_iter().find(|a| *a == i.area).unwrap_or("summary");
                        let sev = ["info", "warn", "error"].into_iter().find(|s| *s == i.severity).unwrap_or("info");
                        issue(&mut is, "ai", area, sev, &i.message.chars().take(200).collect::<String>(), None, "suggested", None);
                    }
                    summary = if parts.is_empty() { "no changes needed".into() } else { format!("swapped {}", parts.join(", ")) };
                }
            }
        }
        // rules are re-applied after lint/AI edits (forbid, require, caps, droppable roles); the owner's choices still win
        resume_core::playbook::enforce(&cx.adj, &cx.resume, &mut sel, cx.decisions.max_certs, cx.decisions.max_projects);
        // the owner's choices always win
        let user = j.overrides.as_deref().and_then(|s| serde_json::from_str::<Value>(s).ok());
        if let Some(u) = &user {
            let ov: Overrides = serde_json::from_value(u.clone()).unwrap_or_default();
            for x in rv::apply_overrides(&cx.resume, &cx.jd, &cx.decisions, &mut sel, &ov, u["force"].as_bool().unwrap_or(false)) {
                if !x.ok {
                    issue(&mut is, "user", area_of(x.part), "warn", &format!("Your {} choice could not be applied", x.part), None, "rejected", x.reason);
                }
            }
        } else if let Some((company, ov)) = self.similar_prefs(id).await? {
            let ov: Overrides = serde_json::from_str(&ov).unwrap_or_default();
            let parts: Vec<&str> = rv::apply_overrides(&cx.resume, &cx.jd, &cx.decisions, &mut sel.clone(), &ov, false).into_iter().filter(|x| x.ok && !(x.added.is_empty() && x.removed.is_empty())).map(|x| x.part).collect();
            if !parts.is_empty() {
                issue(&mut is, "lint", "experience", "info", &format!("For a similar job ({company}) you chose different {}", parts.join(", ")), Some(serde_json::to_value(&ov).unwrap_or_default()), "suggested", None);
            }
        }
        let msg = match (ran, &skipped) {
            (true, _) => format!("AI reviewed the plan: {summary} ({} {} tokens, redacted)", if cached { "reused cache," } else { "sent" }, ktok(sent)),
            (_, Some(_)) => "AI review skipped; kept the local plan (local, no AI)".into(),
            _ => "Checked the plan with local lint (local, no AI)".into(),
        };
        Ok((json!({"ran": ran, "provider": j.provider, "tokens_in": ti, "tokens_out": to, "confidence": conf, "issues": is, "overrides_applied": applied || user.is_some(), "selection": sel_json(&sel), "cached": cached}), msg))
    }

    /// One provider call, cached by (prompt, provider) so a retry never re-sends. `over_budget` skips the call (Err) when nothing is cached.
    pub(crate) async fn complete_cached(self: &Arc<Self>, j: &JobRow, sys: &str, user: &str, over_budget: bool) -> Result<(Result<(String, i64, i64), String>, bool)> {
        let key = sha(&[sys, user, &j.provider]);
        let k = key.clone();
        let hit = self.db(move |c| Ok(c.query_row("SELECT reply_redacted,tokens_in,tokens_out FROM ai_cache WHERE key=?", [k], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?))).optional()?)).await?;
        let cached = hit.is_some();
        let got: Result<(String, i64, i64), String> = match hit {
            Some(h) => Ok(h),
            None if over_budget => Err("over the job token budget".into()),
            None => async {
                let p = self.provider_override.lock().unwrap().clone();
                let p = match p { Some(p) => p, None => self.provider_for(&j.provider).await.map_err(|e| e.to_string())? };
                let r = p.complete(sys, user).await.map_err(|e| e.to_string())?;
                Ok((r.text, r.tokens_in as i64, r.tokens_out as i64))
            }.await,
        };
        if let (Ok((t, a, b)), false) = (&got, cached) {
            let (t, prov, a, b) = (t.clone(), j.provider.clone(), *a, *b);
            self.db(move |c| Ok(c.execute("INSERT OR IGNORE INTO ai_cache VALUES(?,?,?,?,?,?)", params![key, t, prov, a, b, now()])?)).await?;
        }
        Ok((got, cached))
    }

    /// Overrides of the most similar earlier job (same vector kind, similarity >= 0.3): (company, overrides_json).
    async fn similar_prefs(self: &Arc<Self>, id: &str) -> Result<Option<(String, String)>> {
        let jid = id.to_string();
        self.db(move |c| {
            let mine: Option<(String, String)> = c.query_row("SELECT vec_json,vec_kind FROM job_ml WHERE job_id=? AND vec_json IS NOT NULL", [&jid], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
            let Some((q, kind)) = mine else { return Ok(None) };
            let q: Vec<f32> = serde_json::from_str(&q)?;
            let rows: Vec<(String, String, String, String)> = c.prepare("SELECT p.job_id,m.vec_json,coalesce(j.company,'Unknown'),p.overrides_json FROM job_prefs p JOIN job_ml m ON m.job_id=p.job_id JOIN jobs j ON j.id=p.job_id WHERE p.jd_vec_kind=? AND p.job_id!=? AND m.vec_json IS NOT NULL")?
                .query_map(params![kind, jid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect::<rusqlite::Result<_>>()?;
            let past: Vec<(String, Vec<f32>)> = rows.iter().filter_map(|r| Some((r.0.clone(), serde_json::from_str(&r.1).ok()?))).collect();
            Ok(similar_jobs(&q, &past, 1, 0.3).into_iter().next().and_then(|(k, _)| rows.into_iter().find(|r| r.0 == k)).map(|r| (r.2, r.3)))
        }).await
    }
}

/// Job-detail additions: (`pool`, `review`). Restore-stage lint issues are merged in (duplicates by area+message dropped).
pub fn detail(c: &Connection, id: &str, cx: &Ctx, outs: &HashMap<String, Value>) -> Result<(Value, Value)> {
    let mut pool = rv::pool(&cx.resume, &cx.sel, &cx.jd, cx.emb.as_deref(), Some(&cx.facts));
    pool["caps"] = json!({"projects": cx.decisions.max_projects, "certs": cx.decisions.max_certs});
    let Some(o) = outs.get("ai_review") else { return Ok((pool, Value::Null)) };
    let mut issues = o["issues"].as_array().cloned().unwrap_or_default();
    for i in outs.get("restore").and_then(|r| r["lint"].as_array()).into_iter().flatten() {
        if !issues.iter().any(|x| x["area"] == i["area"] && x["message"] == i["message"]) {
            issues.push(i.clone());
        }
    }
    if let Some(p) = outs.get("render_pdf").or(outs.get("render_docx")).and_then(|r| r["fit"]["pages"].as_u64()).filter(|p| *p > cx.decisions.target_pages as u64) {
        issue(&mut issues, "lint", "pages", "warn", &format!("Resume is {p} pages; the target is {}", cx.decisions.target_pages), None, "suggested", None);
    }
    for i in issues.iter_mut() {
        if let Some(m) = i["message"].as_str().and_then(|m| cx.vault.restore(m).ok()) {
            i["message"] = json!(m);
        }
    }
    let uo: Option<String> = c.query_row("SELECT user_overrides FROM jobs WHERE id=?", [id], |r| r.get(0))?;
    let review = json!({"ran": o["ran"], "provider": o["provider"], "tokens_in": o["tokens_in"], "tokens_out": o["tokens_out"], "confidence": o["confidence"], "issues": issues, "overrides_applied": o["overrides_applied"],
        "user_overrides": uo.and_then(|s| serde_json::from_str::<Value>(&s).ok())});
    Ok((pool, review))
}

pub fn routes() -> Router<Arc<App>> {
    Router::new().route("/api/jobs/:id/overrides", post(set_overrides))
}

#[derive(Deserialize)]
struct Body {
    #[serde(flatten)]
    ov: Overrides,
    #[serde(default)]
    force: bool,
    /// Drop the owner's overrides and re-run with the AI plan.
    #[serde(default)]
    reset: bool,
    note: Option<String>,
}

type E = (StatusCode, String);

/// `[]` = select none, `null`/absent = leave as is. Validated like the AI's overrides; stored, ML-trained, and the job re-runs from `select`.
async fn set_overrides(State(a): State<Arc<App>>, Path(id): Path<String>, Json(b): Json<Body>) -> Result<StatusCode, E> {
    let (env, me) = (DetailEnv::of(&a), a.clone());
    let code = a.db(move |c| {
        let row: Option<(Option<String>, String, String, String, Option<String>, Option<String>)> = c.query_row("SELECT (SELECT json FROM resume WHERE id=1),jd_text,extra_redact_json,status,user_overrides,(SELECT vec_kind FROM job_ml WHERE job_id=jobs.id) FROM jobs WHERE id=?", [&id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))).optional()?;
        let Some((rj, jd, extra, status, old, kind)) = row else { return Ok(StatusCode::NOT_FOUND) };
        if !matches!(status.as_str(), "failed" | "dead" | "cancelled" | "done") {
            return Ok(StatusCode::CONFLICT);
        }
        if b.reset {
            c.execute("UPDATE jobs SET user_overrides=NULL WHERE id=?", [&id])?;
            c.execute("DELETE FROM job_prefs WHERE job_id=?", [&id])?;
            retry_job(c, &id, "select")?;
            return Ok(StatusCode::ACCEPTED);
        }
        let outs: HashMap<String, Value> = c.prepare("SELECT name,output_json FROM job_steps WHERE job_id=? AND name IN('select','ai_review') AND status='done' AND output_json IS NOT NULL")?
            .query_map([&id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?.into_iter().filter_map(|(n, o)| Some((n, serde_json::from_str(&o).ok()?))).collect();
        let mut cx = env.ctx(c, &id, &rj.ok_or_else(|| anyhow::anyhow!("no resume stored"))?, &jd, &serde_json::from_str::<Vec<String>>(&extra)?, outs.get("select"))?;
        if let Some(o) = outs.get("ai_review") {
            apply_select(&mut cx, o);
        }
        let prev = cx.sel.clone();
        let mut new = prev.clone();
        let ap = rv::apply_overrides(&cx.resume, &cx.jd, &cx.decisions, &mut new, &b.ov, b.force);
        if let Some(x) = ap.iter().find(|x| !x.ok) {
            anyhow::bail!("{}: {}", x.part, x.reason.clone().unwrap_or_default());
        }
        // learn: role bullets feed the ranker; projects/certs are recorded as feedback rows only (no bullet features)
        let feats = mlsvc::bullet_feats(&cx.resume, &cx.jd, &cx.facts, cx.emb.as_deref());
        let mut m = me.ml.lock().unwrap();
        let mut trained = false;
        for x in &ap {
            for (ids, action, name) in [(&x.added, Action::Keep, "keep"), (&x.removed, Action::Remove, "remove")] {
                for pid in ids {
                    let bullets: Vec<String> = match (x.part, pid[1..].parse::<usize>()) {
                        ("roles", Ok(i)) => { let s = if action == Action::Keep { &new } else { &prev }; s.bullets.get(i).into_iter().flatten().map(|j| format!("e{i}.b{j}")).collect() }
                        _ => vec![pid.clone()],
                    };
                    for bid in bullets {
                        let f = feats.iter().find(|f| f.0 == bid).map(|f| f.1.clone());
                        c.execute("INSERT INTO ml_feedback(job_id,bullet_id,action,features_json,ts) VALUES(?,?,?,?,?)", params![id, bid, name, serde_json::to_string(&f)?, now()])?;
                        if let Some(f) = f {
                            m.ranker.observe(&Feedback { job_id: id.clone(), bullet_id: bid, action }, &f);
                            trained = true;
                        }
                    }
                }
            }
        }
        if trained {
            m.ranker.refit();
            mlsvc::save(c, &m)?;
        }
        drop(m);
        crate::learn::record_overrides(&me, c, &id, &cx.resume, &prev, &new, &ap)?;
        let mut merged: serde_json::Map<String, Value> = old.and_then(|o| serde_json::from_str(&o).ok()).unwrap_or_default();
        if let Value::Object(n) = serde_json::to_value(&b.ov)? {
            merged.extend(n);
        }
        merged.insert("force".into(), json!(b.force || merged.get("force").and_then(Value::as_bool).unwrap_or(false)));
        if let Some(n) = &b.note {
            merged.insert("note".into(), json!(n.chars().take(500).collect::<String>()));
        }
        let merged = Value::Object(merged).to_string();
        c.execute("UPDATE jobs SET user_overrides=? WHERE id=?", params![merged, id])?;
        c.execute("INSERT INTO job_prefs(job_id,jd_vec_kind,overrides_json) VALUES(?1,?2,?3) ON CONFLICT(job_id) DO UPDATE SET jd_vec_kind=?2,overrides_json=?3", params![id, kind, merged])?;
        retry_job(c, &id, "select")?;
        Ok(StatusCode::ACCEPTED)
    }).await.map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    if code == StatusCode::ACCEPTED {
        a.mine_bg();
    }
    match code {
        StatusCode::NOT_FOUND => Err((code, "no such job".into())),
        StatusCode::CONFLICT => Err((code, "job is still running; wait for it to finish".into())),
        c => Ok(c),
    }
}
