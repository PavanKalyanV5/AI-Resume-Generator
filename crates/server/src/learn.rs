//! Learning backend: the owner's corrections, the versioned playbook of rules (seeded, mined, user-written), the
//! approved-resume golden set replayed WITHOUT any AI, and the learning-curve metrics. Nothing here is sent to a provider.
use crate::{apply_select, now, realtime, retry_job, Ctx, DetailEnv, Opts, App};
use anyhow::Result;
use axum::{extract::{Path, State}, http::StatusCode, routing::{get, post}, Json, Router};
use resume_core::{
    ml::jd_classifier::JdClassifier,
    playbook::{self as pb, Action, Cand, Cond, Correction, JdContext, Matcher, Rule, RuleHit},
    redact::{Kind, Vault},
    review::Applied,
    schema::Resume,
    select::Selection,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::{collections::{BTreeMap, HashMap}, sync::Arc};

type E = (StatusCode, String);
type R = std::result::Result<Value, E>;
fn bad(m: impl std::fmt::Display) -> E {
    (StatusCode::BAD_REQUEST, m.to_string())
}
fn nf(m: &str) -> E {
    (StatusCode::NOT_FOUND, m.into())
}
fn conflict(m: &str) -> E {
    (StatusCode::CONFLICT, m.into())
}

pub const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS corrections(id INTEGER PRIMARY KEY AUTOINCREMENT, job_id TEXT NOT NULL, ts INTEGER NOT NULL, area TEXT NOT NULL, action TEXT NOT NULL, item_id TEXT NOT NULL,
  item_json TEXT NOT NULL, before_json TEXT, after_json TEXT, reason_tags TEXT NOT NULL DEFAULT '[]', jd_family TEXT NOT NULL DEFAULT '', seniority TEXT NOT NULL DEFAULT '',
  company TEXT NOT NULL DEFAULT '', weight REAL NOT NULL DEFAULT 1, source TEXT NOT NULL DEFAULT 'owner');
CREATE INDEX IF NOT EXISTS corrections_job ON corrections(job_id);
CREATE TABLE IF NOT EXISTS playbook_rules(id TEXT PRIMARY KEY, status TEXT NOT NULL, scope TEXT NOT NULL, area TEXT NOT NULL, matcher_json TEXT NOT NULL, action_json TEXT NOT NULL,
  condition_json TEXT NOT NULL DEFAULT '{}', text TEXT NOT NULL, origin TEXT NOT NULL, support_count INTEGER NOT NULL DEFAULT 0, examples_json TEXT NOT NULL DEFAULT '[]',
  impact_json TEXT, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, version INTEGER NOT NULL DEFAULT 1);
CREATE TABLE IF NOT EXISTS playbook_versions(version INTEGER PRIMARY KEY AUTOINCREMENT, ts INTEGER NOT NULL, note TEXT NOT NULL, snapshot TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS job_edits(job_id TEXT NOT NULL, path TEXT NOT NULL, text TEXT NOT NULL, ts INTEGER NOT NULL, PRIMARY KEY(job_id,path));
CREATE TABLE IF NOT EXISTS golden(id INTEGER PRIMARY KEY AUTOINCREMENT, job_id TEXT NOT NULL UNIQUE, jd_text TEXT NOT NULL, jd_family TEXT NOT NULL, jd_seniority TEXT NOT NULL DEFAULT '',
  company TEXT NOT NULL DEFAULT '', final_selection_json TEXT NOT NULL, approved_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS regression_runs(id INTEGER PRIMARY KEY AUTOINCREMENT, ts INTEGER NOT NULL, result_json TEXT NOT NULL);";

const COLS: &str = "id,status,scope,area,matcher_json,action_json,condition_json,text,origin,support_count,examples_json,impact_json,created_at,updated_at,version";
const TAGS: [&str; 10] = ["irrelevant", "too_generic", "outdated", "too_long", "too_short", "wrong_tone", "duplicate", "wrong_order", "already_covered", "other"];
const SEED_BADGE: &str = "From your 2026-10-03 feedback";

/// Seed rules (inserted once, owner can disable) and the first snapshot.
pub fn seed(c: &Connection) -> Result<()> {
    for r in pb::seed_rules() {
        if c.query_row("SELECT 1 FROM playbook_rules WHERE id=?", [&r.id], |_| Ok(())).optional()?.is_none() {
            insert(c, &r, &json!([]))?;
        }
    }
    if c.query_row("SELECT 1 FROM playbook_versions", [], |_| Ok(())).optional()?.is_none() {
        snapshot(c, "seed rules")?;
    }
    Ok(())
}

// ---- rule store -------------------------------------------------------------------------------------------------

fn insert(c: &Connection, r: &Rule, examples: &Value) -> Result<()> {
    c.execute("INSERT INTO playbook_rules(id,status,scope,area,matcher_json,action_json,condition_json,text,origin,support_count,examples_json,created_at,updated_at,version) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?12,?13)",
        params![r.id, r.status, r.scope, r.area, serde_json::to_string(&r.matcher)?, serde_json::to_string(&r.action)?, serde_json::to_string(&r.cond)?, r.text, r.origin, r.support_count, examples.to_string(), now(), r.version])?;
    Ok(())
}

fn update(c: &Connection, r: &Rule) -> Result<()> {
    c.execute("UPDATE playbook_rules SET status=?2,scope=?3,matcher_json=?4,action_json=?5,condition_json=?6,text=?7,updated_at=?8,version=?9 WHERE id=?1",
        params![r.id, r.status, r.scope, serde_json::to_string(&r.matcher)?, serde_json::to_string(&r.action)?, serde_json::to_string(&r.cond)?, r.text, now(), r.version])?;
    Ok(())
}

fn row_rule(r: &rusqlite::Row) -> rusqlite::Result<Rule> {
    let j = |i: usize| -> rusqlite::Result<String> { r.get(i) };
    Ok(Rule { id: r.get(0)?, status: r.get(1)?, scope: r.get(2)?, area: r.get(3)?, matcher: serde_json::from_str(&j(4)?).unwrap_or_default(), action: serde_json::from_str(&j(5)?).unwrap_or_default(),
        cond: serde_json::from_str(&j(6)?).unwrap_or_default(), text: r.get(7)?, origin: r.get(8)?, support_count: r.get(9)?, version: r.get(10)? })
}

const RULE_SQL: &str = "id,status,scope,area,matcher_json,action_json,condition_json,text,origin,support_count,version";

pub fn all_rules(c: &Connection) -> Result<Vec<Rule>> {
    Ok(c.prepare(&format!("SELECT {RULE_SQL} FROM playbook_rules ORDER BY created_at,rowid"))?.query_map([], row_rule)?.collect::<rusqlite::Result<_>>()?)
}

fn get_rule(c: &Connection, id: &str) -> Result<Option<Rule>> {
    Ok(c.query_row(&format!("SELECT {RULE_SQL} FROM playbook_rules WHERE id=?"), [id], row_rule).optional()?)
}

fn snapshot(c: &Connection, note: &str) -> Result<i64> {
    let obj: Vec<String> = COLS.split(',').map(|k| format!("'{k}',{k}")).collect();
    let snap: String = c.query_row(&format!("SELECT coalesce(json_group_array(json_object({})),'[]') FROM playbook_rules", obj.join(",")), [], |r| r.get(0))?;
    c.execute("INSERT INTO playbook_versions(ts,note,snapshot) VALUES(?,?,?)", params![now(), note, snap])?;
    Ok(c.last_insert_rowid())
}

/// Restore a snapshot. Rules mined after it (still proposed or rejected) are kept; everything else follows the snapshot.
fn rollback(c: &Connection, version: i64) -> Result<bool> {
    let Some(snap): Option<String> = c.query_row("SELECT snapshot FROM playbook_versions WHERE version=?", [version], |r| r.get(0)).optional()? else { return Ok(false) };
    c.execute("DELETE FROM playbook_rules WHERE status NOT IN('proposed','rejected') AND id NOT IN(SELECT json_extract(value,'$.id') FROM json_each(?1))", [&snap])?;
    let sel: Vec<String> = COLS.split(',').map(|k| format!("json_extract(value,'$.{k}')")).collect();
    c.execute(&format!("INSERT OR REPLACE INTO playbook_rules({COLS}) SELECT {} FROM json_each(?1)", sel.join(",")), [&snap])?;
    snapshot(c, &format!("rolled back to v{version}"))?;
    Ok(true)
}

/// Rule text must be free of the owner's private values (vault guard) and of email/link/phone-like content.
fn check_text(c: &Connection, a: &App, text: &str) -> std::result::Result<(), E> {
    let resume: Resume = c.query_row("SELECT json FROM resume WHERE id=1", [], |r| r.get::<_, String>(0)).optional().ok().flatten().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    let extras: Vec<(Kind, String)> = crate::always_rows(c, &a.key_file).unwrap_or_default().into_iter().filter_map(|(_, k, v)| Some((crate::kind_from(&k)?, v))).collect();
    Vault::from_resume(&resume, &extras).guard(text).map_err(|_| bad("rule text contains private values; rewrite it without names or contact details"))
}

fn check(c: &Connection, a: &App, r: &Rule) -> std::result::Result<(), E> {
    pb::validate(r).map_err(bad)?;
    check_text(c, a, &r.text)
}

// ---- job context ------------------------------------------------------------------------------------------------

/// Everything learned that shapes one job's run: active rules, the JD facts they key on, the owner's persistent edits.
#[derive(Default, Clone)]
pub struct Learned {
    pub rules: Vec<Rule>,
    pub jdc: JdContext,
    pub edits: Vec<(String, String)>,
}

/// (family, seniority): the owner's confirmed label, else the local classifier's top guess.
pub fn family_of(c: &Connection, id: &str, text: &str, cls: Option<&JdClassifier>) -> (String, String) {
    let label: Option<Value> = c.query_row("SELECT label_json FROM job_ml WHERE job_id=?", [id], |r| r.get::<_, Option<String>>(0)).optional().ok().flatten().flatten().and_then(|l| serde_json::from_str(&l).ok());
    if let Some(l) = label {
        return (l["family"].as_str().unwrap_or("").into(), l["seniority"].as_str().unwrap_or("").into());
    }
    let Some(m) = cls else { return Default::default() };
    let k = m.classify(text);
    let top = |v: &[(String, f32)]| v.first().map(|x| x.0.clone()).unwrap_or_default();
    (top(&k.family), top(&k.seniority))
}

pub fn load(c: &Connection, id: &str, jd_text: &str, company: &str, cls: Option<&JdClassifier>) -> Learned {
    let (family, seniority) = family_of(c, id, jd_text, cls);
    let edits = c.prepare("SELECT path,text FROM job_edits WHERE job_id=?").and_then(|mut s| s.query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()).unwrap_or_default();
    Learned { rules: all_rules(c).unwrap_or_default().into_iter().filter(|r| r.status == "active").collect(), jdc: JdContext { family, seniority, company: company.into(), text: jd_text.to_lowercase() }, edits }
}

/// Persistent per-job edits: `summary.K` or `e<i>.b<j>` on the full (un-trimmed) resume. False when the line does not exist.
fn slot<'a>(r: &'a mut Resume, path: &str) -> Option<&'a mut String> {
    match path.split_once('.') {
        Some(("summary", k)) => k.parse::<usize>().ok().and_then(|k| r.summary.get_mut(k)),
        Some((e, b)) => e.strip_prefix('e').and_then(|e| e.parse::<usize>().ok()).zip(b.strip_prefix('b').and_then(|b| b.parse::<usize>().ok())).and_then(|(e, b)| r.experience.get_mut(e)?.bullets.get_mut(b)),
        None => None,
    }
}

pub fn apply_edit(r: &mut Resume, path: &str, text: &str) -> bool {
    slot(r, path).map(|s| *s = text.to_string()).is_some()
}

pub fn apply_edits(r: &mut Resume, edits: &[(String, String)]) {
    edits.iter().for_each(|(p, t)| { apply_edit(r, p, t); });
}

/// `[{rule_id,text,effect}]`, one entry per rule that did something.
pub fn applied(rules: &[Rule], hits: &[RuleHit]) -> Value {
    let mut by: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    hits.iter().for_each(|h| by.entry(&h.rule_id).or_default().push(&h.effect));
    json!(by.into_iter().map(|(id, e)| json!({"rule_id": id, "text": rules.iter().find(|r| r.id == id).map(|r| r.text.as_str()).unwrap_or(""), "effect": e.join("; ")})).collect::<Vec<_>>())
}

/// Texts for the writer prompt (summary/bullets rules) as a ready-to-append section; empty when none.
pub fn writer_section(cx: &Ctx) -> String {
    let t = pb::texts(&cx.learn.rules, &cx.learn.jdc, &["summary", "bullets"]);
    if t.is_empty() { String::new() } else { format!("\n\nOwner rules (must follow):\n{}", t.iter().map(|x| format!("- {}", cx.vault.redact(x))).collect::<Vec<_>>().join("\n")) }
}

/// Texts for the plan-review prompt (certs/projects/skills/roles rules), redacted.
pub fn review_section(cx: &Ctx) -> String {
    let t = pb::texts(&cx.learn.rules, &cx.learn.jdc, &["certs", "projects", "skills", "roles"]);
    if t.is_empty() { String::new() } else { format!("\n\nOwner rules (must follow):\n{}", t.iter().map(|x| format!("- {}", cx.vault.redact(x))).collect::<Vec<_>>().join("\n")) }
}

// ---- corrections ------------------------------------------------------------------------------------------------

pub struct Info {
    pub family: String,
    pub seniority: String,
    pub company: String,
}

pub fn info(a: &App, c: &Connection, id: &str) -> Result<Info> {
    let (text, company): (String, Option<String>) = c.query_row("SELECT jd_text,company FROM jobs WHERE id=?", [id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    let (family, seniority) = family_of(c, id, &text, Some(&a.ml.lock().unwrap().jd));
    Ok(Info { family, seniority, company: company.unwrap_or_default() })
}

#[allow(clippy::too_many_arguments)]
pub fn record(c: &Connection, job: &str, i: &Info, area: &str, action: &str, item_id: &str, item: &Value, before: &Value, after: &Value, weight: f64) -> Result<i64> {
    c.execute("INSERT INTO corrections(job_id,ts,area,action,item_id,item_json,before_json,after_json,jd_family,seniority,company,weight,source) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,'owner')",
        params![job, now(), area, action, item_id, item.to_string(), before.to_string(), after.to_string(), i.family, i.seniority, i.company, weight])?;
    Ok(c.last_insert_rowid())
}

/// One correction per project/cert/role the owner added or removed relative to the AI/local plan (`prev` -> `new`),
/// plus one reorder for the skills order.
pub fn record_overrides(a: &App, c: &Connection, id: &str, r: &Resume, prev: &Selection, new: &Selection, ap: &[Applied]) -> Result<()> {
    let i = info(a, c, id)?;
    for x in ap.iter().filter(|x| x.ok) {
        if x.part == "skills_order" {
            let ids = |s: &Selection| s.skills.iter().map(|k| r.skills.get(k.0).map(|k| k.label.clone()).unwrap_or_default()).collect::<Vec<_>>();
            if prev.skills.iter().map(|k| k.0).ne(new.skills.iter().map(|k| k.0)) {
                record(c, id, &i, "skills", "reorder", "skills_order", &json!({"kind": "skill"}), &json!(ids(prev)), &json!(ids(new)), 1.0)?;
            }
            continue;
        }
        for (ids, action) in [(&x.added, "add"), (&x.removed, "remove")] {
            for pid in ids {
                let n: usize = pid[1..].parse()?;
                let item = match x.part {
                    "projects" => r.projects.get(n).map(|p| Cand::project(n, p)),
                    "certs" => r.certifications.get(n).map(|k| Cand::cert(n, k)),
                    _ => r.experience.get(n).map(|e| Cand::role(n, e)),
                };
                if let Some(item) = item {
                    record(c, id, &i, x.part, action, pid, &json!(item), &json!({"selected": action == "remove"}), &json!({"selected": action == "add"}), 1.0)?;
                }
            }
        }
    }
    Ok(())
}

fn correction_json(r: &rusqlite::Row) -> rusqlite::Result<Value> {
    let p = |i: usize| -> rusqlite::Result<Value> { Ok(r.get::<_, Option<String>>(i)?.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null)) };
    Ok(json!({"id": r.get::<_, i64>(0)?, "job_id": r.get::<_, String>(1)?, "ts": r.get::<_, i64>(2)?, "area": r.get::<_, String>(3)?, "action": r.get::<_, String>(4)?, "item_id": r.get::<_, String>(5)?, "item": p(6)?,
        "before": p(7)?, "after": p(8)?, "reason_tags": p(9)?, "jd_family": r.get::<_, String>(10)?, "seniority": r.get::<_, String>(11)?, "company": r.get::<_, String>(12)?, "weight": r.get::<_, f64>(13)?, "source": r.get::<_, String>(14)?}))
}
const CORR_SQL: &str = "SELECT id,job_id,ts,area,action,item_id,item_json,before_json,after_json,reason_tags,jd_family,seniority,company,weight,source FROM corrections";

/// Job-detail additions: (corrections, rules_applied, approved).
pub fn detail(c: &Connection, id: &str, outs: &HashMap<String, Value>) -> Result<(Value, Value, bool)> {
    let cs: Vec<Value> = c.prepare(&format!("{CORR_SQL} WHERE job_id=? ORDER BY id"))?.query_map([id], correction_json)?.collect::<rusqlite::Result<_>>()?;
    let approved = c.query_row("SELECT 1 FROM golden WHERE job_id=?", [id], |_| Ok(())).optional()?.is_some();
    Ok((json!(cs), outs.get("select").map(|s| s["rules"].clone()).filter(|v| !v.is_null()).unwrap_or(json!([])), approved))
}

// ---- mining -----------------------------------------------------------------------------------------------------

impl App {
    /// Mine proposals in the background after a correction batch.
    pub fn mine_bg(self: &Arc<Self>) {
        let me = self.clone();
        tokio::spawn(async move {
            if let Err(e) = me.mine().await {
                eprintln!("playbook mining: {e}");
            }
        });
    }

    /// Propose rules from the corrections (never activates them). Returns the number of new proposals.
    pub async fn mine(self: &Arc<Self>) -> Result<usize> {
        self.db(|c| {
            let cs: Vec<Correction> = c.prepare("SELECT job_id,area,action,item_json,jd_family FROM corrections WHERE area IN('certs','projects')")?
                .query_map([], |r| Ok(Correction { job_id: r.get(0)?, area: r.get(1)?, action: r.get(2)?, item: serde_json::from_str(&r.get::<_, String>(3)?).unwrap_or_default(), jd_family: r.get(4)? }))?.collect::<rusqlite::Result<_>>()?;
            let ps = pb::mine_rules(&cs, &all_rules(c)?);
            for mut p in ps.iter().cloned() {
                p.rule.id = format!("mined-{}", uuid::Uuid::new_v4());
                insert(c, &p.rule, &json!(p.examples))?;
                realtime::emit(c, &realtime::Ev { kind: "notice", level: "info", code: Some("playbook.proposed"), title: "New rule suggestion", message: &p.rule.text, local: true, notify: true,
                    actions: Some(json!([{"label": "Open playbook", "action": "open", "target": "/playbook"}])), ..Default::default() })?;
            }
            Ok(ps.len())
        }).await
    }
}

// ---- golden set / regression (local pipeline only) --------------------------------------------------------------

fn sel_items(r: &Resume, s: &Selection) -> Value {
    json!({
        "projects": s.projects.iter().filter_map(|&i| r.projects.get(i).map(|p| json!({"id": format!("p{i}"), "title": p.name}))).collect::<Vec<_>>(),
        "certs": s.certs.iter().filter_map(|&i| r.certifications.get(i).map(|k| json!({"id": format!("c{i}"), "title": k.title}))).collect::<Vec<_>>(),
        "roles": s.bullets.iter().enumerate().filter(|(_, b)| !b.is_empty()).filter_map(|(i, _)| r.experience.get(i).map(|e| json!({"id": format!("e{i}"), "title": format!("{} @ {}", e.role, e.organization)}))).collect::<Vec<_>>(),
        "skills": s.skills.iter().filter_map(|k| r.skills.get(k.0).map(|c| json!({"id": format!("s{}", k.0), "title": c.label}))).collect::<Vec<_>>(),
    })
}

struct Golden {
    job_id: String,
    jd_text: String,
    jdc: (String, String, String),
    fin: Value,
}

fn goldens(c: &Connection) -> Result<Vec<Golden>> {
    Ok(c.prepare("SELECT job_id,jd_text,jd_family,jd_seniority,company,final_selection_json FROM golden ORDER BY id")?
        .query_map([], |r| Ok(Golden { job_id: r.get(0)?, jd_text: r.get(1)?, jdc: (r.get(2)?, r.get(3)?, r.get(4)?), fin: serde_json::from_str(&r.get::<_, String>(5)?).unwrap_or_default() }))?.collect::<rusqlite::Result<_>>()?)
}

/// parse_jd + select with `rules` and the embedder; no AI, no rendering, no ML reranking.
fn replay(c: &Connection, env: &DetailEnv, resume_json: &str, rules: &[Rule], g: &Golden) -> Result<Value> {
    let (pages, role, company): (Option<i64>, Option<String>, Option<String>) = c.query_row("SELECT pages,role,company FROM jobs WHERE id=?", [&g.job_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).optional()?.unwrap_or_default();
    let jdc = JdContext { family: g.jdc.0.clone(), seniority: g.jdc.1.clone(), company: g.jdc.2.clone(), text: g.jd_text.to_lowercase() };
    let cx = crate::ctx_with(resume_json, &g.jd_text, &[], Opts { pages: pages.map(|p| p as u8), prior: env.prior, h: env.h.clone(), emb: env.emb.clone(), role, company, learn: Learned { rules: rules.iter().filter(|r| r.status == "active").cloned().collect(), jdc, edits: vec![] }, ..Default::default() })?;
    // the same local lint fixes and rule enforcement the review step applies, minus the AI
    let mut sel = cx.sel.clone();
    for i in resume_core::review::lint(&cx.resume, &sel.clone(), &cx.jd, &cx.decisions, cx.emb.as_deref(), Some(&cx.facts), false) {
        if let Some(f) = &i.auto_fix {
            f.apply(&mut sel);
        }
    }
    pb::enforce(&cx.adj, &cx.resume, &mut sel, cx.decisions.max_certs, cx.decisions.max_projects);
    Ok(sel_items(&cx.resume, &sel))
}

fn titles(v: &Value, k: &str) -> Vec<String> {
    v[k].as_array().into_iter().flatten().map(|x| x["title"].as_str().unwrap_or("").to_string()).collect()
}

/// {} when projects/certs/roles match exactly (set and order), else per area the missing/extra titles and whether only the order differs.
fn diff(want: &Value, got: &Value) -> Value {
    let mut d = serde_json::Map::new();
    for k in ["projects", "certs", "roles"] {
        let (w, g) = (titles(want, k), titles(got, k));
        if w != g {
            d.insert(k.into(), json!({"missing": w.iter().filter(|x| !g.contains(x)).collect::<Vec<_>>(), "extra": g.iter().filter(|x| !w.contains(x)).collect::<Vec<_>>(), "order_only": w.len() == g.len() && w.iter().all(|x| g.contains(x))}));
        }
    }
    Value::Object(d)
}

fn run_regression(c: &Connection, env: &DetailEnv, rules: &[Rule]) -> Result<Value> {
    let rj: Option<String> = c.query_row("SELECT json FROM resume WHERE id=1", [], |r| r.get(0)).optional()?;
    let rj = rj.ok_or_else(|| anyhow::anyhow!("no resume stored"))?;
    let mut results = vec![];
    for g in goldens(c)? {
        let d = diff(&g.fin, &replay(c, env, &rj, rules, &g)?);
        results.push(json!({"job_id": g.job_id, "pass": d.as_object().is_some_and(|o| o.is_empty()), "diff": d}));
    }
    let passing = results.iter().filter(|r| r["pass"] == true).count();
    Ok(json!({"ran_at": now(), "cases": results.len(), "passing": passing, "results": results}))
}

/// How many golden resumes change if `rule` were active on top of the active rules.
fn impact(c: &Connection, env: &DetailEnv, rule: &Rule) -> Result<Value> {
    let rj: Option<String> = c.query_row("SELECT json FROM resume WHERE id=1", [], |r| r.get(0)).optional()?;
    let Some(rj) = rj else { return Ok(json!({"would_change": 0, "of": 0, "cases": [], "summary": "no resume stored"})) };
    let base: Vec<Rule> = all_rules(c)?.into_iter().filter(|r| r.status == "active").collect();
    let with: Vec<Rule> = base.iter().cloned().chain(std::iter::once(Rule { status: "active".into(), ..rule.clone() })).collect();
    let (mut changed, gs) = (vec![], goldens(c)?);
    for g in &gs {
        if replay(c, env, &rj, &base, g)? != replay(c, env, &rj, &with, g)? {
            changed.push(g.job_id.clone());
        }
    }
    Ok(json!({"would_change": changed.len(), "of": gs.len(), "cases": changed, "summary": format!("would change {} of {} approved resumes", changed.len(), gs.len())}))
}

// ---- endpoints --------------------------------------------------------------------------------------------------

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/jobs/:id/corrections", get(job_corrections))
        .route("/api/jobs/:id/edit", post(edit))
        .route("/api/jobs/:id/approve", post(approve))
        .route("/api/corrections/:id/reason", post(reason))
        .route("/api/playbook", get(playbook))
        .route("/api/playbook/rules", post(create_rule))
        .route("/api/playbook/rules/:id", axum::routing::put(put_rule).delete(del_rule))
        .route("/api/playbook/rules/:id/:op", post(rule_op))
        .route("/api/playbook/rollback", post(do_rollback))
        .route("/api/regression/run", post(regression_run))
        .route("/api/regression/latest", get(regression_latest))
        .route("/api/learning/metrics", get(metrics))
}

async fn run(a: &Arc<App>, f: impl FnOnce(&mut Connection) -> Result<R> + Send + 'static) -> std::result::Result<Json<Value>, E> {
    match a.db(f).await {
        Ok(Ok(v)) => Ok(Json(v)),
        Ok(Err(e)) => Err(e),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string())),
    }
}

async fn job_corrections(State(a): State<Arc<App>>, Path(id): Path<String>) -> std::result::Result<Json<Value>, E> {
    run(&a, move |c| {
        let v: Vec<Value> = c.prepare(&format!("{CORR_SQL} WHERE job_id=? ORDER BY id"))?.query_map([id], correction_json)?.collect::<rusqlite::Result<_>>()?;
        Ok(Ok(json!(v)))
    }).await
}

fn tags_of(b: &Value) -> std::result::Result<Vec<String>, E> {
    let t: Vec<String> = b.as_array().into_iter().flatten().filter_map(|x| x.as_str().map(String::from)).collect();
    match t.iter().find(|x| !TAGS.contains(&x.as_str())) {
        Some(x) => Err(bad(format!("unknown reason tag {x}; use one of {TAGS:?}"))),
        None => Ok(t),
    }
}

async fn reason(State(a): State<Arc<App>>, Path(id): Path<i64>, Json(b): Json<Value>) -> std::result::Result<Json<Value>, E> {
    let tags = tags_of(&b["tags"])?;
    run(&a, move |c| Ok(if c.execute("UPDATE corrections SET reason_tags=? WHERE id=?", params![json!(tags).to_string(), id])? == 0 { Err(nf("no such correction")) } else { Ok(json!({"id": id, "reason_tags": tags})) })).await
}

/// Persistent per-job edit of one line; it is applied after every (re-)render and recorded as a labelled correction.
async fn edit(State(a): State<Arc<App>>, Path(id): Path<String>, Json(b): Json<Value>) -> std::result::Result<Json<Value>, E> {
    let path = b["path"].as_str().filter(|p| regex::Regex::new(r"^(summary\.\d+|e\d+\.b\d+)$").unwrap().is_match(p)).ok_or_else(|| bad("path must be summary.<n> or e<i>.b<j>"))?.to_string();
    let text = b["text"].as_str().map(str::trim).filter(|t| !t.is_empty() && t.chars().count() <= 2000).ok_or_else(|| bad("text must be 1-2000 characters"))?.to_string();
    let tags = tags_of(&b["reason_tags"])?;
    let (env, me) = (DetailEnv::of(&a), a.clone());
    let out = run(&a, move |c| {
        let row: Option<(Option<String>, String, String, String)> = c.query_row("SELECT (SELECT json FROM resume WHERE id=1),jd_text,extra_redact_json,status FROM jobs WHERE id=?", [&id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).optional()?;
        let Some((Some(rj), jd, extra, status)) = row else { return Ok(Err(nf("no such job"))) };
        if !matches!(status.as_str(), "failed" | "dead" | "cancelled" | "done") {
            return Ok(Err(conflict("job is still running; wait for it to finish")));
        }
        let outs: HashMap<String, Value> = c.prepare("SELECT name,output_json FROM job_steps WHERE job_id=? AND name IN('select','ai_review','ai_tailor','restore') AND status='done' AND output_json IS NOT NULL")?
            .query_map([&id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?.into_iter().filter_map(|(n, o)| Some((n, serde_json::from_str(&o).ok()?))).collect();
        let mut cx = env.ctx(c, &id, &rj, &jd, &serde_json::from_str::<Vec<String>>(&extra)?, outs.get("select"))?;
        if let Some(o) = outs.get("ai_review") {
            apply_select(&mut cx, o);
        }
        let mut cur = if outs.contains_key("ai_tailor") && outs.contains_key("restore") {
            crate::restored_full(&cx, &outs.iter().map(|(k, v)| (k.as_str(), v.clone())).collect()).map_err(|e| anyhow::anyhow!("{e:?}"))?.0
        } else {
            cx.resume.clone()
        };
        let Some(old) = slot(&mut cur, &path).map(|s| s.clone()) else { return Ok(Err(bad("no such line in this resume"))) };
        c.execute("INSERT INTO job_edits(job_id,path,text,ts) VALUES(?1,?2,?3,?4) ON CONFLICT(job_id,path) DO UPDATE SET text=?3,ts=?4", params![id, path, text, now()])?;
        let i = info(&me, c, &id)?;
        let area = if path.starts_with("summary") { "summary" } else { "bullets" };
        let cid = record(c, &id, &i, area, "edit", &path, &json!({"kind": area, "path": path}), &json!({"text": old}), &json!({"text": text}), 1.0)?;
        if !tags.is_empty() {
            c.execute("UPDATE corrections SET reason_tags=? WHERE id=?", params![json!(tags).to_string(), cid])?;
        }
        retry_job(c, &id, "render_docx")?;
        Ok(Ok(json!({"correction_id": cid, "path": path, "before": old, "after": text})))
    }).await?;
    a.mine_bg();
    Ok(out)
}

/// The owner marks this job's final selection: stored as a golden case, items they left in place are positive signals.
async fn approve(State(a): State<Arc<App>>, Path(id): Path<String>) -> std::result::Result<Json<Value>, E> {
    let (env, me) = (DetailEnv::of(&a), a.clone());
    let out = run(&a, move |c| {
        let row: Option<(Option<String>, String, String, String)> = c.query_row("SELECT (SELECT json FROM resume WHERE id=1),jd_text,extra_redact_json,status FROM jobs WHERE id=?", [&id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).optional()?;
        let Some((Some(rj), jd, extra, status)) = row else { return Ok(Err(nf("no such job"))) };
        if status != "done" {
            return Ok(Err(conflict("only a finished job can be approved")));
        }
        let outs: HashMap<String, Value> = c.prepare("SELECT name,output_json FROM job_steps WHERE job_id=? AND name IN('select','ai_review') AND status='done' AND output_json IS NOT NULL")?
            .query_map([&id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?.into_iter().filter_map(|(n, o)| Some((n, serde_json::from_str(&o).ok()?))).collect();
        let mut cx = env.ctx(c, &id, &rj, &jd, &serde_json::from_str::<Vec<String>>(&extra)?, outs.get("select"))?;
        if let Some(o) = outs.get("ai_review") {
            apply_select(&mut cx, o);
        }
        let fin = sel_items(&cx.resume, &cx.sel);
        let i = info(&me, c, &id)?;
        c.execute("INSERT INTO golden(job_id,jd_text,jd_family,jd_seniority,company,final_selection_json,approved_at) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(job_id) DO UPDATE SET jd_text=?2,jd_family=?3,jd_seniority=?4,company=?5,final_selection_json=?6,approved_at=?7",
            params![id, jd, i.family, i.seniority, i.company, fin.to_string(), now()])?;
        c.execute("DELETE FROM corrections WHERE job_id=? AND action='keep'", [&id])?;
        let mut keeps = 0;
        let kept = cx.sel.certs.iter().filter_map(|&n| cx.resume.certifications.get(n).map(|k| Cand::cert(n, k))).chain(cx.sel.projects.iter().filter_map(|&n| cx.resume.projects.get(n).map(|p| Cand::project(n, p))));
        for k in kept {
            record(c, &id, &i, if k.kind == "cert" { "certs" } else { "projects" }, "keep", &k.id, &json!(k), &Value::Null, &json!({"selected": true}), 0.5)?;
            keeps += 1;
        }
        Ok(Ok(json!({"approved": true, "golden": fin, "keep_corrections": keeps})))
    }).await?;
    a.mine_bg();
    Ok(out)
}

fn rule_json(c: &Connection, id: &str) -> Result<Option<Value>> {
    let hits = rule_hits(c)?;
    Ok(c.query_row(&format!("SELECT {RULE_SQL},examples_json,impact_json,created_at,updated_at FROM playbook_rules WHERE id=?"), [id], |r| Ok((row_rule(r)?, r.get::<_, String>(11)?, r.get::<_, Option<String>>(12)?, r.get::<_, i64>(13)?, r.get::<_, i64>(14)?))).optional()?
        .map(|(rule, ex, imp, created, updated)| {
            let mut v = json!(rule);
            let p = |s: &str| serde_json::from_str::<Value>(s).unwrap_or(Value::Null);
            v["condition"] = v.as_object_mut().and_then(|o| o.remove("cond")).unwrap_or_default();
            v["examples"] = p(&ex);
            v["impact"] = imp.map(|i| p(&i)).unwrap_or(Value::Null);
            v["created_at"] = json!(created);
            v["updated_at"] = json!(updated);
            v["hits"] = json!(hits.get(id).copied().unwrap_or(0));
            v["badge"] = if rule.origin == "seed" { json!(SEED_BADGE) } else { Value::Null };
            v
        }))
}

/// Times each rule fired across stored select steps.
fn rule_hits(c: &Connection) -> Result<HashMap<String, i64>> {
    let mut m = HashMap::new();
    for o in c.prepare("SELECT output_json FROM job_steps WHERE name='select' AND status='done'")?.query_map([], |r| r.get::<_, Option<String>>(0))?.flatten().flatten() {
        for r in serde_json::from_str::<Value>(&o).ok().and_then(|v| v["rules"].as_array().cloned()).unwrap_or_default() {
            *m.entry(r["rule_id"].as_str().unwrap_or("").to_string()).or_default() += 1;
        }
    }
    Ok(m)
}

fn playbook_state(c: &Connection) -> Result<Value> {
    let ids: Vec<(String, String)> = c.prepare("SELECT id,status FROM playbook_rules ORDER BY created_at,rowid")?.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
    let mut rules = vec![];
    let mut groups: BTreeMap<String, Vec<String>> = ["proposed", "active", "disabled", "rejected"].iter().map(|s| (s.to_string(), vec![])).collect();
    for (id, st) in ids {
        rules.extend(rule_json(c, &id)?);
        groups.entry(st).or_default().push(id);
    }
    let versions: Vec<Value> = c.prepare("SELECT version,ts,note FROM playbook_versions ORDER BY version DESC")?.query_map([], |r| Ok(json!({"version": r.get::<_, i64>(0)?, "ts": r.get::<_, i64>(1)?, "note": r.get::<_, String>(2)?})))?.collect::<rusqlite::Result<_>>()?;
    Ok(json!({"rules": rules, "groups": groups, "versions": versions}))
}

async fn playbook(State(a): State<Arc<App>>) -> std::result::Result<Json<Value>, E> {
    run(&a, |c| Ok(Ok(playbook_state(c)?))).await
}

/// Rule from a request body (`text`, `area`, `scope`, `matcher`, `action`, `condition`).
fn rule_from(b: &Value, base: Rule) -> std::result::Result<Rule, E> {
    let mut r = base;
    if let Some(t) = b["text"].as_str() { r.text = t.trim().into(); }
    if let Some(t) = b["area"].as_str() { r.area = t.into(); }
    if let Some(t) = b["scope"].as_str() { r.scope = t.into(); }
    if !b["matcher"].is_null() { r.matcher = serde_json::from_value::<Matcher>(b["matcher"].clone()).map_err(bad)?; }
    if !b["action"].is_null() { r.action = serde_json::from_value::<Action>(b["action"].clone()).map_err(bad)?; }
    if !b["condition"].is_null() { r.cond = serde_json::from_value::<Cond>(b["condition"].clone()).map_err(bad)?; }
    Ok(r)
}

async fn create_rule(State(a): State<Arc<App>>, Json(b): Json<Value>) -> std::result::Result<Json<Value>, E> {
    let r = rule_from(&b, Rule { id: format!("user-{}", uuid::Uuid::new_v4()), status: "active".into(), scope: "global".into(), origin: "user".into(), version: 1, ..Default::default() })?;
    let me = a.clone();
    run(&a, move |c| {
        if let Err(e) = check(c, &me, &r) { return Ok(Err(e)); }
        insert(c, &r, &json!([]))?;
        snapshot(c, &format!("added rule {}", r.id))?;
        Ok(Ok(rule_json(c, &r.id)?.unwrap_or_default()))
    }).await
}

async fn put_rule(State(a): State<Arc<App>>, Path(id): Path<String>, Json(b): Json<Value>) -> std::result::Result<Json<Value>, E> {
    let me = a.clone();
    run(&a, move |c| {
        let Some(old) = get_rule(c, &id)? else { return Ok(Err(nf("no such rule"))) };
        let r = match rule_from(&b, Rule { version: old.version + 1, ..old }) { Ok(r) => r, Err(e) => return Ok(Err(e)) };
        if let Err(e) = check(c, &me, &r) { return Ok(Err(e)); }
        update(c, &r)?;
        snapshot(c, &format!("edited rule {id}"))?;
        Ok(Ok(rule_json(c, &id)?.unwrap_or_default()))
    }).await
}

async fn del_rule(State(a): State<Arc<App>>, Path(id): Path<String>) -> std::result::Result<Json<Value>, E> {
    run(&a, move |c| {
        let Some(r) = get_rule(c, &id)? else { return Ok(Err(nf("no such rule"))) };
        if r.origin == "seed" {
            return Ok(Err(conflict("seed rules can be disabled, not deleted")));
        }
        c.execute("DELETE FROM playbook_rules WHERE id=?", [&id])?;
        snapshot(c, &format!("deleted rule {id}"))?;
        Ok(Ok(json!({"deleted": id})))
    }).await
}

async fn rule_op(State(a): State<Arc<App>>, Path((id, op)): Path<(String, String)>) -> std::result::Result<Json<Value>, E> {
    let (env, me) = (DetailEnv::of(&a), a.clone());
    run(&a, move |c| {
        let Some(mut r) = get_rule(c, &id)? else { return Ok(Err(nf("no such rule"))) };
        let to = match (op.as_str(), r.status.as_str()) {
            ("approve", "proposed") => "active",
            ("reject", "proposed") => "rejected",
            ("disable", "active") => "disabled",
            ("enable", "disabled" | "rejected") => "active",
            ("approve" | "reject" | "disable" | "enable", s) => return Ok(Err(conflict(&format!("cannot {op} a {s} rule")))),
            _ => return Ok(Err(nf("unknown operation"))),
        };
        let mut imp = Value::Null;
        if to == "active" {
            if let Err(e) = check(c, &me, &r) { return Ok(Err(e)); }
            imp = impact(c, &env, &r)?; // replayed on the golden set before the rule goes live
            c.execute("UPDATE playbook_rules SET impact_json=? WHERE id=?", params![imp.to_string(), id])?;
        }
        (r.status, r.version) = (to.into(), r.version + 1);
        update(c, &r)?;
        snapshot(c, &format!("{op} rule {id}"))?;
        Ok(Ok(json!({"rule": rule_json(c, &id)?, "impact": imp})))
    }).await
}

async fn do_rollback(State(a): State<Arc<App>>, Json(b): Json<Value>) -> std::result::Result<Json<Value>, E> {
    let v = b["version"].as_i64().ok_or_else(|| bad("version required"))?;
    run(&a, move |c| Ok(if rollback(c, v)? { Ok(playbook_state(c)?) } else { Err(nf("no such version")) })).await
}

async fn regression_run(State(a): State<Arc<App>>) -> std::result::Result<Json<Value>, E> {
    let env = DetailEnv::of(&a);
    run(&a, move |c| {
        let rules = all_rules(c)?;
        let v = match run_regression(c, &env, &rules) { Ok(v) => v, Err(e) => return Ok(Err(bad(e))) };
        c.execute("INSERT INTO regression_runs(ts,result_json) VALUES(?,?)", params![now(), v.to_string()])?;
        Ok(Ok(v))
    }).await
}

async fn regression_latest(State(a): State<Arc<App>>) -> std::result::Result<Json<Value>, E> {
    run(&a, |c| Ok(Ok(c.query_row("SELECT result_json FROM regression_runs ORDER BY id DESC LIMIT 1", [], |r| r.get::<_, String>(0)).optional()?.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null)))).await
}

async fn metrics(State(a): State<Arc<App>>) -> std::result::Result<Json<Value>, E> {
    run(&a, |c| {
        let tok = |k: &str| format!("coalesce((SELECT sum(json_extract(output_json,'$.{k}')) FROM job_steps s WHERE s.job_id=j.id AND s.name IN('ai_tailor','ai_review') AND s.status='done' AND coalesce(json_extract(output_json,'$.cached'),0)=0),0)");
        let jobs: Vec<Value> = c.prepare(&format!("SELECT j.id,j.created_at,coalesce(j.company,'Unknown'),(SELECT count(*) FROM corrections x WHERE x.job_id=j.id AND x.action!='keep'),EXISTS(SELECT 1 FROM golden g WHERE g.job_id=j.id),{},{} FROM jobs j ORDER BY j.created_at,j.rowid", tok("tokens_in"), tok("tokens_out")))?
            .query_map([], |r| Ok(json!({"job_id": r.get::<_, String>(0)?, "ts": r.get::<_, i64>(1)?, "company": r.get::<_, String>(2)?, "corrections": r.get::<_, i64>(3)?, "approved": r.get::<_, bool>(4)?, "tokens_in": r.get::<_, i64>(5)?, "tokens_out": r.get::<_, i64>(6)?})))?.collect::<rusqlite::Result<_>>()?;
        let n = |sql: &str| -> Result<i64> { Ok(c.query_row(sql, [], |r| r.get(0))?) };
        let done = n("SELECT count(*) FROM jobs WHERE status='done'")?;
        let approved = n("SELECT count(*) FROM golden")?;
        let reg = c.query_row("SELECT result_json FROM regression_runs ORDER BY id DESC LIMIT 1", [], |r| r.get::<_, String>(0)).optional()?.and_then(|s| serde_json::from_str::<Value>(&s).ok());
        let saved = n("SELECT coalesce(sum(json_extract(output_json,'$.tokens_saved_est')),0) FROM job_steps WHERE name='build_payload' AND status='done'")?
            + n("SELECT coalesce(sum(json_extract(output_json,'$.tokens_in')+json_extract(output_json,'$.tokens_out')),0) FROM job_steps WHERE name='ai_tailor' AND status='done' AND json_extract(output_json,'$.cached')=1")?;
        Ok(Ok(json!({
            "corrections_per_job_trend": jobs.iter().map(|j| j["corrections"].clone()).collect::<Vec<_>>(),
            "jobs": jobs,
            "approval_rate": if done > 0 { approved as f64 / done as f64 } else { 0.0 },
            "active_rules": n("SELECT count(*) FROM playbook_rules WHERE status='active'")?,
            "proposed_rules": n("SELECT count(*) FROM playbook_rules WHERE status='proposed'")?,
            "rule_hits": rule_hits(c)?.values().sum::<i64>(),
            "regression": {"cases": reg.as_ref().map_or(approved, |r| r["cases"].as_i64().unwrap_or(0)), "passing": reg.as_ref().and_then(|r| r["passing"].as_i64()).unwrap_or(0)},
            "tokens_saved_est": saved,
        })))
    }).await
}
