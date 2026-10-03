//! Job engine: sqlite-backed saga with leases, outbox events and fail-closed redaction.
pub mod api;
pub mod drive;
pub mod errors;
pub mod fetch;
pub mod heur;
pub mod keys;
pub mod learn;
pub mod mlsvc;
pub mod pii;
pub mod providers;
pub mod realtime;
pub mod remote;
pub mod review;
use anyhow::{anyhow, Result};
use providers::{provider_for, Provider, ProviderError};
use resume_core::{docx::render_docx, embed::Embedder, fit::{fit_to_pages_with, FitResult}, emphasis::emphasize_resume, grounding::{repair_prompt, GroundingEvent}, heuristics::{decide, Decisions, Features, Heuristics}, jd::{parse_jd, Jd}, latex::{pdf_pages, render_pdf_with, Layout}, payload::{build_payload, parse_reply_full, parse_reply_grounded_repaired}, profile_model::{build_facts, ProfileFacts}, redact::{Kind, Vault}, schema::Resume, select::{ats_coverage_with, select_with_facts, select_with_rules, Budget, Selection}, seniority::now_month};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, fmt::Display, path::{Path, PathBuf}, sync::{Arc, Mutex}, time::Duration};

pub const STEPS: [&str; 10] = ["parse_jd", "select", "ai_review", "build_payload", "ai_tailor", "restore", "render_docx", "render_pdf", "upload_drive", "finalize"];
const MAX_ATTEMPTS: i64 = 4;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS resume(id INTEGER PRIMARY KEY CHECK(id=1), json TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS jobs(id TEXT PRIMARY KEY, status TEXT NOT NULL, provider TEXT NOT NULL, jd_text TEXT NOT NULL,
  extra_redact_json TEXT NOT NULL, max_tokens INTEGER, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
  lease_owner TEXT, lease_until INTEGER, error TEXT);
CREATE TABLE IF NOT EXISTS job_steps(job_id TEXT NOT NULL REFERENCES jobs(id), name TEXT NOT NULL, status TEXT NOT NULL,
  attempts INTEGER NOT NULL DEFAULT 0, input_hash TEXT, output_json TEXT, error TEXT, started_at INTEGER, finished_at INTEGER,
  PRIMARY KEY(job_id,name));
CREATE TABLE IF NOT EXISTS job_events(seq INTEGER PRIMARY KEY AUTOINCREMENT, job_id TEXT NOT NULL, ts INTEGER NOT NULL,
  step TEXT NOT NULL, level TEXT NOT NULL, message TEXT NOT NULL, local_only INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS ai_cache(key TEXT PRIMARY KEY, reply_redacted TEXT NOT NULL, provider TEXT NOT NULL,
  tokens_in INTEGER, tokens_out INTEGER, created_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS keys(provider TEXT PRIMARY KEY, blob BLOB NOT NULL); -- AES-256-GCM, see keys.rs
CREATE TABLE IF NOT EXISTS settings(key TEXT PRIMARY KEY, value TEXT);
CREATE TABLE IF NOT EXISTS pii_always(id TEXT PRIMARY KEY, kind TEXT NOT NULL, value_encrypted BLOB NOT NULL, created_at INTEGER NOT NULL);";

/// Columns added after the first release: (name, DDL), applied with ALTER TABLE when missing.
const JOB_COLS: [(&str, &str); 7] = [("company", "TEXT"), ("role", "TEXT"), ("upload_drive", "INTEGER NOT NULL DEFAULT 0"), ("pages", "INTEGER"), ("always_enc", "BLOB"), ("ai_review", "INTEGER NOT NULL DEFAULT 0"), ("user_overrides", "TEXT")];

fn migrate(c: &Connection) -> Result<()> {
    let have: Vec<String> = c.prepare("SELECT name FROM pragma_table_info('jobs')")?.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    for (n, ddl) in JOB_COLS {
        if !have.iter().any(|h| h == n) {
            c.execute_batch(&format!("ALTER TABLE jobs ADD COLUMN {n} {ddl}"))?;
        }
    }
    Ok(())
}

/// Strip characters unsafe in file names, collapse whitespace, cap at `max` chars.
pub fn sanitize_name(s: &str, max: usize) -> String {
    let t: String = s.chars().filter(|c| !c.is_control() && !"/\\:*?\"<>|".contains(*c)).collect();
    t.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(max).collect::<String>().trim().to_string()
}

/// `{Company} - {Role} - Resume.{ext}`; legal suffixes (Inc, Pvt Ltd...) are dropped from the company.
pub fn display_name(company: &str, role: &str, ext: &str) -> String {
    let re = regex::Regex::new(r"(?i)[,\s]+(inc|llc|ltd|pvt|corp|corporation|gmbh|co|limited|plc)\.?$").unwrap();
    let mut c = company.trim().to_string();
    while let Some(m) = re.find(&c) {
        if m.start() == 0 { break; }
        c.truncate(m.start());
    }
    let ext = format!(".{ext}");
    let base = sanitize_name(&format!("{c} - {role} - Resume"), 120 - ext.len());
    format!("{base}{ext}")
}

pub fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64
}

fn sha(parts: &[&str]) -> String {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p.as_bytes());
        h.update([0]);
    }
    hex::encode(h.finalize())
}

#[derive(Debug)]
pub enum StepErr {
    /// Simulated crash (fault injection): leaves everything exactly as it is.
    Abort,
    Transient(String),
    Permanent(String),
}
fn perm(e: impl Display) -> StepErr {
    StepErr::Permanent(e.to_string())
}
impl From<anyhow::Error> for StepErr {
    fn from(e: anyhow::Error) -> Self {
        perm(e)
    }
}
impl From<ProviderError> for StepErr {
    fn from(e: ProviderError) -> Self {
        match e {
            ProviderError::Transient(m) => StepErr::Transient(m),
            ProviderError::Permanent(m) => StepErr::Permanent(m),
        }
    }
}

/// Test hook: abort the worker after step `n` committed, or mid-step `n` (work done, not committed).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Fault {
    After(usize),
    Mid(usize),
}

pub struct App {
    pub conn: Mutex<Connection>,
    /// Learned models (persisted in settings 'ml_bundle'); lock order: conn, then ml.
    pub ml: Mutex<resume_core::ml::ModelBundle>,
    pub out: PathBuf,
    /// Used for every job when set (tests); otherwise `provider_for(job.provider)`.
    pub provider_override: Mutex<Option<Arc<dyn Provider>>>,
    pub fault: Mutex<Option<Fault>>,
    pub lease_ms: i64,
    pub backoff_ms: u64,
    /// Master key file for the key vault.
    pub key_file: PathBuf,
    pub drive: drive::DriveState,
    /// Process-wide embedder; None = keyword scoring only. Set once the model is ready (or injected by tests).
    pub embedder: Mutex<Option<Arc<dyn Embedder>>>,
    /// heuristics.json (env RESUME_HEURISTICS, else beside the DB).
    pub heuristics_path: PathBuf,
    /// Remote companion listener, pairing and device state.
    pub remote: remote::Remote,
    /// Fitted resume per job, shared between the docx and pdf steps so both match.
    fits: Mutex<HashMap<String, FitResult>>,
}

struct JobRow {
    jd_text: String,
    provider: String,
    extra: Vec<String>,
    max_tokens: Option<i64>,
    resume_json: String,
    company: String,
    role: String,
    upload_drive: bool,
    pages: Option<u8>,
    always: Vec<(Kind, String)>,
    ai_review: bool,
    overrides: Option<String>,
}

pub struct Ctx {
    pub resume: Resume,
    pub jd: Jd,
    pub sel: Selection,
    pub vault: Vault,
    pub decisions: Decisions,
    pub facts: ProfileFacts,
    pub emb: Option<Arc<dyn Embedder>>,
    /// Owner playbook adjustments already applied to `sel`, and what they keyed on.
    pub adj: resume_core::playbook::RuleAdj,
    pub learn: learn::Learned,
}

/// Everything besides the resume/JD that shapes a job's selection and redaction.
#[derive(Default)]
pub struct Opts {
    pub always: Vec<(Kind, String)>,
    pub pages: Option<u8>,
    pub prior: Option<u8>,
    pub h: Heuristics,
    pub emb: Option<Arc<dyn Embedder>>,
    /// The job's own company/role (when known): they override what the JD text suggests, e.g. in the fallback summary.
    pub role: Option<String>,
    pub company: Option<String>,
    /// Active rules, JD facts for them, the owner's persistent edits.
    pub learn: learn::Learned,
}

pub fn ctx(resume_json: &str, jd_text: &str, extra: &[String]) -> Result<Ctx> {
    ctx_with(resume_json, jd_text, extra, Opts::default())
}

pub fn ctx_with(resume_json: &str, jd_text: &str, extra: &[String], o: Opts) -> Result<Ctx> {
    let resume: Resume = serde_json::from_str(resume_json)?;
    let mut jd = parse_jd(jd_text);
    jd.role = o.role.clone().or(jd.role);
    jd.company = o.company.clone().or(jd.company);
    let m = now_month();
    let facts = build_facts(&resume, (m.div_euclid(12), m.rem_euclid(12) as u32 + 1));
    let mut f = Features::from_facts(&facts, &jd, jd_text, o.prior, &o.h);
    if let Some(p) = o.pages {
        f.jd_signals.page_ask = Some(p);
    }
    let mut decisions = decide(&f, &o.h);
    if let Some(p) = o.pages {
        decisions.why[0].outcome = format!("Page target set to {p} for this job (overrides the automatic decision)");
    }
    let adj = resume_core::playbook::apply_rules(&o.learn.rules, &o.learn.jdc, &resume);
    let sel = if o.learn.rules.is_empty() { select_with_facts(&resume, &jd, Budget::from_decisions(&decisions), o.emb.as_deref(), &facts) } else { select_with_rules(&resume, &jd, Budget::from_decisions(&decisions), o.emb.as_deref(), Some(&facts), &adj) };
    let extras: Vec<_> = extra.iter().map(|s| (Kind::Org, s.clone())).chain(o.always).collect();
    let vault = Vault::from_resume(&resume, &extras);
    Ok(Ctx { resume, jd, sel, vault, decisions, facts, emb: o.emb, adj, learn: o.learn })
}

pub fn sel_json(s: &Selection) -> Value {
    json!({"bullets": s.bullets, "projects": s.projects, "skills": s.skills, "certs": s.certs})
}

fn apply_select(cx: &mut Ctx, out: &Value) {
    if let Ok(s) = serde_json::from_value::<(Vec<Vec<usize>>, Vec<usize>, Vec<(usize, Vec<usize>)>, Vec<usize>)>(json!([out["selection"]["bullets"], out["selection"]["projects"], out["selection"]["skills"], out["selection"]["certs"]])) {
        cx.sel = Selection { bullets: s.0, projects: s.1, skills: s.2, certs: s.3 };
    }
    if let Ok(d) = serde_json::from_value(out["decisions"].clone()) {
        cx.decisions = d;
    }
}

pub fn kind_from(s: &str) -> Option<Kind> {
    Some(match s { "person" => Kind::Person, "email" => Kind::Email, "phone" => Kind::Phone, "org" | "custom" => Kind::Org, "client" => Kind::Client, _ => return None })
}
const ALWAYS_AAD: &str = "pii_always";
const JOB_AAD: &str = "job_always";

/// Decrypted (kind, value) rows of the always-redact table; `kind` is the stored label (may be "custom").
pub fn always_rows(c: &Connection, kf: &Path) -> Result<Vec<(String, String, String)>> {
    let mut s = c.prepare("SELECT id,kind,value_encrypted FROM pii_always ORDER BY created_at,id")?;
    let rows: Vec<(String, String, Vec<u8>)> = s.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<rusqlite::Result<_>>()?;
    rows.into_iter().map(|(i, k, b)| Ok((i, k, keys::decrypt(kf, ALWAYS_AAD, &b)?))).collect()
}
pub fn always_encrypt(kf: &Path, v: &str) -> Result<Vec<u8>> {
    keys::encrypt(kf, ALWAYS_AAD, v)
}

/// The always-redact snapshot frozen for a job at first run (encrypted), so token numbering never drifts.
fn job_always(c: &Connection, kf: &Path, id: &str, snapshot: bool) -> Result<Vec<(Kind, String)>> {
    let blob: Option<Vec<u8>> = c.query_row("SELECT always_enc FROM jobs WHERE id=?", [id], |r| r.get(0)).optional()?.flatten();
    let list: Vec<(String, String)> = match blob {
        Some(b) => serde_json::from_str(&keys::decrypt(kf, JOB_AAD, &b)?)?,
        None if snapshot => {
            let l: Vec<(String, String)> = always_rows(c, kf)?.into_iter().map(|(_, k, v)| (k, v)).collect();
            c.execute("UPDATE jobs SET always_enc=? WHERE id=?", params![keys::encrypt(kf, JOB_AAD, &serde_json::to_string(&l)?)?, id])?;
            l
        }
        None => vec![],
    };
    Ok(list.into_iter().filter_map(|(k, v)| Some((kind_from(&k)?, v))).collect())
}

/// Read-only context for job_detail.
#[derive(Default)]
pub struct DetailEnv {
    pub emb: Option<Arc<dyn Embedder>>,
    pub h: Heuristics,
    pub prior: Option<u8>,
    pub key_file: Option<PathBuf>,
    /// For the JD family the playbook rules key on.
    pub jdc: Option<resume_core::ml::jd_classifier::JdClassifier>,
}

impl DetailEnv {
    pub fn of(a: &App) -> DetailEnv {
        let prior = a.conn.lock().unwrap().query_row("SELECT value FROM settings WHERE key='prior_resume_pages'", [], |r| r.get::<_, String>(0)).ok().and_then(|v| v.parse().ok());
        DetailEnv { emb: a.embedder(), h: a.heuristics(), prior, key_file: Some(a.key_file.clone()), jdc: Some(a.ml.lock().unwrap().jd.clone()) }
    }
    /// Context for a job: stored selection/decisions win over recomputation when `sel_out` is the select step output.
    pub fn ctx(&self, c: &Connection, id: &str, resume_json: &str, jd_text: &str, extra: &[String], sel_out: Option<&Value>) -> Result<Ctx> {
        let always = match &self.key_file { Some(k) => job_always(c, k, id, false)?, None => vec![] };
        let (pages, role, company): (Option<i64>, Option<String>, Option<String>) = c.query_row("SELECT pages,role,company FROM jobs WHERE id=?", [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        let learn = learn::load(c, id, jd_text, company.as_deref().unwrap_or(""), self.jdc.as_ref());
        let mut cx = ctx_with(resume_json, jd_text, extra, Opts { always, pages: pages.map(|p| p as u8), prior: self.prior, h: self.h.clone(), emb: self.emb.clone(), role, company, learn })?;
        if let Some(o) = sel_out {
            apply_select(&mut cx, o);
        }
        Ok(cx)
    }
}

pub fn full_sel(r: &Resume) -> Selection {
    Selection {
        bullets: r.experience.iter().map(|e| (0..e.bullets.len()).collect()).collect(),
        projects: (0..r.projects.len()).collect(),
        skills: r.skills.iter().enumerate().map(|(i, c)| (i, (0..c.skills.len()).collect())).collect(),
        certs: (0..r.certifications.len()).collect(),
    }
}

pub fn ev(c: &Connection, job: &str, step: &str, level: &str, msg: &str, local: bool) -> rusqlite::Result<()> {
    c.execute("INSERT INTO job_events(job_id,ts,step,level,message,local_only) VALUES(?,?,?,?,?,?)", params![job, now(), step, level, msg, local])?;
    Ok(())
}

pub fn events_after(c: &Connection, job: &str, seq: i64) -> Result<Vec<Value>> {
    let mut s = c.prepare("SELECT seq,ts,step,level,message,local_only FROM job_events WHERE job_id=? AND seq>? ORDER BY seq")?;
    let r = s.query_map(params![job, seq], |r| Ok(json!({"seq": r.get::<_, i64>(0)?, "ts": r.get::<_, i64>(1)?, "step": r.get::<_, String>(2)?, "level": r.get::<_, String>(3)?, "message": r.get::<_, String>(4)?, "local_only": r.get::<_, bool>(5)?})))?;
    Ok(r.collect::<rusqlite::Result<_>>()?)
}

pub fn create_job(c: &mut Connection, jd: &str, provider: &str, extra: &[String], max_tokens: Option<i64>) -> Result<String> {
    create_job_with(c, jd, provider, extra, max_tokens, None, None, false)
}

/// Like `create_job_with`, plus an optional 1|2 page override.
pub fn create_job_pages(c: &mut Connection, jd: &str, provider: &str, extra: &[String], max_tokens: Option<i64>, company: Option<&str>, role: Option<&str>, upload_drive: bool, pages: Option<u8>) -> Result<String> {
    let id = create_job_with(c, jd, provider, extra, max_tokens, company, role, upload_drive)?;
    c.execute("UPDATE jobs SET pages=? WHERE id=?", params![pages.map(i64::from), id])?;
    Ok(id)
}

/// Company/role fall back to what the JD says, then "Unknown" / "Role".
pub fn create_job_with(c: &mut Connection, jd: &str, provider: &str, extra: &[String], max_tokens: Option<i64>, company: Option<&str>, role: Option<&str>, upload_drive: bool) -> Result<String> {
    let id = uuid::Uuid::new_v4().to_string();
    let parsed = parse_jd(jd);
    let pick = |given: Option<&str>, found: Option<String>, d: &str| given.map(str::trim).filter(|s| !s.is_empty()).map(String::from).or(found).unwrap_or(d.into());
    let (company, role) = (pick(company, parsed.company, "Unknown"), pick(role, parsed.role, "Role"));
    let tx = c.transaction()?;
    tx.execute("INSERT INTO jobs(id,status,provider,jd_text,extra_redact_json,max_tokens,created_at,updated_at,company,role,upload_drive) VALUES(?,?,?,?,?,?,?,?,?,?,?)", params![id, "queued", provider, jd, serde_json::to_string(extra)?, max_tokens, now(), now(), company, role, upload_drive])?;
    for s in STEPS {
        tx.execute("INSERT INTO job_steps(job_id,name,status) VALUES(?,?,'pending')", params![id, s])?;
    }
    tx.execute("UPDATE jobs SET ai_review=? WHERE id=?", params![provider != "mock", id])?; // real providers review the plan by default
    ev(&tx, &id, "queue", "info", "Job queued", true)?;
    tx.commit()?;
    Ok(id)
}

/// Reset `from` and later steps, requeue. Retrying the AI step drops its cached reply so it is asked again.
pub fn retry_job(c: &mut Connection, id: &str, from: &str) -> Result<()> {
    let i = STEPS.iter().position(|s| *s == from).ok_or_else(|| anyhow!("unknown step"))?;
    let tx = c.transaction()?;
    let st: String = tx.query_row("SELECT status FROM jobs WHERE id=?", [id], |r| r.get(0))?;
    if !matches!(st.as_str(), "failed" | "dead" | "cancelled" | "done") {
        return Err(anyhow!("job is {st}"));
    }
    if i <= 4 {
        tx.execute("DELETE FROM ai_cache WHERE key=(SELECT input_hash FROM job_steps WHERE job_id=? AND name='ai_tailor')", [id])?;
    }
    for s in &STEPS[i..] {
        tx.execute("UPDATE job_steps SET status='pending',attempts=0,error=NULL,output_json=NULL,input_hash=NULL WHERE job_id=? AND name=?", params![id, s])?;
    }
    tx.execute("UPDATE jobs SET status='queued',error=NULL,lease_owner=NULL,lease_until=NULL,updated_at=? WHERE id=?", params![now(), id])?;
    ev(&tx, id, from, "info", &format!("Retrying from {from}"), true)?;
    tx.commit()?;
    Ok(())
}

pub fn cancel_job(c: &mut Connection, id: &str) -> Result<bool> {
    let tx = c.transaction()?;
    let n = tx.execute("UPDATE jobs SET status='cancelled',updated_at=? WHERE id=? AND status IN('queued','running')", params![now(), id])?;
    if n > 0 {
        ev(&tx, id, "cancel", "warn", "Cancelled; takes effect at the next step boundary", true)?;
        realtime::job_terminal(&tx, id, "cancelled", None)?;
    }
    tx.commit()?;
    Ok(n > 0)
}

fn claim(c: &mut Connection, owner: &str, lease_ms: i64) -> Result<Option<String>> {
    let tx = c.transaction()?;
    let id: Option<String> = tx.query_row("UPDATE jobs SET status='running',lease_owner=?1,lease_until=?2,updated_at=?3 WHERE id=(SELECT id FROM jobs WHERE status='queued' ORDER BY created_at LIMIT 1) RETURNING id", params![owner, now() + lease_ms, now()], |r| r.get(0)).optional()?;
    if let Some(id) = &id {
        ev(&tx, id, "claim", "info", "Worker picked up job", true)?;
    }
    tx.commit()?;
    Ok(id)
}

/// Requeue running jobs whose lease expired (all running jobs when `all`, e.g. at startup).
pub fn sweep(c: &mut Connection, all: bool) -> Result<usize> {
    let tx = c.transaction()?;
    let ids: Vec<String> = tx.prepare("SELECT id FROM jobs WHERE status='running' AND (?1 OR lease_until<?2)")?.query_map(params![all, now()], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    for id in &ids {
        tx.execute("UPDATE jobs SET status='queued',lease_owner=NULL,lease_until=NULL,updated_at=? WHERE id=?", params![now(), id])?;
        ev(&tx, id, "lease", "warn", "Worker lease expired; resuming from first unfinished step", true)?;
    }
    tx.commit()?;
    Ok(ids.len())
}

fn ktok(n: usize) -> String {
    if n >= 1000 { format!("{:.1}k", n as f64 / 1000.0) } else { n.to_string() }
}

fn promote(tmp: &Path, file: &str, dest: &Path) -> Result<()> {
    std::fs::File::open(tmp.join(file))?.sync_all()?;
    std::fs::rename(tmp.join(file), dest.join(file))?;
    std::fs::File::open(dest)?.sync_all()?;
    std::fs::remove_dir_all(tmp)?;
    Ok(())
}

impl App {
    pub fn open(db: &Path, out: &Path) -> Result<Arc<App>> {
        if let Some(p) = db.parent() {
            std::fs::create_dir_all(p)?;
        }
        let c = Connection::open(db)?;
        c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;")?;
        c.execute_batch(SCHEMA)?;
        migrate(&c)?;
        realtime::migrate(&c)?;
        c.execute_batch(mlsvc::SCHEMA)?;
        c.execute_batch(review::SCHEMA)?;
        c.execute_batch(learn::SCHEMA)?;
        c.execute_batch(remote::SCHEMA)?;
        learn::seed(&c)?;
        Ok(Arc::new(App { drive: Default::default(), ml: Mutex::new(mlsvc::load(&c)), conn: Mutex::new(c), out: out.into(), provider_override: Mutex::new(None), fault: Mutex::new(None), lease_ms: 30_000, backoff_ms: 500,
            key_file: std::env::var_os("RESUME_MASTER_KEY_FILE").map(PathBuf::from).unwrap_or_else(|| db.with_file_name("master.key")),
            remote: remote::Remote::new(db.parent().map(PathBuf::from).unwrap_or_default()), embedder: Mutex::new(None), fits: Mutex::new(HashMap::new()),
            heuristics_path: std::env::var_os("RESUME_HEURISTICS").map(PathBuf::from).unwrap_or_else(|| db.with_file_name("heuristics.json")) }))
    }

    pub fn embedder(&self) -> Option<Arc<dyn Embedder>> {
        self.embedder.lock().unwrap().clone()
    }
    pub fn set_embedder(&self, e: Option<Arc<dyn Embedder>>) {
        *self.embedder.lock().unwrap() = e;
    }
    /// Effective heuristics, re-read each call so edits apply to the next job.
    pub fn heuristics(&self) -> Heuristics {
        Heuristics::load(&self.heuristics_path)
    }

    /// Real providers load + decrypt their key at call time.
    pub async fn provider_for(self: &Arc<Self>, name: &str) -> Result<Arc<dyn Provider>, ProviderError> {
        if !matches!(name, "anthropic" | "gemini") {
            return provider_for(name);
        }
        let n = name.to_string();
        let blob: Option<Vec<u8>> = self.db(move |c| Ok(c.query_row("SELECT blob FROM keys WHERE provider=?", [n], |r| r.get(0)).optional()?)).await.map_err(|e| ProviderError::Permanent(e.to_string()))?;
        let key = match blob {
            Some(b) => keys::decrypt(&self.key_file, name, &b).map_err(|e| ProviderError::Permanent(format!("{name}: {e}")))?,
            None => keys::env_key(name).ok_or_else(|| ProviderError::Permanent(format!("no API key configured for {name} (set {} or add one in Settings)", keys::env_names(name)[0])))?,
        };
        let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        Ok(if name == "anthropic" {
            Arc::new(providers::anthropic::Anthropic::new(env("RESUME_ANTHROPIC_URL").unwrap_or("https://api.anthropic.com".into()), env("RESUME_ANTHROPIC_MODEL").unwrap_or("claude-haiku-4-5-20251001".into()), key)?)
        } else {
            Arc::new(providers::gemini::Gemini::new(env("RESUME_GEMINI_URL").unwrap_or("https://generativelanguage.googleapis.com".into()), env("RESUME_GEMINI_MODEL").unwrap_or("gemini-flash-latest".into()), key)?)
        })
    }

    pub async fn db<T: Send + 'static>(self: &Arc<Self>, f: impl FnOnce(&mut Connection) -> Result<T> + Send + 'static) -> Result<T> {
        let me = self.clone();
        tokio::task::spawn_blocking(move || f(&mut me.conn.lock().unwrap())).await?
    }

    fn hit(&self, f: Fault) -> bool {
        let mut g = self.fault.lock().unwrap();
        (*g == Some(f)) && g.take().is_some()
    }

    /// Claim and run one queued job. Ok(false) = nothing queued. Err = aborted/crashed (lease left to expire).
    pub async fn run_next(self: &Arc<Self>) -> Result<bool> {
        let (owner, lease) = (uuid::Uuid::new_v4().to_string(), self.lease_ms);
        let o = owner.clone();
        let Some(id) = self.db(move |c| claim(c, &o, lease)).await? else { return Ok(false) };
        let (me, i, o) = (self.clone(), id.clone(), owner.clone());
        let hb = tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis((lease / 3) as u64)).await;
                let (i, o) = (i.clone(), o.clone());
                let _ = me.db(move |c| Ok(c.execute("UPDATE jobs SET lease_until=? WHERE id=? AND lease_owner=?", params![now() + lease, i, o])?)).await;
            }
        });
        let r = self.run_job(&id, &owner).await;
        hb.abort();
        r.map(|_| true).map_err(|e| anyhow!("job stopped: {e:?}"))
    }

    pub async fn worker_loop(self: Arc<Self>) {
        loop {
            match self.run_next().await {
                Ok(true) => {}
                Ok(false) => tokio::time::sleep(Duration::from_millis(300)).await,
                Err(e) => {
                    eprintln!("worker: {e}");
                    tokio::time::sleep(Duration::from_secs(1)).await
                }
            }
        }
    }

    pub async fn sweeper_loop(self: Arc<Self>) {
        let mut first = true;
        loop {
            let all = std::mem::take(&mut first);
            let _ = self.db(move |c| sweep(c, all)).await;
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }

    async fn run_job(self: &Arc<Self>, id: &str, owner: &str) -> Result<(), StepErr> {
        let jid = id.to_string();
        let j = self.db(move |c| Ok(c.query_row("SELECT jd_text,provider,extra_redact_json,max_tokens,(SELECT json FROM resume WHERE id=1),coalesce(company,'Unknown'),coalesce(role,'Role'),upload_drive,pages,ai_review,user_overrides FROM jobs WHERE id=?", [&jid], |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, String>(2)?, r.get(3)?, r.get::<_, Option<String>>(4)?, r.get::<_, String>(5)?, r.get::<_, String>(6)?, r.get::<_, bool>(7)?, r.get::<_, Option<i64>>(8)?, r.get::<_, bool>(9)?, r.get::<_, Option<String>>(10)?)))?)).await?;
        let (jid, kf) = (id.to_string(), self.key_file.clone());
        let always = self.db(move |c| job_always(c, &kf, &jid, true)).await?;
        let prior: Option<u8> = self.setting("prior_resume_pages").await.and_then(|v| v.parse().ok());
        let extra: Vec<String> = serde_json::from_str(&j.2).map_err(perm)?;
        let Some(resume_json) = j.4 else { return self.fail(id, "parse_jd", "no resume stored".into(), false).await };
        let j = JobRow { jd_text: j.0, provider: j.1, extra, max_tokens: j.3, resume_json, company: j.5, role: j.6, upload_drive: j.7, pages: j.8.map(|p| p as u8), always, ai_review: j.9, overrides: j.10 };
        let (jid, me, jd_t, comp) = (id.to_string(), self.clone(), j.jd_text.clone(), j.company.clone());
        let learn = self.db(move |c| Ok(learn::load(c, &jid, &jd_t, &comp, Some(&me.ml.lock().unwrap().jd)))).await?;
        let opts = Opts { learn, always: j.always.clone(), pages: j.pages, prior, h: self.heuristics(), emb: self.embedder(), role: Some(j.role.clone()).filter(|r| r != "Role"), company: Some(j.company.clone()).filter(|c| c != "Unknown") };
        let mut cx = match ctx_with(&j.resume_json, &j.jd_text, &j.extra, opts) {
            Ok(c) => c,
            Err(e) => return self.fail(id, "parse_jd", e.to_string(), false).await,
        };
        mlsvc::rerank(&mut cx, &self.ml.lock().unwrap().ranker);
        let mut prev = sha(&[&j.resume_json, &j.jd_text, &j.extra.join("\u{1}"), &j.provider, &format!("{:?}", j.max_tokens)]);
        let mut outs: HashMap<&str, Value> = HashMap::new();
        for (i, name) in STEPS.iter().enumerate() {
            let (jid, own) = (id.to_string(), owner.to_string());
            let alive = self.db(move |c| Ok(c.query_row("SELECT status='running' AND lease_owner=? FROM jobs WHERE id=?", params![own, jid], |r| r.get::<_, bool>(0))?)).await?;
            if !alive {
                return Ok(()); // cancelled or lease lost
            }
            let hash = if *name == "ai_tailor" {
                let p = &outs["build_payload"];
                sha(&[p["system"].as_str().unwrap_or(""), p["user"].as_str().unwrap_or(""), &j.provider])
            } else if *name == "ai_review" {
                sha(&[&prev, name, &j.ai_review.to_string(), j.overrides.as_deref().unwrap_or("")])
            } else {
                sha(&[&prev, name])
            };
            prev = sha(&[&prev, &hash]);
            let (jid, n) = (id.to_string(), name.to_string());
            let old = self.db(move |c| Ok(c.query_row("SELECT status,input_hash,output_json FROM job_steps WHERE job_id=? AND name=?", params![jid, n], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<String>>(2)?)))?)).await?;
            if let (s, Some(h), Some(o)) = old {
                if s == "done" && h == hash {
                    outs.insert(name, serde_json::from_str(&o).map_err(perm)?);
                    if matches!(*name, "select" | "ai_review") {
                        apply_select(&mut cx, &outs[name]);
                    }
                    continue;
                }
            }
            loop {
                let (jid, n, h) = (id.to_string(), name.to_string(), hash.clone());
                let sending = if *name == "ai_tailor" {
                    let t = outs["build_payload"]["tokens_est"].as_u64().unwrap_or(0) as usize;
                    Some(format!("Sending {} tokens, redacted, to {}", ktok(t), j.provider))
                } else if *name == "upload_drive" && j.upload_drive {
                    let n = outs.keys().filter(|k| k.starts_with("render_")).count();
                    Some(format!("Uploading {n} files to Google Drive folder \"{}\" (your account)", self.drive_folder().await))
                } else {
                    None
                };
                let attempt = self.db(move |c| {
                    let tx = c.transaction()?;
                    tx.execute("UPDATE job_steps SET status='running',attempts=attempts+1,input_hash=?,started_at=?,error=NULL WHERE job_id=? AND name=?", params![h, now(), jid, n])?;
                    if let Some(m) = sending {
                        ev(&tx, &jid, &n, "info", &m, false)?;
                    }
                    let a = tx.query_row("SELECT attempts FROM job_steps WHERE job_id=? AND name=?", params![jid, n], |r| r.get::<_, i64>(0))?;
                    tx.commit()?;
                    Ok(a)
                }).await?;
                match self.work(name, id, &j, &cx, &outs, &hash).await {
                    Ok((out, msg)) => {
                        if self.hit(Fault::Mid(i)) {
                            return Err(StepErr::Abort);
                        }
                        let (jid, n, o) = (id.to_string(), name.to_string(), out.to_string());
                        let local = *name != "ai_tailor" && !(*name == "ai_review" && out["ran"] == true);
                        self.db(move |c| {
                            let tx = c.transaction()?;
                            tx.execute("UPDATE job_steps SET status='done',output_json=?,finished_at=? WHERE job_id=? AND name=?", params![o, now(), jid, n])?;
                            ev(&tx, &jid, &n, "info", &msg, local)?;
                            if n == "finalize" {
                                if tx.execute("UPDATE jobs SET status='done',updated_at=? WHERE id=? AND status='running'", params![now(), jid])? > 0 {
                                    realtime::job_terminal(&tx, &jid, "done", None)?;
                                }
                            }
                            tx.commit()?;
                            Ok(())
                        }).await?;
                        outs.insert(name, out);
                        if matches!(*name, "select" | "ai_review") {
                            apply_select(&mut cx, &outs[name]);
                        }
                        if *name == "finalize" {
                            self.fits.lock().unwrap().remove(id);
                        }
                        if self.hit(Fault::After(i)) {
                            return Err(StepErr::Abort);
                        }
                        break;
                    }
                    Err(StepErr::Abort) => return Err(StepErr::Abort),
                    Err(StepErr::Permanent(m)) => return self.fail(id, name, m, false).await,
                    Err(StepErr::Transient(m)) if attempt >= MAX_ATTEMPTS => return self.fail(id, name, m, true).await,
                    Err(StepErr::Transient(m)) => {
                        let wait = self.backoff_ms * (1 << (attempt - 1)) + rand::random::<u64>() % (self.backoff_ms + 1);
                        let (jid, n) = (id.to_string(), name.to_string());
                        self.db(move |c| {
                            let tx = c.transaction()?;
                            tx.execute("UPDATE job_steps SET status='pending',error=? WHERE job_id=? AND name=?", params![m, jid, n])?;
                            ev(&tx, &jid, &n, "warn", &format!("Transient error ({m}); retry {attempt}/{MAX_ATTEMPTS} in {wait}ms"), n != "ai_tailor")?;
                            tx.commit()?;
                            Ok(())
                        }).await?;
                        tokio::time::sleep(Duration::from_millis(wait)).await;
                    }
                }
            }
        }
        Ok(())
    }

    async fn fail(self: &Arc<Self>, id: &str, step: &str, msg: String, dead: bool) -> Result<(), StepErr> {
        let (id, step) = (id.to_string(), step.to_string());
        self.db(move |c| {
            let tx = c.transaction()?;
            let e = errors::AppError::classify(&step, &msg); // msg may carry private values (leak); only the classified text is stored/sent
            tx.execute("UPDATE job_steps SET status='failed',error=?,finished_at=? WHERE job_id=? AND name=?", params![e.message, now(), id, step])?;
            let n = tx.execute("UPDATE jobs SET status=?,error=?,updated_at=? WHERE id=? AND status='running'", params![if dead { "dead" } else { "failed" }, serde_json::to_string(&e)?, now(), id])?;
            realtime::emit_err(&tx, &id, "step", "error", &e, &format!("{}: {}", if dead { "Gave up after retries" } else { "Failed" }, e.message), step != "ai_tailor", false)?;
            if n > 0 {
                realtime::job_terminal(&tx, &id, if dead { "dead" } else { "failed" }, Some(&e))?;
            }
            tx.commit()?;
            Ok(())
        }).await?;
        Ok(())
    }

    async fn work(self: &Arc<Self>, name: &str, id: &str, j: &JobRow, cx: &Ctx, outs: &HashMap<&str, Value>, hash: &str) -> Result<(Value, String), StepErr> {
        Ok(match name {
            "parse_jd" => {
                self.record_jd(id, &j.jd_text, &cx.jd).await?;
                let c = self.ml.lock().unwrap().jd.classify(&j.jd_text);
                let top = |v: &[(String, f32)]| v.first().map(|x| x.0.clone()).unwrap_or_default();
                let (f, sn) = (top(&c.family), top(&c.seniority));
                (json!({"requirements": cx.jd.requirements.len(), "jd_class": {"family": f, "seniority": sn}}), format!("Parsed job description: {} requirements, looks like {sn} {f} (local, no AI)", cx.jd.requirements.len()))
            }
            "select" => {
                let b: usize = cx.sel.bullets.iter().map(Vec::len).sum();
                let cov = ats_coverage_with(&cx.resume, &cx.sel, &cx.jd, cx.emb.as_deref());
                let how = if cx.emb.is_some() { "local semantic model + keywords" } else { "local keywords" };
                (json!({"bullets": b, "coverage": cov.score, "semantic": cov.semantic, "decisions": cx.decisions, "selection": sel_json(&cx.sel), "semantic_used": cx.emb.is_some(), "rules": learn::applied(&cx.learn.rules, &cx.adj.hits)}),
                    format!("Matching {b} bullets to {} JD requirements ({how}); target {} page(s)", cx.jd.requirements.len(), cx.decisions.target_pages))
            }
            "ai_review" => self.ai_review(id, j, cx).await?,
            "build_payload" => {
                let (psel, sent, skipped, saved) = mlsvc::prune(&self.ml.lock().unwrap().rewrite, cx);
                let mut p = build_payload(&cx.resume, &psel, &cx.jd, &cx.vault).map_err(perm)?;
                p.system += &learn::writer_section(cx); // owner's summary/bullet rules; redacted text, guarded below
                cx.vault.guard(&format!("{}\n{}", p.system, p.user)).map_err(perm)?;
                let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
                for m in regex::Regex::new(r"\[([A-Z]+)_\d+\]").unwrap().captures_iter(&p.user) {
                    *counts.entry(m[1].to_string()).or_default() += 1;
                }
                let est = (p.system.len() + p.user.len()) / 4;
                let n: usize = counts.values().sum();
                (json!({"system": p.system, "user": p.user, "token_counts": counts, "tokens_est": est, "rw_sent": sent, "skipped": skipped, "tokens_saved_est": saved}), format!("Built redacted payload ({} tokens, {n} placeholders), leak guard passed (local, no AI){}", ktok(est), mlsvc::saved_note(skipped, saved)))
            }
            "ai_tailor" => {
                let (sys, user) = (outs["build_payload"]["system"].as_str().unwrap_or("").to_string(), outs["build_payload"]["user"].as_str().unwrap_or("").to_string());
                let est = (sys.len() + user.len()) / 4;
                if j.max_tokens.is_some_and(|m| est as i64 > m) {
                    return Err(perm(format!("payload ~{est} tokens exceeds job budget")));
                }
                let key = hash.to_string();
                let hit = self.db(move |c| Ok(c.query_row("SELECT reply_redacted,tokens_in,tokens_out FROM ai_cache WHERE key=?", [key], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?))).optional()?)).await?;
                let (reply, ti, to, cached) = match hit {
                    Some((r, a, b)) => (r, a, b, true),
                    None => {
                        let p = self.provider_override.lock().unwrap().clone();
                        let p = match p { Some(p) => p, None => self.provider_for(&j.provider).await? };
                        let r = p.complete(&sys, &user).await?;
                        let (key, text, prov, a, b) = (hash.to_string(), r.text.clone(), j.provider.clone(), r.tokens_in as i64, r.tokens_out as i64);
                        // Cached immediately, separate from the step commit: a crash after this never re-sends.
                        self.db(move |c| Ok(c.execute("INSERT OR IGNORE INTO ai_cache VALUES(?,?,?,?,?,?)", params![key, text, prov, a, b, now()])?)).await?;
                        (r.text, a, b, false)
                    }
                };
                let m = if cached { format!("Reused cached reply from {} (no new request)", j.provider) } else { format!("Received reply from {} ({} tokens out)", j.provider, ktok(to as usize)) };
                (json!({"reply": reply, "tokens_in": ti, "tokens_out": to, "cached": cached}), m)
            }
            "restore" => {
                let (mut full, mut ev) = restored_full(cx, outs)?;
                let asked = ev.len();
                let repair_reply = if asked > 0 { self.repair(id, j, cx, outs, &ev).await? } else { None };
                if let Some(rep) = &repair_reply {
                    (full, ev) = restored_with(cx, outs, Some(rep))?;
                }
                self.rw_observe(cx, &outs["build_payload"]["rw_sent"], &full).await;
                let grounding = grounding_json(cx, &ev);
                // style-only issues (length, copying, line-1 shape) kept as written without a repair are not "reverted"
                let ev_all = ev;
                let ev: Vec<_> = ev_all.iter().filter(|e| e.repaired || e.violations.iter().any(|v| !matches!(v.kind, resume_core::grounding::ViolationKind::WordCount | resume_core::grounding::ViolationKind::CopiedFromSource | resume_core::grounding::ViolationKind::Line1))).cloned().collect();
                let (fixed, n) = (ev.iter().filter(|e| e.repaired).count(), ev.iter().filter(|e| !e.repaired).count());
                let jid = id.to_string();
                let msgs: Vec<(&str, String)> = ev.iter().map(|e| if e.repaired { ("info", format!("Repaired rewrite of {}", e.bullet_id)) } else { ("warn", format!("Reverted rewrite of {}: {}", e.bullet_id, e.violations.iter().map(|v| v.kind_text()).collect::<Vec<_>>().join(", "))) }).collect();
                // each violation and its repair outcome is a rewrite-acceptance signal for mlsvc (kinds only, no text)
                let rows: Vec<(String, &str, String)> = ev.iter().map(|e| (e.bullet_id.clone(), if e.repaired { "rewrite_repaired" } else { "rewrite_reverted" }, json!({"kinds": e.violations.iter().map(|v| v.kind_text()).collect::<Vec<_>>(), "repaired": e.repaired, "asked": repair_reply.is_some()}).to_string())).collect();
                self.db(move |c| {
                    for (level, m) in &msgs { if *level == "info" { crate::ev(c, &jid, "restore", "info", m, true)?; } else { ev_(c, &jid, "restore", level, m)?; } }
                    for (bid, action, f) in &rows { c.execute("INSERT INTO ml_feedback(job_id,bullet_id,action,features_json,ts) VALUES(?,?,?,?,?)", params![jid, bid, action, f, now()])?; }
                    Ok(())
                }).await?;
                if repair_reply.is_some() {
                    self.note(id, "restore", "info", &format!("Repair outcome: {fixed} of {asked} lines fixed; {n} kept their original wording"), true).await?;
                }
                let t = cx.sel.apply(&full);
                (json!({"bullets": t.experience.iter().map(|e| e.bullets.len()).sum::<usize>(), "grounding": grounding, "repair_reply": repair_reply, "repair": {"asked": asked, "fixed": fixed}, "lint": review::lint_json(&full, cx)}), format!("Validated reply and restored real details locally; grounding reverted {n} rewrite(s){} (local, no AI)", if fixed > 0 { format!(", repaired {fixed}") } else { String::new() }))
            }
            "render_docx" | "render_pdf" => {
                let (full, _) = restored_full(cx, outs)?;
                let pdf = name == "render_pdf";
                let file = if pdf { "resume.pdf" } else { "resume.docx" };
                let mut fit = self.fitted(id, cx, full, !pdf).await?;
                let dir = self.out.join(id);
                let mut r = fit.resume.clone();
                resume_core::review::sort_newest_first(&mut r);
                if self.heuristics().emphasis {
                    emphasize_resume(&mut r, &cx.jd);
                }
                let level = fit.layout_level;
                let pages = tokio::task::spawn_blocking(move || -> Result<Option<usize>> {
                    std::fs::create_dir_all(&dir)?;
                    for e in std::fs::read_dir(&dir)?.flatten() {
                        if e.file_name().to_string_lossy().starts_with(".tmp") {
                            let _ = std::fs::remove_dir_all(e.path());
                        }
                    }
                    let tmp = dir.join(format!(".tmp-{}", uuid::Uuid::new_v4()));
                    std::fs::create_dir_all(&tmp)?;
                    let mut pages = None;
                    if pdf {
                        let levels: Vec<Layout> = std::iter::once(Layout::default()).chain(Layout::levels()).collect();
                        let p = render_pdf_with(&r, &levels[level.min(levels.len() - 1)], &tmp)?;
                        pages = Some(pdf_pages(&p)?);
                    } else {
                        render_docx(&r, &tmp.join("resume.docx"))?;
                    }
                    promote(&tmp, file, &dir)?;
                    Ok(pages)
                }).await.map_err(perm)?.map_err(perm)?;
                if let Some(p) = pages { fit.pages = p; }
                if pdf { self.fit_observe(id, &fit.resume, fit.layout_level).await; }
                (json!({"file": file, "fit": {"pages": fit.pages, "layout_level": fit.layout_level, "steps": fit.steps, "dropped": fit.dropped}}), if pdf { format!("Rendered {file}, {} (local, no AI)", resume_core::native_pdf::pdf_engine()) } else { format!("Rendered {file} (local, no AI)") })
            }
            "upload_drive" if !j.upload_drive => (json!(null), "Google Drive upload not requested (local, no AI)".into()),
            "upload_drive" => {
                let files: Vec<(&str, String)> = [("render_pdf", "pdf"), ("render_docx", "docx")].iter().filter(|(s, _)| outs.contains_key(*s)).map(|(_, k)| (*k, display_name(&j.company, &j.role, k))).collect();
                let up = self.drive_upload(id, &self.out.join(id), &files).await?;
                let n = up.len();
                (json!(up), format!("Uploaded {n} files to Google Drive"))
            }
            "finalize" => (json!({}), "Job complete".into()),
            _ => return Err(perm("unknown step")),
        })
    }
}

/// Parse the stored (redacted) reply back into real text, grounded against the profile; vault is rebuilt from the resume, never persisted.
/// A stored repair reply (restore step output) is replayed, so every later step sees the same repaired lines.
/// Returns the full (un-trimmed) resume with rewrites applied plus one event per violating rewrite (`repaired` says which were fixed).
pub fn restored_full(cx: &Ctx, outs: &HashMap<&str, Value>) -> Result<(Resume, Vec<GroundingEvent>), StepErr> {
    let (mut r, ev) = restored_with(cx, outs, outs.get("restore").and_then(|r| r["repair_reply"].as_str()))?;
    learn::apply_edits(&mut r, &cx.learn.edits); // the owner's own lines win, and survive every re-render
    Ok((r, ev))
}

fn restored_with(cx: &Ctx, outs: &HashMap<&str, Value>, repair: Option<&str>) -> Result<(Resume, Vec<GroundingEvent>), StepErr> {
    parse_reply_grounded_repaired(outs["ai_tailor"]["reply"].as_str().unwrap_or(""), repair, &cx.vault, &cx.resume, &cx.sel, &cx.facts, &cx.jd).map_err(perm)
}

/// The selection with skill rows in JD relevance -> recency -> proficiency order (renderers keep it: their own arrange is stable).
pub fn jd_ordered(cx: &Ctx, full: &Resume) -> Selection {
    let a = resume_core::arrange::arrange(full, &cx.sel, Some(&cx.jd), Some(&cx.facts));
    Selection { skills: a.skill_rows_order.iter().filter_map(|&i| cx.sel.skills.get(i).cloned()).collect(), ..cx.sel.clone() }
}

/// Trimmed to the selection.
pub fn restored(cx: &Ctx, outs: &HashMap<&str, Value>) -> Result<Resume, StepErr> {
    let full = restored_full(cx, outs)?.0;
    let mut t = jd_ordered(cx, &full).apply(&full);
    resume_core::review::sort_newest_first(&mut t);
    Ok(t)
}

/// Events as `[{bullet_id, violations:[{kind, detail}]}]`; details are passed through the vault so no real value can appear.
pub fn grounding_json(cx: &Ctx, ev: &[GroundingEvent]) -> Value {
    json!(ev.iter().map(|e| json!({"bullet_id": e.bullet_id, "repaired": e.repaired, "violations": e.violations.iter().map(|v| json!({"kind": v.kind_text(), "detail": cx.vault.redact(&v.detail)})).collect::<Vec<_>>()})).collect::<Vec<_>>())
}

trait KindText { fn kind_text(&self) -> String; }
impl KindText for resume_core::grounding::Violation {
    fn kind_text(&self) -> String {
        format!("{:?}", self.kind)
    }
}

fn ev_(c: &Connection, job: &str, step: &str, level: &str, msg: &str) -> rusqlite::Result<()> {
    realtime::emit_err(c, job, "step", level, &errors::AppError::new("grounding.reverted", msg, step), msg, true, false).map(|_| ())
}

impl App {
    async fn note(self: &Arc<Self>, id: &str, step: &str, level: &str, msg: &str, local: bool) -> Result<(), StepErr> {
        let (id, step, level, msg) = (id.to_string(), step.to_string(), level.to_string(), msg.to_string());
        Ok(self.db(move |c| Ok(ev(c, &id, &step, &level, &msg, local)?)).await?)
    }

    /// The bounded self-repair: ONE call to the job's provider with only the violating rewritten lines and their
    /// violation tokens (redacted, leak-guarded like `build_payload`). Returns the raw (redacted) reply, None when skipped.
    /// Skipped for the unscripted mock provider, over the job token budget, or on any provider trouble.
    async fn repair(self: &Arc<Self>, id: &str, j: &JobRow, cx: &Ctx, outs: &HashMap<&str, Value>, ev: &[GroundingEvent]) -> Result<Option<String>, StepErr> {
        if j.provider == "mock" && self.provider_override.lock().unwrap().is_none() {
            return Ok(None);
        }
        let raw = parse_reply_full(outs["ai_tailor"]["reply"].as_str().unwrap_or(""), &cx.vault, &cx.resume, &cx.sel).map_err(perm)?;
        let Ok(Some((sys, user))) = repair_prompt(&raw, ev, &cx.vault, &cx.facts, &cx.jd) else { return Ok(None) };
        let est = (sys.len() + user.len()) / 4;
        let used: i64 = ["tokens_in", "tokens_out"].iter().map(|k| outs["ai_tailor"][k].as_i64().unwrap_or(0)).sum();
        if j.max_tokens.is_some_and(|m| used + est as i64 > m) {
            self.note(id, "restore", "warn", "Repair skipped: over the job token budget", true).await?;
            return Ok(None);
        }
        self.note(id, "restore", "info", &format!("Asked the AI to repair {} lines that made unsupported claims (sent ~{} tokens)", ev.len(), ktok(est)), false).await?;
        match self.complete_cached(j, &sys, &user, false).await?.0 {
            Ok((text, ..)) => Ok(Some(text)),
            Err(why) => {
                self.note(id, "restore", "warn", &format!("Repair skipped: {}", why.chars().take(80).collect::<String>()), false).await?;
                Ok(None)
            }
        }
    }

    /// Fit the full rewritten resume to the page target. `force` recomputes (render_docx); the pdf step reuses it so both files match.
    async fn fitted(self: &Arc<Self>, id: &str, cx: &Ctx, full: Resume, force: bool) -> Result<FitResult, StepErr> {
        if !force {
            if let Some(f) = self.fits.lock().unwrap().get(id).cloned() {
                return Ok(f);
            }
        }
        let (sel, jd, target, start, h) = (jd_ordered(cx, &full), cx.jd.clone(), cx.decisions.target_pages, cx.decisions.layout_start_level, self.heuristics());
        let start = mlsvc::suggest_level(&self.ml.lock().unwrap().fit, &full, &sel, target).unwrap_or(start);
        let tmp = self.out.join(id).join(format!(".tmp-fit-{}", uuid::Uuid::new_v4()));
        let t = tmp.clone();
        let f = tokio::task::spawn_blocking(move || {
            std::fs::create_dir_all(&t)?;
            let r = fit_to_pages_with(&full, &sel, &jd, target, start, &h, &t);
            let _ = std::fs::remove_dir_all(&t);
            r
        }).await.map_err(perm)?.map_err(perm)?;
        let step = if force { "render_docx" } else { "render_pdf" };
        let (jid, steps, warn) = (id.to_string(), f.steps.clone(), (f.pages > target as usize).then(|| format!("Could not fit {target} page(s): the result is {} page(s)", f.pages)));
        self.db(move |c| {
            for m in &steps { ev(c, &jid, step, "info", m, true)?; }
            if let Some(w) = warn { realtime::emit_err(c, &jid, "step", "warn", &errors::AppError::new("render.fit_failed", &w, step), &w, true, false)?; }
            Ok(())
        }).await?;
        self.fits.lock().unwrap().insert(id.to_string(), f.clone());
        Ok(f)
    }
}

impl App {
    pub async fn drive_folder(self: &Arc<Self>) -> String {
        self.db(|c| Ok(c.query_row("SELECT value FROM settings WHERE key='drive_folder'", [], |r| r.get::<_, String>(0)).optional()?)).await.ok().flatten().filter(|f| !f.is_empty()).unwrap_or_else(|| "Tailored Resumes".into())
    }
}
