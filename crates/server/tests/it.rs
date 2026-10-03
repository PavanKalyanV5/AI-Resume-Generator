use resume_server::{api::job_detail, cancel_job, create_job, ctx, events_after, providers::{mock::MockProvider, ProviderError}, sweep, App, Fault, STEPS};
use resume_core::payload::build_payload;
use std::{path::{Path, PathBuf}, process::Command, sync::Arc};

const JD: &str = "Senior Backend Engineer\nRequirements:\n- 5+ years of Python and Kubernetes\n- Strong C# and .NET experience\nNice to have:\n- Terraform\n";
const REAL: [&str; 7] = ["Jane", "Doe", "jane.doe", "Acme", "Globex", "555", "0104477"];

fn resume() -> String {
    serde_json::json!({
        "profile": {"name": "Jane Doe", "email": "jane.doe@acme-robotics.test", "phone": "+1 555 010 4477", "role": "Engineer", "location": "Springfield"},
        "summary": ["Engineer who ships."],
        "experience": [{"id": "x1", "role": "Backend Engineer", "organization": "Acme Robotics Pvt Ltd", "client": "Globex Corp", "dateLabel": "2020-2024",
            "bullets": ["Built Python services on Kubernetes for Globex Corp, cutting latency 40%", "Ran lunches at Acme Robotics", "Maintained C# and .NET billing code"]}],
        "skills": [{"label": "Cloud", "skills": ["Python", "Terraform", "Kubernetes"]}]
    }).to_string()
}

fn setup(p: Arc<MockProvider>) -> (Arc<App>, PathBuf) {
    let dir = std::env::temp_dir().join(format!("rs-test-{}", uuid::Uuid::new_v4()));
    let app = App::open(&dir.join("app.db"), &dir.join("out")).unwrap();
    app.conn.lock().unwrap().execute("INSERT INTO resume VALUES(1,?)", [resume()]).unwrap();
    *app.provider_override.lock().unwrap() = Some(p);
    // backoff is tiny so retries are fast
    let app = Arc::try_unwrap(app).ok().map(|mut a| { a.backoff_ms = 1; Arc::new(a) }).unwrap();
    (app, dir)
}

fn new_job(a: &App) -> String {
    create_job(&mut a.conn.lock().unwrap(), JD, "mock", &[], None).unwrap()
}
fn status(a: &App, id: &str) -> String {
    a.conn.lock().unwrap().query_row("SELECT status FROM jobs WHERE id=?", [id], |r| r.get(0)).unwrap()
}
fn files(d: &Path, out: &mut Vec<String>) {
    for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
        out.push(e.file_name().to_string_lossy().into());
        if e.path().is_dir() { files(&e.path(), out); }
    }
}
fn assert_clean_outputs(a: &App, id: &str) {
    let dir = a.out.join(id);
    let mut names = vec![];
    files(&dir, &mut names);
    names.sort();
    assert_eq!(names, ["resume.docx", "resume.pdf"], "no tmp/partial files");
    let pdf = Command::new("pdftotext").arg(dir.join("resume.pdf")).arg("-").output().unwrap();
    let docx = Command::new("unzip").args(["-p"]).arg(dir.join("resume.docx")).arg("word/document.xml").output().unwrap();
    for (n, o) in [("pdf", pdf), ("docx", docx)] {
        let t = String::from_utf8_lossy(&o.stdout).to_string();
        assert!(t.contains("Acme") && t.contains("Jane"), "{n} has real details restored");
        assert!(!regex_tok(&t), "{n} contains placeholder tokens");
    }
}
fn regex_tok(t: &str) -> bool {
    ["[ORG_", "[CLIENT_", "[PERSON_", "[EMAIL_", "[PHONE_"].iter().any(|k| t.contains(k))
}

#[tokio::test]
async fn happy_path_and_audit_is_redacted() {
    // Force LaTeX/tectonic by removing the native engine override
    std::env::remove_var("RESUME_PDF_ENGINE");
    let p = Arc::new(MockProvider::default());
    let (a, _d) = setup(p.clone());
    let id = new_job(&a);
    assert!(a.run_next().await.unwrap());
    assert_eq!(status(&a, &id), "done");
    assert_eq!(p.calls(), 1);
    assert_clean_outputs(&a, &id);
    let d = job_detail(&a.conn.lock().unwrap(), &id).unwrap().unwrap();
    let audit = d["audit"].to_string();
    for r in REAL { assert!(!audit.contains(r), "audit leaked {r}"); }
    assert!(audit.contains("ORG") && d["files"].as_array().unwrap().len() == 2);
    assert!(d["original"]["profile"]["name"] == "Jane Doe" && d["tailored"]["profile"]["name"] == "Jane Doe");
    // ai step is the only non-local event
    let evs = events_after(&a.conn.lock().unwrap(), &id, 0).unwrap();
    assert!(evs.iter().filter(|e| e["local_only"] == false).all(|e| e["step"] == "ai_tailor"));
    assert!(evs.iter().any(|e| e["message"].as_str().unwrap().starts_with("Sending ")));
}

#[tokio::test]
async fn crash_at_every_boundary_resumes_without_duplicate_ai_call() {
    std::env::set_var("RESUME_PDF_ENGINE", "native");
    let faults: Vec<Fault> = (0..STEPS.len()).flat_map(|i| [Fault::After(i), Fault::Mid(i)]).collect();
    for f in faults {
        let p = Arc::new(MockProvider::default());
        let (a, _d) = setup(p.clone());
        let id = new_job(&a);
        *a.fault.lock().unwrap() = Some(f);
        assert!(a.run_next().await.is_err(), "{f:?} should abort");
        // the dead worker's lease expires; a fresh worker resumes
        a.conn.lock().unwrap().execute("UPDATE jobs SET lease_until=0 WHERE status='running'", []).unwrap();
        sweep(&mut a.conn.lock().unwrap(), false).unwrap();
        a.run_next().await.unwrap();
        assert_eq!(status(&a, &id), "done", "{f:?}");
        assert_eq!(p.calls(), 1, "{f:?}: duplicate AI call");
        assert_clean_outputs(&a, &id);
        // vault is deterministic: stored payload equals one rebuilt from scratch
        let c = ctx(&resume(), JD, &[]).unwrap();
        let fresh = build_payload(&c.resume, &c.sel, &c.jd, &c.vault).unwrap();
        let stored: String = a.conn.lock().unwrap().query_row("SELECT output_json FROM job_steps WHERE name='build_payload'", [], |r| r.get(0)).unwrap();
        assert_eq!(serde_json::from_str::<serde_json::Value>(&stored).unwrap()["user"], fresh.user, "{f:?}");
    }
}

#[tokio::test]
async fn transient_retries_permanent_fails() {
    std::env::set_var("RESUME_PDF_ENGINE", "native");
    let p = Arc::new(MockProvider::with_failures(vec![ProviderError::Transient("429".into()), ProviderError::Transient("503".into())]));
    let (a, _d) = setup(p.clone());
    let id = new_job(&a);
    a.run_next().await.unwrap();
    assert_eq!(status(&a, &id), "done");
    assert_eq!(p.calls(), 3);

    let p = Arc::new(MockProvider::with_failures(vec![ProviderError::Permanent("bad key".into())]));
    let (a, _d) = setup(p.clone());
    let id = new_job(&a);
    a.run_next().await.unwrap();
    assert_eq!(status(&a, &id), "failed");
    assert_eq!(p.calls(), 1);

    let p = Arc::new(MockProvider::with_failures(vec![ProviderError::Transient("t".into()); 9]));
    let (a, _d) = setup(p.clone());
    let id = new_job(&a);
    a.run_next().await.unwrap();
    assert_eq!((status(&a, &id).as_str(), p.calls()), ("dead", 4));
}

#[tokio::test]
async fn token_budget_is_enforced_before_sending() {
    std::env::set_var("RESUME_PDF_ENGINE", "native");
    let p = Arc::new(MockProvider::default());
    let (a, _d) = setup(p.clone());
    let id = create_job(&mut a.conn.lock().unwrap(), JD, "mock", &[], Some(5)).unwrap();
    a.run_next().await.unwrap();
    assert_eq!((status(&a, &id).as_str(), p.calls()), ("failed", 0));
}

#[tokio::test]
async fn cancel_stops_at_step_boundary() {
    std::env::set_var("RESUME_PDF_ENGINE", "native");
    let p = Arc::new(MockProvider::default());
    let (a, _d) = setup(p.clone());
    let id = new_job(&a);
    assert!(cancel_job(&mut a.conn.lock().unwrap(), &id).unwrap());
    assert!(!a.run_next().await.unwrap());
    // cancel mid-run: next boundary sees it and stops
    let id2 = new_job(&a);
    *a.fault.lock().unwrap() = Some(Fault::After(0));
    assert!(a.run_next().await.is_err());
    cancel_job(&mut a.conn.lock().unwrap(), &id2).unwrap();
    assert_eq!(p.calls(), 0);
    assert_eq!(status(&a, &id2), "cancelled");
    assert!(!a.out.join(&id2).join("resume.pdf").exists());
}

#[tokio::test]
async fn sse_replay_returns_only_later_events() {
    std::env::set_var("RESUME_PDF_ENGINE", "native");
    let (a, _d) = setup(Arc::new(MockProvider::default()));
    let id = new_job(&a);
    a.run_next().await.unwrap();
    let c = a.conn.lock().unwrap();
    let all = events_after(&c, &id, 0).unwrap();
    assert!(all.len() > 8);
    let mid = all[3]["seq"].as_i64().unwrap();
    let later = events_after(&c, &id, mid).unwrap();
    assert_eq!(later.len(), all.len() - 4);
    assert!(later.iter().all(|e| e["seq"].as_i64().unwrap() > mid));
}
