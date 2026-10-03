//! AI + lint plan review and owner overrides (fake data, scripted MockProvider, no network).
use axum::{body::{to_bytes, Body}, http::{Request, StatusCode}, Router};
use resume_core::review::SYSTEM;
use resume_server::{api::router, ctx, events_after, providers::{mock::MockProvider, ProviderError}, sweep, App, Fault};
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Arc};
use tower::ServiceExt;

const JD: &str = "Backend Engineer at Initech\nRequirements:\n- Python and Kubernetes\n- Terraform\n";
const REAL: [&str; 6] = ["Jane", "Doe", "Acme", "Globex", "jane.doe", "0104477"];

fn resume() -> Value {
    json!({"profile": {"name": "Jane Doe", "email": "jane.doe@acme-robotics.test", "phone": "+1 555 010 4477", "role": "Engineer", "location": "Springfield"},
        "summary": ["Engineer who ships."],
        "experience": [
            {"id": "x2", "role": "Software Intern", "organization": "Initech", "dateLabel": "2017", "bullets": ["Wrote Terraform modules"]},
            {"id": "x1", "role": "Backend Engineer", "organization": "Acme Robotics Pvt Ltd", "dateLabel": "2021 - 2024", "bullets": ["Built Python services on Kubernetes at Acme Robotics", "Ran lunches"]},
            {"id": "x0", "role": "Developer", "organization": "Globex Corp", "dateLabel": "2018 - 2020", "bullets": ["Maintained billing code", "Wrote Python scripts"]}],
        "skills": [{"label": "Cloud", "skills": ["Python", "Terraform", "Kubernetes"]}, {"label": "Misc", "skills": ["Juggling"]}, {"label": "Tools", "skills": ["Vim"]},
            {"label": "Data", "skills": ["SQL"]}, {"label": "Ops", "skills": ["Linux"]}],
        "projects": [{"name": "Garden Bot", "description": "Hobby robot", "techStack": ["Arduino"]},
            {"name": "Pipeline Tool", "description": "Terraform and Python deployment tool", "techStack": ["Terraform"]},
            {"name": "Kube Operator", "description": "Kubernetes operator in Python", "techStack": ["Kubernetes"]},
            {"name": "Chess Engine", "description": "Plays chess", "techStack": ["Rust"]}],
        "certifications": [{"title": "Scrum Master", "issuer": "Scrum.org", "dateLabel": "2019"}, {"title": "Yoga Teacher", "issuer": "YA", "dateLabel": "2020"},
            {"title": "Terraform Associate", "issuer": "HashiCorp", "dateLabel": "2021"}, {"title": "Kubernetes Admin", "issuer": "CNCF", "dateLabel": "2022"}]})
}

fn setup(p: Arc<MockProvider>) -> (Arc<App>, Router) {
    let dir: PathBuf = std::env::temp_dir().join(format!("rs-review-{}", uuid::Uuid::new_v4()));
    let a = App::open(&dir.join("app.db"), &dir.join("out")).unwrap();
    a.conn.lock().unwrap().execute("INSERT INTO resume VALUES(1,?)", [resume().to_string()]).unwrap();
    *a.provider_override.lock().unwrap() = Some(p);
    let a = Arc::try_unwrap(a).ok().map(|mut a| { a.backoff_ms = 1; Arc::new(a) }).unwrap();
    let r = router(a.clone());
    (a, r)
}

async fn call(app: &Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut r = Request::builder().method(method).uri(uri);
    let b = match body { Some(v) => { r = r.header("content-type", "application/json"); Body::from(v.to_string()) } None => Body::empty() };
    let resp = app.clone().oneshot(r.body(b).unwrap()).await.unwrap();
    let st = resp.status();
    let b = to_bytes(resp.into_body(), 16 << 20).await.unwrap();
    (st, serde_json::from_slice(&b).unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&b).into())))
}
async fn new_job(r: &Router, review: bool) -> String {
    let (st, v) = call(r, "POST", "/api/jobs", Some(json!({"jd_text": JD, "provider": "mock", "ai_review": review}))).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    v["id"].as_str().unwrap().to_string()
}
async fn detail(r: &Router, id: &str) -> Value {
    call(r, "GET", &format!("/api/jobs/{id}"), None).await.1
}
/// ids of pool entries (projects/certs) with the given selected flag.
fn ids(d: &Value, kind: &str, selected: bool) -> Vec<String> {
    d["pool"][kind].as_array().unwrap().iter().filter(|x| x["selected"] == selected).map(|x| x["id"].as_str().unwrap().to_string()).collect()
}
fn issue<'a>(d: &'a Value, source: &str, area: &str, status: &str) -> Option<&'a Value> {
    d["review"]["issues"].as_array().unwrap().iter().find(|i| i["source"] == source && i["area"] == area && i["status"] == status)
}

#[tokio::test]
async fn ai_swaps_validated_and_invalid_parts_rejected_without_pii() {
    let p = Arc::new(MockProvider::default());
    let (a, r) = setup(p.clone());
    let base = new_job(&r, false).await;
    a.run_next().await.unwrap();
    let d0 = detail(&r, &base).await;
    assert_eq!(p.review_calls(), 0, "mock default: review off");
    assert_eq!(d0["pool"]["caps"].as_object().unwrap().len(), 2);
    let (np, nc) = (ids(&d0, "projects", false)[0].clone(), ids(&d0, "certs", false)[0].clone());
    // projects swap is valid; certs reference an unknown id; roles drop the latest internship
    *p.review.lock().unwrap() = Some(json!({"overrides": {"projects": [np], "certs": ["c99", nc], "roles": ["e1", "e2"], "skills_order": ["Ops", "Cloud"]},
        "issues": [{"area": "summary", "severity": "warn", "message": "Summary is generic"}], "confidence": 0.8}).to_string());
    let id = new_job(&r, true).await;
    a.run_next().await.unwrap();
    let d = detail(&r, &id).await;
    assert_eq!(d["job"]["status"], "done", "{d}");
    assert_eq!(p.review_calls(), 1);
    assert_eq!(ids(&d, "projects", true), [np.clone()]);
    let rv = &d["review"];
    assert!((rv["ran"].as_bool(), rv["overrides_applied"].as_bool()) == (Some(true), Some(true)) && (rv["confidence"].as_f64().unwrap() - 0.8).abs() < 0.01);
    assert!(rv["tokens_in"].as_i64().unwrap() > 0 && rv["provider"] == "mock");
    assert!(issue(&d, "ai", "projects", "applied").is_some());
    let c = issue(&d, "ai", "certs", "rejected").unwrap();
    assert!(c["reason"].as_str().unwrap().contains("c99"));
    let ro = issue(&d, "ai", "experience", "rejected").unwrap();
    assert!(ro["reason"].as_str().unwrap().contains("latest"), "{ro}");
    assert!(issue(&d, "ai", "summary", "suggested").is_some());
    let evs = events_after(&a.conn.lock().unwrap(), &id, 0).unwrap();
    assert!(evs.iter().any(|e| e["message"].as_str().unwrap().starts_with("AI reviewed the plan: swapped ") && e["message"].as_str().unwrap().ends_with("tokens, redacted)") && e["local_only"] == false), "{evs:?}");
    // everything sent was redacted and passes the vault guard
    let prompt = p.review_prompts.lock().unwrap()[0].clone();
    assert!(prompt.starts_with(SYSTEM) && prompt.contains("[ORG_"), "{prompt}");
    for x in REAL { assert!(!prompt.contains(x), "review prompt leaked {x}"); }
    assert!(!prompt.contains("x1") && !prompt.contains("http"));
    ctx(&resume().to_string(), JD, &[]).unwrap().vault.guard(&prompt).unwrap();
    // pool contract
    let pr = &d["pool"]["projects"][0];
    for k in ["id", "name", "tech", "tier", "score", "selected"] { assert!(pr.get(k).is_some(), "project.{k}"); }
    let ro = d["pool"]["roles"].as_array().unwrap();
    assert!(ro.iter().any(|x| x["kind"] == "Internship" && x["locked"] == true) && ro[2]["id"] == "e2");
    assert!(d["pool"]["certs"][0].get("featured").is_some() && d["pool"]["skills"][0].get("label").is_some());
}

#[tokio::test]
async fn provider_failure_is_non_fatal() {
    for e in [ProviderError::Permanent("boom".into()), ProviderError::Transient("503".into())] {
        let p = Arc::new(MockProvider::with_failures(vec![e]));
        let (a, r) = setup(p.clone());
        let id = new_job(&r, true).await;
        a.run_next().await.unwrap();
        let d = detail(&r, &id).await;
        assert_eq!(d["job"]["status"], "done", "{d}");
        assert_eq!((p.review_calls(), p.calls()), (1, 1));
        assert_eq!(d["review"]["ran"], false);
        let evs = events_after(&a.conn.lock().unwrap(), &id, 0).unwrap();
        assert!(evs.iter().any(|e| e["level"] == "warn" && e["message"].as_str().unwrap().starts_with("AI review skipped: ")), "{evs:?}");
    }
}

#[tokio::test]
async fn lint_restores_skills_and_orders_experience() {
    let (a, r) = setup(Arc::new(MockProvider::default()));
    let id = new_job(&r, false).await;
    a.run_next().await.unwrap();
    let d = detail(&r, &id).await;
    assert_eq!(d["review"]["ran"], false);
    assert!(d["selection"]["skills"].as_array().unwrap().len() >= 4);
    assert!(issue(&d, "lint", "order", "applied").is_some());
    let roles: Vec<&str> = d["tailored"]["experience"].as_array().unwrap().iter().map(|e| e["role"].as_str().unwrap()).collect();
    assert_eq!(roles[0], "Backend Engineer", "{roles:?}");
}

#[tokio::test]
async fn user_overrides_win_rerun_from_select_and_train() {
    let p = Arc::new(MockProvider::default());
    let (a, r) = setup(p.clone());
    let base = new_job(&r, false).await;
    a.run_next().await.unwrap();
    let d0 = detail(&r, &base).await;
    let (unsel_p, unsel_c) = (ids(&d0, "projects", false), ids(&d0, "certs", false));
    *p.review.lock().unwrap() = Some(json!({"overrides": {"projects": [unsel_p[0]]}, "issues": [], "confidence": 0.9}).to_string());
    let id = new_job(&r, true).await;
    a.run_next().await.unwrap();
    assert_eq!(ids(&detail(&r, &id).await, "projects", true), [unsel_p[0].clone()]);
    // owner picks another project; unknown ids and dropping the locked roles are refused
    let url = format!("/api/jobs/{id}/overrides");
    assert_eq!(call(&r, "POST", &url, Some(json!({"projects": ["p77"]}))).await.0, StatusCode::BAD_REQUEST);
    let (st, e) = call(&r, "POST", &url, Some(json!({"roles": ["e2"]}))).await;
    assert!(st == StatusCode::BAD_REQUEST && e.as_str().unwrap().contains("latest"), "{e}");
    let (st, _) = call(&r, "POST", &url, Some(json!({"projects": [unsel_p[1]], "certs": [unsel_c[0]], "note": "prefer this"}))).await;
    assert_eq!(st, StatusCode::ACCEPTED);
    a.run_next().await.unwrap();
    let d = detail(&r, &id).await;
    assert_eq!(d["job"]["status"], "done", "{d}");
    assert_eq!(ids(&d, "projects", true), [unsel_p[1].clone()], "user beats AI");
    assert_eq!(ids(&d, "certs", true), [unsel_c[0].clone()]);
    assert_eq!(d["review"]["user_overrides"]["projects"], json!([unsel_p[1]]));
    assert_eq!(p.review_calls(), 1, "re-run reuses the cached review");
    let n: i64 = a.conn.lock().unwrap().query_row("SELECT count(*) FROM ml_feedback WHERE job_id=? AND action='keep' AND bullet_id=?", params(&id, &unsel_p[1]), |r| r.get(0)).unwrap();
    let m: i64 = a.conn.lock().unwrap().query_row("SELECT count(*) FROM ml_feedback WHERE job_id=? AND action='remove' AND bullet_id=?", params(&id, &unsel_p[0]), |r| r.get(0)).unwrap();
    assert_eq!((n, m), (1, 1));
    let prefs: i64 = a.conn.lock().unwrap().query_row("SELECT count(*) FROM job_prefs WHERE job_id=?", [&id], |r| r.get(0)).unwrap();
    assert_eq!(prefs, 1);
    // a later similar JD is offered the same picks (suggested, not applied)
    let again = new_job(&r, false).await;
    a.run_next().await.unwrap();
    let d2 = detail(&r, &again).await;
    let s = issue(&d2, "lint", "experience", "suggested").unwrap();
    assert_eq!(s["change"]["projects"], json!([unsel_p[1]]));
    assert_ne!(ids(&d2, "projects", true), [unsel_p[1].clone()]);
    // empty array = select none; null/absent = untouched; reset clears and re-runs the AI plan
    let (st, _) = call(&r, "POST", &url, Some(json!({"certs": [], "projects": null}))).await;
    assert_eq!(st, StatusCode::ACCEPTED);
    a.run_next().await.unwrap();
    let d = detail(&r, &id).await;
    assert!(ids(&d, "certs", true).is_empty());
    assert_eq!(ids(&d, "projects", true), [unsel_p[1].clone()]);
    let (st, _) = call(&r, "POST", &url, Some(json!({"reset": true}))).await;
    assert_eq!(st, StatusCode::ACCEPTED);
    a.run_next().await.unwrap();
    let d = detail(&r, &id).await;
    assert!(d["review"]["user_overrides"].is_null());
    assert_eq!(ids(&d, "projects", true), [unsel_p[0].clone()], "back to the AI plan");
    // a locked role may only be dropped with force
    let (st, _) = call(&r, "POST", &url, Some(json!({"roles": ["e2"], "force": true}))).await;
    assert_eq!(st, StatusCode::ACCEPTED);
    a.run_next().await.unwrap();
    let d = detail(&r, &id).await;
    assert!(d["pool"]["roles"].as_array().unwrap().iter().filter(|x| x["selected"] == true).all(|x| x["id"] == "e2"));
}

fn params<'a>(a: &'a str, b: &'a str) -> [&'a str; 2] { [a, b] }

#[tokio::test]
async fn crash_during_review_does_not_repeat_the_ai_call() {
    for f in [Fault::After(2), Fault::Mid(2)] {
        let p = Arc::new(MockProvider::default());
        let (a, r) = setup(p.clone());
        let id = new_job(&r, true).await;
        *a.fault.lock().unwrap() = Some(f);
        assert!(a.run_next().await.is_err(), "{f:?}");
        a.conn.lock().unwrap().execute("UPDATE jobs SET lease_until=0 WHERE status='running'", []).unwrap();
        sweep(&mut a.conn.lock().unwrap(), false).unwrap();
        a.run_next().await.unwrap();
        assert_eq!(detail(&r, &id).await["job"]["status"], "done", "{f:?}");
        assert_eq!((p.review_calls(), p.calls()), (1, 1), "{f:?}");
    }
}
