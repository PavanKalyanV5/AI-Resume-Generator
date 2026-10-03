//! Learning backend: corrections, playbook rules, golden set + regression, metrics (fake data, MockProvider, no network).
use axum::{body::{to_bytes, Body}, http::{Request, StatusCode}, Router};
use resume_server::{api::router, providers::mock::MockProvider, App};
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Arc};
use tower::ServiceExt;

const JD: &str = "Machine Learning Engineer at Initech\nRequirements:\n- Python and SQL\n- Machine learning and data analytics\n- At least one modern language such as Java, C++, or C#\n";

fn resume() -> Value {
    json!({"profile": {"name": "Jane Doe", "email": "jane.doe@acme-robotics.test", "phone": "+1 555 010 4477", "role": "Engineer", "location": "Springfield"},
        "summary": ["Engineer who ships."],
        "experience": [
            {"id": "x2", "role": "Software Intern", "organization": "Initech", "dateLabel": "2017", "bullets": ["Wrote Python scripts"]},
            {"id": "x1", "role": "Backend Engineer", "organization": "Acme Robotics Pvt Ltd", "dateLabel": "2021 - 2024", "bullets": ["Built Python and C# services at Acme Robotics", "Ran lunches"]},
            {"id": "x0", "role": "Developer", "organization": "Globex Corp", "dateLabel": "2018 - 2020", "bullets": ["Maintained billing code", "Wrote SQL reports"]}],
        "skills": [{"label": "Cloud", "skills": ["Python", "Terraform"]}, {"label": "Misc", "skills": ["Juggling"]}, {"label": "Tools", "skills": ["Vim"]}, {"label": "Data", "skills": ["SQL"]}, {"label": "Ops", "skills": ["Linux"]}],
        "projects": [{"name": "Garden Bot", "description": "Hobby robot", "techStack": ["Arduino"]}, {"name": "Netflix UI Clone", "description": "UI clone", "techStack": ["HTML", "CSS"]},
            {"name": "ML Pipeline", "description": "Machine learning data pipeline in Python", "techStack": ["Python"], "tier": "featured"}, {"name": "Chess Engine", "description": "Plays chess", "techStack": ["Rust"]}],
        "certifications": [
            {"title": "Java Programming Basics", "issuer": "Udemy", "dateLabel": "2024", "featured": true},
            {"title": "TalentNext Full Stack Training", "issuer": "TalentNext", "dateLabel": "2023", "featured": true},
            {"title": "Pottery", "issuer": "Studio", "dateLabel": "2020"},
            {"title": "Google Data Analytics Certificate", "issuer": "Google", "dateLabel": "2024"},
            {"title": "Yoga Teacher", "issuer": "YA", "dateLabel": "2019"}]})
}

fn setup() -> (Arc<App>, Router) {
    let dir: PathBuf = std::env::temp_dir().join(format!("rs-learn-{}", uuid::Uuid::new_v4()));
    let a = App::open(&dir.join("app.db"), &dir.join("out")).unwrap();
    a.conn.lock().unwrap().execute("INSERT INTO resume VALUES(1,?)", [resume().to_string()]).unwrap();
    *a.provider_override.lock().unwrap() = Some(Arc::new(MockProvider::default()));
    a.set_embedder(Some(Arc::new(resume_core::embed::HashEmbedder::default())));
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
async fn ok(app: &Router, method: &str, uri: &str, body: Option<Value>) -> Value {
    let (st, v) = call(app, method, uri, body).await;
    assert!(st.is_success(), "{method} {uri}: {st} {v}");
    v
}
async fn job(a: &Arc<App>, r: &Router, jd: &str) -> String {
    let v = ok(r, "POST", "/api/jobs", Some(json!({"jd_text": jd, "provider": "mock"}))).await;
    a.run_next().await.unwrap();
    v["id"].as_str().unwrap().to_string()
}
async fn detail(r: &Router, id: &str) -> Value {
    ok(r, "GET", &format!("/api/jobs/{id}"), None).await
}
fn sel(d: &Value, kind: &str) -> Vec<String> {
    d["pool"][kind].as_array().unwrap().iter().filter(|x| x["selected"] == true).map(|x| x["id"].as_str().unwrap().to_string()).collect()
}
fn rule<'a>(pb: &'a Value, id: &str) -> &'a Value {
    pb["rules"].as_array().unwrap().iter().find(|r| r["id"] == id).unwrap_or_else(|| panic!("no rule {id}"))
}
/// Owner removes `cert` from the job's plan.
async fn remove_cert(a: &Arc<App>, r: &Router, id: &str, cert: &str) {
    let d = detail(r, id).await;
    let keep: Vec<String> = sel(&d, "certs").into_iter().filter(|c| c != cert).collect();
    ok(r, "POST", &format!("/api/jobs/{id}/overrides"), Some(json!({"certs": keep}))).await;
    a.run_next().await.unwrap();
}
fn proposed_notices(a: &Arc<App>) -> i64 {
    a.conn.lock().unwrap().query_row("SELECT count(*) FROM job_events e JOIN notifications n ON n.seq=e.seq WHERE e.code='playbook.proposed'", [], |r| r.get(0)).unwrap()
}

#[tokio::test]
async fn overrides_become_corrections_with_diffs_and_reasons() {
    let (a, r) = setup();
    ok(&r, "POST", "/api/playbook/rules/seed-cert-basics/disable", None).await;
    let id = job(&a, &r, JD).await;
    let d = detail(&r, &id).await;
    let (ps, cs) = (sel(&d, "projects"), sel(&d, "certs"));
    assert!(cs.contains(&"c0".to_string()), "language-specific cert is in the plan once its seed rule is off: {cs:?}");
    let add = d["pool"]["projects"].as_array().unwrap().iter().find(|p| p["selected"] == false).unwrap()["id"].as_str().unwrap().to_string();
    let drop = ps[0].clone();
    let mut np: Vec<String> = ps.iter().filter(|p| **p != drop).cloned().collect();
    np.push(add.clone());
    ok(&r, "POST", &format!("/api/jobs/{id}/overrides"), Some(json!({"projects": np}))).await;
    a.run_next().await.unwrap();
    let cs = ok(&r, "GET", &format!("/api/jobs/{id}/corrections"), None).await;
    let cs = cs.as_array().unwrap();
    assert_eq!(cs.len(), 2, "{cs:?}");
    let by = |act: &str| cs.iter().find(|c| c["action"] == act).unwrap();
    assert_eq!((by("add")["item_id"].as_str(), by("add")["area"].as_str(), by("add")["source"].as_str()), (Some(add.as_str()), Some("projects"), Some("owner")));
    assert_eq!(by("remove")["item_id"], drop.as_str());
    assert!(by("remove")["item"]["title"].is_string() && by("remove")["item"]["tier"].is_string() && by("remove")["before"]["selected"] == true && by("remove")["after"]["selected"] == false);
    assert!(by("remove")["jd_family"].as_str().is_some_and(|f| !f.is_empty()));
    assert_eq!(detail(&r, &id).await["corrections"].as_array().unwrap().len(), 2);
    // reasons
    let cid = by("remove")["id"].as_i64().unwrap();
    assert_eq!(call(&r, "POST", &format!("/api/corrections/{cid}/reason"), Some(json!({"tags": ["wrong_tone", "nope"]}))).await.0, StatusCode::BAD_REQUEST);
    ok(&r, "POST", &format!("/api/corrections/{cid}/reason"), Some(json!({"tags": ["irrelevant", "too_generic"]}))).await;
    let cs = ok(&r, "GET", &format!("/api/jobs/{id}/corrections"), None).await;
    assert_eq!(cs.as_array().unwrap().iter().find(|c| c["id"] == cid).unwrap()["reason_tags"], json!(["irrelevant", "too_generic"]));
    assert_eq!(call(&r, "POST", "/api/corrections/9999/reason", Some(json!({"tags": []}))).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn three_consistent_removals_propose_one_rule_two_do_not_and_keeps_block() {
    let (a, r) = setup();
    ok(&r, "POST", "/api/playbook/rules/seed-cert-basics/disable", None).await;
    let mut ids = vec![];
    for _ in 0..2 {
        let id = job(&a, &r, JD).await;
        remove_cert(&a, &r, &id, "c0").await;
        ids.push(id);
    }
    assert_eq!(a.mine().await.unwrap(), 0, "two removals are not enough");
    assert_eq!(proposed_notices(&a), 0);
    let id = job(&a, &r, JD).await;
    remove_cert(&a, &r, &id, "c0").await;
    a.mine().await.unwrap();
    a.mine().await.unwrap();
    let pb = ok(&r, "GET", "/api/playbook", None).await;
    let prop: Vec<&Value> = pb["rules"].as_array().unwrap().iter().filter(|x| x["status"] == "proposed").collect();
    assert_eq!(prop.len(), 1, "exactly one proposal: {prop:?}");
    let p = prop[0];
    assert!(p["text"].as_str().unwrap().contains("removed language-specific basics certs 3 times"), "{p}");
    assert_eq!((p["origin"].as_str(), p["support_count"].as_i64()), (Some("mined"), Some(3)));
    assert_eq!(p["examples"].as_array().unwrap().len(), 3);
    assert_eq!(proposed_notices(&a), 1, "one notification, mining twice does not repeat it");
    let n = a.conn.lock().unwrap().query_row("SELECT actions FROM job_events WHERE code='playbook.proposed'", [], |r| r.get::<_, String>(0)).unwrap();
    assert!(n.contains("/playbook"));
    // never auto-activated
    assert!(pb["groups"]["active"].as_array().unwrap().iter().all(|i| !i.as_str().unwrap().starts_with("mined-")));

    // contradiction: an approved job that keeps a language-specific cert in the same family blocks a fresh proposal
    let (a, r) = setup();
    ok(&r, "POST", "/api/playbook/rules/seed-cert-basics/disable", None).await;
    let keep = job(&a, &r, JD).await;
    ok(&r, "POST", &format!("/api/jobs/{keep}/approve"), None).await;
    for _ in 0..3 {
        let id = job(&a, &r, JD).await;
        remove_cert(&a, &r, &id, "c0").await;
    }
    assert_eq!(a.mine().await.unwrap(), 0);
    assert_eq!(proposed_notices(&a), 0);
}

#[tokio::test]
async fn playbook_lifecycle_approve_reject_disable_edit_rollback() {
    let (a, r) = setup();
    let pb = ok(&r, "GET", "/api/playbook", None).await;
    assert_eq!(rule(&pb, "seed-cert-basics")["badge"], "From your 2026-10-03 feedback");
    assert!(pb["groups"]["active"].as_array().unwrap().len() >= 7 && pb["versions"].as_array().unwrap().len() == 1);
    // mined proposals via three removals (seed off), then approve one and reject another
    ok(&r, "POST", "/api/playbook/rules/seed-cert-basics/disable", None).await;
    for _ in 0..3 {
        let id = job(&a, &r, JD).await;
        remove_cert(&a, &r, &id, "c0").await;
    }
    a.mine().await.unwrap();
    let pb = ok(&r, "GET", "/api/playbook", None).await;
    let pid = pb["groups"]["proposed"][0].as_str().unwrap().to_string();
    assert_eq!(call(&r, "POST", &format!("/api/playbook/rules/{pid}/disable"), None).await.0, StatusCode::CONFLICT, "a proposal cannot be disabled");
    // an edit is validated: PII-like text is rejected, a good edit bumps the version
    assert_eq!(call(&r, "PUT", &format!("/api/playbook/rules/{pid}"), Some(json!({"text": "ask jane.doe@example.com"}))).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(call(&r, "PUT", &format!("/api/playbook/rules/{pid}"), Some(json!({"action": {"boost": 0.9}}))).await.0, StatusCode::BAD_REQUEST);
    let e = ok(&r, "PUT", &format!("/api/playbook/rules/{pid}"), Some(json!({"text": "Skip language basics certs", "action": {"boost": -0.4}}))).await;
    assert_eq!((e["version"].as_i64(), e["text"].as_str(), e["action"]["boost"].as_f64().map(|b| (b * 10.0).round())), (Some(2), Some("Skip language basics certs"), Some(-4.0)));
    let v = ok(&r, "POST", &format!("/api/playbook/rules/{pid}/approve"), None).await;
    assert_eq!(v["rule"]["status"], "active");
    assert_eq!(v["impact"]["of"], 0, "no approved resumes yet");
    ok(&r, "POST", &format!("/api/playbook/rules/{pid}/disable"), None).await;
    ok(&r, "POST", &format!("/api/playbook/rules/{pid}/enable"), None).await;
    let n = ok(&r, "GET", "/api/playbook", None).await["versions"].as_array().unwrap().len();
    assert!(n >= 6, "{n}");
    // user rule + delete; seeds cannot be deleted
    let u = ok(&r, "POST", "/api/playbook/rules", Some(json!({"area": "certs", "text": "Never show pottery", "matcher": {"kind": "cert", "title_regex": "Pottery"}, "action": {"forbid": true}}))).await;
    assert_eq!((u["status"].as_str(), u["origin"].as_str()), (Some("active"), Some("user")));
    assert_eq!(call(&r, "DELETE", "/api/playbook/rules/seed-cert-basics", None).await.0, StatusCode::CONFLICT);
    ok(&r, "DELETE", &format!("/api/playbook/rules/{}", u["id"].as_str().unwrap()), None).await;
    // reject path
    let (a2, r2) = setup();
    ok(&r2, "POST", "/api/playbook/rules/seed-cert-basics/disable", None).await;
    for _ in 0..3 {
        let id = job(&a2, &r2, JD).await;
        remove_cert(&a2, &r2, &id, "c0").await;
    }
    a2.mine().await.unwrap();
    let pid2 = ok(&r2, "GET", "/api/playbook", None).await["groups"]["proposed"][0].as_str().unwrap().to_string();
    ok(&r2, "POST", &format!("/api/playbook/rules/{pid2}/reject"), None).await;
    a2.mine().await.unwrap();
    let pb2 = ok(&r2, "GET", "/api/playbook", None).await;
    assert_eq!((pb2["groups"]["rejected"].as_array().unwrap().len(), pb2["groups"]["proposed"].as_array().unwrap().len()), (1, 0), "a rejected rule is not proposed again");
    // rollback to the seed snapshot: user-level changes are undone (the seed disable is reverted)
    let back = ok(&r, "POST", "/api/playbook/rollback", Some(json!({"version": 1}))).await;
    assert_eq!(rule(&back, "seed-cert-basics")["status"], "active");
    assert!(back["rules"].as_array().unwrap().iter().all(|x| x["origin"] != "user"));
    assert!(back["rules"].as_array().unwrap().iter().all(|x| x["id"] != pid.as_str()), "the approved mined rule is not in the seed snapshot");
    assert_eq!(call(&r, "POST", "/api/playbook/rollback", Some(json!({"version": 999}))).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn rules_change_selection_and_locked_roles_survive() {
    let (a, r) = setup();
    ok(&r, "POST", "/api/playbook/rules/seed-cert-basics/disable", None).await;
    let off = job(&a, &r, JD).await;
    let d_off = detail(&r, &off).await;
    assert!(sel(&d_off, "certs").contains(&"c1".to_string()), "TalentNext training cert tops up without the rule: {:?}", sel(&d_off, "certs"));
    ok(&r, "POST", "/api/playbook/rules/seed-cert-basics/enable", None).await;
    let on = job(&a, &r, JD).await;
    let d = detail(&r, &on).await;
    let cs = sel(&d, "certs");
    assert!(!cs.contains(&"c1".to_string()) && !cs.contains(&"c0".to_string()), "seed (a) removes TalentNext/DSA-like certs: {cs:?}");
    assert!(d["rules_applied"].as_array().unwrap().iter().any(|x| x["rule_id"] == "seed-cert-basics" && x["effect"].as_str().unwrap().contains("avoided")), "{}", d["rules_applied"]);
    assert!(!sel(&d, "projects").contains(&"p1".to_string()), "UI clone dropped for a non-frontend JD");
    // a JD that asks for data structures keeps them eligible
    let ds = job(&a, &r, &format!("{JD}- Strong data structures")).await;
    assert!(!detail(&r, &ds).await["rules_applied"].as_array().unwrap().iter().any(|x| x["rule_id"] == "seed-cert-basics"));
    // (b) vendor data cert is boosted for a data/ML JD
    let g = d["rules_applied"].as_array().unwrap().iter().find(|x| x["rule_id"].as_str().unwrap().starts_with("seed-cert-") && x["effect"].as_str().unwrap().contains("Google Data Analytics"));
    assert!(g.is_some() && sel(&d, "certs").contains(&"c3".to_string()), "{}", d["rules_applied"]);
    // forbid on every role: locked roles stay
    let u = ok(&r, "POST", "/api/playbook/rules", Some(json!({"area": "roles", "text": "Drop old roles", "matcher": {"kind": "role", "title_regex": "."}, "action": {"forbid": true}}))).await;
    assert_eq!(u["status"], "active");
    let l = job(&a, &r, JD).await;
    let roles = detail(&r, &l).await["pool"]["roles"].clone();
    for x in roles.as_array().unwrap().iter().filter(|x| x["locked"] == true) {
        assert_eq!(x["selected"], true, "locked role dropped: {x}");
    }
    // rule text with private values is rejected
    for t in ["Hide Jane Doe from the list", "Do not mention Acme Robotics", "email me at a@b.co"] {
        let (st, m) = call(&r, "POST", "/api/playbook/rules", Some(json!({"area": "certs", "text": t, "matcher": {"kind": "cert", "title_regex": "x"}, "action": {"forbid": true}}))).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{t}: {m}");
    }
    // the review prompt carries the owner rules (redacted) and the writer prompt the summary rule
    let p = a.conn.lock().unwrap().query_row("SELECT output_json FROM job_steps WHERE job_id=? AND name='build_payload'", [&on], |r| r.get::<_, String>(0)).unwrap();
    assert!(p.contains("Owner rules (must follow)") && p.contains("22-32 words"), "{p}");
}

#[tokio::test]
async fn approve_regression_and_metrics() {
    let (a, r) = setup();
    let id = job(&a, &r, JD).await;
    assert_eq!(detail(&r, &id).await["approved"], false);
    let ap = ok(&r, "POST", &format!("/api/jobs/{id}/approve"), None).await;
    assert!(ap["keep_corrections"].as_i64().unwrap() > 0);
    let d = detail(&r, &id).await;
    assert_eq!(d["approved"], true);
    assert!(d["corrections"].as_array().unwrap().iter().all(|c| c["action"] == "keep" && c["weight"] == 0.5));
    assert!(ok(&r, "GET", "/api/regression/latest", None).await.is_null());
    let run = ok(&r, "POST", "/api/regression/run", None).await;
    assert_eq!((run["cases"].as_i64(), run["passing"].as_i64()), (Some(1), Some(1)), "{run}");
    assert_eq!(ok(&r, "GET", "/api/regression/latest", None).await["passing"], 1);
    // a rule that forbids an approved cert makes the replay fail with a diff; approving a proposal reports the impact
    let cert = d["pool"]["certs"].as_array().unwrap().iter().find(|c| c["selected"] == true).unwrap()["title"].as_str().unwrap().to_string();
    let u = ok(&r, "POST", "/api/playbook/rules", Some(json!({"area": "certs", "text": "Never show that cert", "matcher": {"kind": "cert", "title_regex": regex_escape(&cert)}, "action": {"forbid": true}}))).await;
    let run = ok(&r, "POST", "/api/regression/run", None).await;
    assert_eq!((run["cases"].as_i64(), run["passing"].as_i64()), (Some(1), Some(0)));
    let res = &run["results"][0];
    assert!(res["pass"] == false && res["diff"]["certs"]["missing"].as_array().unwrap().iter().any(|m| m == cert.as_str()), "{res}");
    // disabling the rule restores the pass
    ok(&r, "POST", &format!("/api/playbook/rules/{}/disable", u["id"].as_str().unwrap()), None).await;
    assert_eq!(ok(&r, "POST", "/api/regression/run", None).await["passing"], 1);
    // impact of enabling it again
    let v = ok(&r, "POST", &format!("/api/playbook/rules/{}/enable", u["id"].as_str().unwrap()), None).await;
    assert_eq!((v["impact"]["would_change"].as_i64(), v["impact"]["of"].as_i64()), (Some(1), Some(1)));
    assert_eq!(v["impact"]["summary"], "would change 1 of 1 approved resumes");
    // metrics
    let m = ok(&r, "GET", "/api/learning/metrics", None).await;
    assert_eq!(m["jobs"][0]["job_id"], id.as_str());
    assert_eq!(m["jobs"][0]["approved"], true);
    assert_eq!(m["approval_rate"], 1.0);
    assert!(m["jobs"][0]["tokens_in"].as_i64().unwrap() > 0 && m["active_rules"].as_i64().unwrap() >= 8);
    assert_eq!((m["regression"]["cases"].as_i64(), m["regression"]["passing"].as_i64()), (Some(1), Some(1)));
    assert_eq!(m["corrections_per_job_trend"], json!([0]));
    assert!(m["rule_hits"].as_i64().unwrap() > 0 && m.get("tokens_saved_est").is_some() && m.get("proposed_rules").is_some());
    assert_eq!(call(&r, "POST", "/api/jobs/nope/approve", None).await.0, StatusCode::NOT_FOUND);
}

fn regex_escape(s: &str) -> String {
    format!("^{}", s.chars().map(|c| if c.is_alphanumeric() || c == ' ' { c.to_string() } else { format!("\\{c}") }).collect::<String>())
}

#[tokio::test]
async fn edit_persists_across_rerun_and_is_a_correction() {
    let (a, r) = setup();
    let id = job(&a, &r, JD).await;
    assert_eq!(call(&r, "POST", &format!("/api/jobs/{id}/edit"), Some(json!({"path": "summary.7", "text": "x"}))).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(call(&r, "POST", &format!("/api/jobs/{id}/edit"), Some(json!({"path": "bogus", "text": "x"}))).await.0, StatusCode::BAD_REQUEST);
    let e = ok(&r, "POST", &format!("/api/jobs/{id}/edit"), Some(json!({"path": "summary.0", "text": "Hands-on ML engineer.", "reason_tags": ["wrong_tone"]}))).await;
    assert_eq!(e["after"], "Hands-on ML engineer.");
    assert!(e["before"].as_str().unwrap().len() > 3);
    a.run_next().await.unwrap();
    assert_eq!(detail(&r, &id).await["tailored"]["summary"][0], "Hands-on ML engineer.");
    // a full re-run from the start keeps the owner's line
    let (st, _) = call(&r, "POST", &format!("/api/jobs/{id}/retry?from=parse_jd"), None).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    a.run_next().await.unwrap();
    let d = detail(&r, &id).await;
    assert_eq!(d["tailored"]["summary"][0], "Hands-on ML engineer.");
    let c = d["corrections"].as_array().unwrap().iter().find(|c| c["action"] == "edit").unwrap();
    assert_eq!((c["area"].as_str(), c["item_id"].as_str(), c["after"]["text"].as_str()), (Some("summary"), Some("summary.0"), Some("Hands-on ML engineer.")));
    assert_eq!(c["reason_tags"], json!(["wrong_tone"]));
    // a bullet edit, then a second edit of the same line replaces it
    ok(&r, "POST", &format!("/api/jobs/{id}/edit"), Some(json!({"path": "e1.b0", "text": "Shipped Python and C# services."}))).await;
    a.run_next().await.unwrap();
    let e = ok(&r, "POST", &format!("/api/jobs/{id}/edit"), Some(json!({"path": "e1.b0", "text": "Shipped Python services."}))).await;
    assert_eq!(e["before"], "Shipped Python and C# services.");
    a.run_next().await.unwrap();
    let t = detail(&r, &id).await["tailored"].clone();
    assert!(t["experience"].as_array().unwrap().iter().any(|x| x["bullets"].as_array().unwrap().iter().any(|b| b == "Shipped Python services.")), "{t}");
}
