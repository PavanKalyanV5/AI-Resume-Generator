//! Bounded self-repair of grounding violations, bold rendering and newest-first order (fake data, scripted MockProvider).
use axum::{body::{to_bytes, Body}, http::{Request, StatusCode}, Router};
use resume_core::embed::HashEmbedder;
use resume_server::{api::router, events_after, providers::mock::MockProvider, App};
use serde_json::{json, Value};
use std::{path::PathBuf, process::Command, sync::Arc};
use tower::ServiceExt;

const JD: &str = "Senior Backend Engineer at Initech\nRequirements:\n- 5+ years of Python and Kubernetes\n";

fn resume() -> Value {
    json!({"profile": {"name": "Jane Doe", "email": "jane.doe@example.org", "phone": "+1 555 010 4477", "role": "Backend Engineer", "location": "Springfield"},
        "summary": ["Engineer who ships."],
        "experience": [{"id": "x1", "role": "Backend Engineer", "organization": "Acme Robotics Pvt Ltd", "dateLabel": "Jan 2020 - Dec 2024",
            "bullets": ["Built Python services on Kubernetes, cutting latency 40%", "Maintained C# billing code"]}],
        "projects": [{"name": "OldProj", "dateLabel": "Sep 2019", "description": "Python tool", "bullets": ["Wrote Python tooling"]}, {"name": "NewProj", "dateLabel": "Sep 2026", "description": "Kubernetes tool", "bullets": ["Wrote Kubernetes tooling"]}],
        "certifications": [{"title": "Cert Old", "issuer": "Org", "dateLabel": "Jan 2020 · Expires Jan 2030"}, {"title": "Cert New", "issuer": "Org", "dateLabel": "Jun 2024"}],
        "skills": [{"label": "Cloud", "skills": ["Python", "Kubernetes", "C#"]}]})
}

fn setup(p: Arc<MockProvider>) -> (Arc<App>, Router, PathBuf) {
    let dir = std::env::temp_dir().join(format!("rs-repair-{}", uuid::Uuid::new_v4()));
    let app = App::open(&dir.join("app.db"), &dir.join("out")).unwrap();
    app.conn.lock().unwrap().execute("INSERT INTO resume VALUES(1,?)", [resume().to_string()]).unwrap();
    *app.provider_override.lock().unwrap() = Some(p);
    app.set_embedder(Some(Arc::new(HashEmbedder::default())));
    let app = Arc::try_unwrap(app).ok().map(|mut a| { a.backoff_ms = 1; Arc::new(a) }).unwrap();
    let r = router(app.clone());
    (app, r, dir)
}

async fn call(app: &Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut r = Request::builder().method(method).uri(uri);
    let b = match body { Some(v) => { r = r.header("content-type", "application/json"); Body::from(v.to_string()) } None => Body::empty() };
    let resp = app.clone().oneshot(r.body(b).unwrap()).await.unwrap();
    let st = resp.status();
    let b = to_bytes(resp.into_body(), 16 << 20).await.unwrap();
    (st, serde_json::from_slice(&b).unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&b).into())))
}

async fn run(a: &Arc<App>, r: &Router) -> (String, Value) {
    let (st, v) = call(r, "POST", "/api/jobs", Some(json!({"jd_text": JD, "provider": "mock"}))).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let id = v["id"].as_str().unwrap().to_string();
    assert!(a.run_next().await.unwrap());
    let d = call(r, "GET", &format!("/api/jobs/{id}"), None).await.1;
    assert_eq!(d["job"]["status"], "done", "{d}");
    (id, d)
}

/// Summary line 2 and bullet e0.b0 invent GraphQL / Rust, which the profile does not have.
fn tailor() -> String {
    json!({"summary": ["Backend engineer with **5+ years** of experience building **Python** and **Kubernetes** platforms that keep services dependable, scalable and easy to operate for fast growing product teams and their customers.", "Delivered **GraphQL** gateways and **Rust** tooling at scale."],
        "bullets": [{"id": "e0.b0", "text": "Built **Python** services on Kubernetes, cutting latency 40% with GraphQL"}]}).to_string()
}

fn messages(a: &App, id: &str) -> Vec<String> {
    events_after(&a.conn.lock().unwrap(), id, 0).unwrap().iter().map(|e| e["message"].as_str().unwrap().to_string()).collect()
}
fn feedback(a: &App, id: &str, action: &str) -> i64 {
    a.conn.lock().unwrap().query_row("SELECT count(*) FROM ml_feedback WHERE job_id=? AND action=?", [id, action], |r| r.get(0)).unwrap()
}

#[tokio::test]
async fn repair_call_fixes_violating_lines_and_renders_bold_newest_first() {
    std::env::set_var("RESUME_PDF_ENGINE", "native");
    let p = Arc::new(MockProvider::default());
    *p.tailor.lock().unwrap() = Some(tailor());
    *p.repair.lock().unwrap() = Some(json!({"lines": [{"id": "e0.b0", "text": "Built **Python** services on Kubernetes, cutting latency **40%**"},
        {"id": "summary.1", "text": "Delivered **Python** and **Kubernetes** platforms at scale for product teams with steady, measurable reliability."}]}).to_string());
    let (a, r, _d) = setup(p.clone());
    let (id, d) = run(&a, &r).await;
    assert_eq!((p.calls(), p.repair_calls()), (1, 1), "exactly one extra call");
    let g = d["grounding"].as_array().unwrap();
    assert_eq!(g.len(), 2, "{g:?}");
    assert!(g.iter().all(|e| e["repaired"] == true), "{g:?}");
    let t = &d["tailored"];
    assert_eq!(t["experience"][0]["bullets"][0], "Built **Python** services on Kubernetes, cutting latency **40%**");
    assert!(t["summary"][1].as_str().unwrap().starts_with("Delivered **Python** and **Kubernetes**") && !t.to_string().contains("GraphQL"));
    let m = messages(&a, &id);
    assert!(m.iter().any(|x| x.starts_with("Asked the AI to repair 2 lines that made unsupported claims (sent ~")), "{m:?}");
    assert!(m.iter().any(|x| x.starts_with("Repair outcome: 2 of 2 lines fixed")), "{m:?}");
    assert_eq!(feedback(&a, &id, "rewrite_repaired"), 2);

    // bold survives into the files, projects/certs are newest first, no raw asterisks
    let dir = a.out.join(&id);
    let docx = String::from_utf8_lossy(&Command::new("unzip").arg("-p").arg(dir.join("resume.docx")).arg("word/document.xml").output().unwrap().stdout).to_string();
    let pdf = String::from_utf8_lossy(&Command::new("pdftotext").arg(dir.join("resume.pdf")).arg("-").output().unwrap().stdout).to_string();
    assert!(docx.split("<w:r>").any(|r| r.contains(">Python</w:t>") && r.contains("<w:b")), "bold run in docx");
    assert!(!docx.contains("**") && !pdf.contains('*'), "no raw asterisks");
    let pos = |t: &str, s: &str| t.find(s).unwrap_or_else(|| panic!("{s} missing"));
    for t in [&docx, &pdf] {
        assert!(pos(t, "NewProj") < pos(t, "OldProj") && pos(t, "Cert New") < pos(t, "Cert Old"));
    }
}

#[tokio::test]
async fn failed_repair_falls_back_gracefully() {
    std::env::set_var("RESUME_PDF_ENGINE", "native");
    let p = Arc::new(MockProvider::default());
    *p.tailor.lock().unwrap() = Some(tailor());
    *p.repair.lock().unwrap() = Some(json!({"lines": [{"id": "e0.b0", "text": "Built services with GraphQL"}, {"id": "summary.1", "text": "Rust everywhere"}]}).to_string());
    let (a, r, _d) = setup(p.clone());
    let (id, d) = run(&a, &r).await;
    assert_eq!((p.calls(), p.repair_calls()), (1, 1));
    assert!(d["grounding"].as_array().unwrap().iter().all(|e| e["repaired"] == false));
    let t = d["tailored"].to_string();
    assert!(t.contains("Built Python services on Kubernetes, cutting latency 40%") && !t.contains("GraphQL") && !t.contains("Rust everywhere"), "{t}");
    assert!(d["tailored"]["summary"][0].as_str().unwrap().contains("Backend engineer with **5+ years**"), "valid summary line kept");
    assert!(d["tailored"]["summary"][1].as_str().unwrap().contains("Senior Backend Engineer"), "fallback summary uses the JD role: {t}");
    assert_eq!(feedback(&a, &id, "rewrite_reverted"), 2);
    assert!(messages(&a, &id).iter().any(|x| x.starts_with("Repair outcome: 0 of 2")));
}

const L1: &str = "Backend Engineer with **5+ years** of experience building **Python** and **Kubernetes** platforms that keep services dependable, scalable and easy to operate for fast growing product teams and their customers.";
const GOOD: &str = "Delivers **dependable** backend platforms that keep **document workflows** fast, observable and easy to evolve, helping growing product teams ship changes with confidence and fewer production surprises.";

/// One job with scripted tailor + repair replies; returns (provider, job detail, job id, app).
async fn go(summary: Value, repair: &str) -> (Arc<MockProvider>, Value, String, Arc<App>) {
    let p = Arc::new(MockProvider::default());
    *p.tailor.lock().unwrap() = Some(json!({"summary": summary, "bullets": []}).to_string());
    *p.repair.lock().unwrap() = Some(repair.to_string());
    let (a, r, _d) = setup(p.clone());
    let (id, d) = run(&a, &r).await;
    (p, d, id, a)
}
fn rep(id: &str, text: &str) -> String { json!({"lines": [{"id": id, "text": text}]}).to_string() }

#[tokio::test]
async fn wrong_years_claim_is_fixed_not_deleted() {
    std::env::set_var("RESUME_PDF_ENGINE", "native");
    let bad = L1.replace("**5+ years**", "**3+ years**");
    let (p, d, ..) = go(json!([bad, GOOD]), &rep("summary.0", L1)).await;
    assert_eq!((p.calls(), p.repair_calls()), (1, 1));
    let s = d["tailored"]["summary"][0].as_str().unwrap();
    assert!(s.contains("Backend Engineer") && s.contains("5+ years") && s.contains("Python"), "{s}");
}

#[tokio::test]
async fn repair_that_drops_years_gets_the_template_and_renders_clean() {
    std::env::set_var("RESUME_PDF_ENGINE", "native");
    let bad = L1.replace("**5+ years** of experience", "experience");
    let nope = "Backend Engineer with experience specializing in **Python** and **Kubernetes** platforms that keep services dependable, scalable and easy to operate for fast growing product teams and customers.";
    let (p, d, id, a) = go(json!([bad, GOOD]), &rep("summary.0", nope)).await;
    assert_eq!((p.calls(), p.repair_calls()), (1, 1), "no second repair");
    let s = d["tailored"]["summary"][0].as_str().unwrap();
    assert!(s.starts_with("Senior Backend Engineer with **5+ years** of experience building **") , "{s}");
    assert_eq!(d["tailored"]["summary"][1], GOOD);
    let dir = a.out.join(&id);
    let pdf = String::from_utf8_lossy(&Command::new("pdftotext").arg(dir.join("resume.pdf")).arg("-").output().unwrap().stdout).to_string();
    let docx = String::from_utf8_lossy(&Command::new("unzip").arg("-p").arg(dir.join("resume.docx")).arg("word/document.xml").output().unwrap().stdout).to_string();
    assert!(pdf.contains("5+ years") && !pdf.contains("**") && !docx.contains("**"));
}

#[tokio::test]
async fn copied_line_is_repaired() {
    std::env::set_var("RESUME_PDF_ENGINE", "native");
    let copied = "Built **Python** services on Kubernetes, cutting latency 40% across the platform";
    let (p, d, ..) = go(json!([L1, copied]), &rep("summary.1", GOOD)).await;
    assert_eq!((p.calls(), p.repair_calls()), (1, 1));
    assert_eq!(d["tailored"]["summary"][1], GOOD);
    assert!(d["grounding"].to_string().contains("CopiedFromSource"));
}

#[tokio::test]
async fn word_count_shares_the_single_repair_call_and_bad_repairs_are_accepted() {
    std::env::set_var("RESUME_PDF_ENGINE", "native");
    let short = "Backend platforms **Python** specialist for product teams.";
    let (p, d, ..) = go(json!([L1.replace("Backend Engineer", "Backend Engineer"), short, short]), &json!({"lines": [{"id": "summary.1", "text": GOOD}, {"id": "summary.2", "text": "Still **short** but grounded."}]}).to_string()).await;
    assert_eq!((p.calls(), p.repair_calls()), (1, 1), "one extra call for both lines");
    assert_eq!(d["tailored"]["summary"][1], GOOD);
    assert_eq!(d["tailored"]["summary"][2], "Still **short** but grounded.", "out-of-range but grounding-clean is accepted");
}
