//! Fit/page target, embeddings, PII inspector, heuristics API and grounding through the HTTP API (fake data only).
use async_trait::async_trait;
use axum::{body::{to_bytes, Body}, http::{HeaderMap, Request, StatusCode}, Router};
use resume_core::{embed::HashEmbedder, latex::pdf_pages};
use resume_server::{api::router, providers::{mock::MockProvider, Provider, ProviderError, Reply}, App};
use serde_json::{json, Value};
use std::{collections::BTreeSet, path::PathBuf, process::Command, sync::Arc};
use tower::ServiceExt;

const JD: &str = "Senior Backend Engineer at Initech\nRequirements:\n- 5+ years of Python and Kubernetes\n- Strong C# and .NET experience\n- Experience with Terraform\n";

fn long_resume() -> Value {
    let role = |r: usize, name: &str, org: &str, dates: &str| {
        let bullets: Vec<String> = (0..9).map(|n| format!("Zq{r}x{n} Built and operated Python services on Kubernetes with Terraform pipelines handling {}00 requests per second, reducing incident response effort for the platform teams across several regions and improving release confidence, while mentoring teammates, writing design documents, reviewing code from peers in other groups, coordinating on-call rotations, and presenting quarterly results to stakeholders in engineering and product management", n + 2)).collect();
        json!({"id": format!("r{r}"), "role": name, "organization": org, "dateLabel": dates, "bullets": bullets})
    };
    json!({"profile": {"name": "Jane Doe", "email": "jane.doe@example.org", "phone": "+1 555 010 4477", "role": "Backend Engineer", "location": "Springfield"},
        "summary": ["Engineer who ships."],
        "experience": [role(0, "Senior Backend Engineer", "Acme Robotics Pvt Ltd", "Jan 2019 - Present"), role(1, "Backend Engineer", "Globex Corp", "Jan 2016 - Dec 2018"), role(2, "Software Engineer", "Hooli Systems", "Jan 2013 - Dec 2015")],
        "projects": (0..3).map(|i| json!({"name": format!("Project {i}"), "description": "A Python and Kubernetes side project with a long description of what it is for and who uses it", "techStack": ["Python", "Kubernetes"], "bullets": ["Shipped it with Terraform"]})).collect::<Vec<_>>(),
        "certifications": (0..5).map(|i| json!({"title": format!("Certification {i}"), "issuer": "Initech Academy", "dateLabel": "2022"})).collect::<Vec<_>>(),
        "skills": [{"label": "Cloud", "skills": ["Python", "Terraform", "Kubernetes", "C#", ".NET"]}, {"label": "Other", "skills": ["Git", "Linux", "Bash"]}]})
}

fn short_resume() -> Value {
    json!({"profile": {"name": "Jane Doe", "email": "jane.doe@example.org", "phone": "+1 555 010 4477", "role": "Engineer", "location": "Springfield"},
        "summary": ["Engineer who ships."],
        "experience": [{"id": "x1", "role": "Backend Engineer", "organization": "Acme Robotics Pvt Ltd", "client": "Globex Corp", "dateLabel": "Jan 2020 - Dec 2024",
            "bullets": ["Built Python services on Kubernetes for Globex Corp, cutting latency 40%", "Maintained C# and .NET billing code", "Wrote Python scripts for Terraform deployments",
                "Paired with Dmitri Volkov and wrote to dmitri.volkov@example.net about Initech Solutions billing"]}],
        "skills": [{"label": "Cloud", "skills": ["Python", "Terraform", "Kubernetes"]}]})
}

fn setup(resume: Value, p: Arc<dyn Provider>) -> (Arc<App>, Router, PathBuf) {
    let dir = std::env::temp_dir().join(format!("rs-fit-{}", uuid::Uuid::new_v4()));
    let app = App::open(&dir.join("app.db"), &dir.join("out")).unwrap();
    app.conn.lock().unwrap().execute("INSERT INTO resume VALUES(1,?)", [resume.to_string()]).unwrap();
    *app.provider_override.lock().unwrap() = Some(p);
    app.set_embedder(Some(Arc::new(HashEmbedder::default())));
    let app = Arc::try_unwrap(app).ok().map(|mut a| { a.backoff_ms = 1; Arc::new(a) }).unwrap();
    let r = router(app.clone());
    (app, r, dir)
}

async fn raw(app: &Router, method: &str, uri: &str, body: Option<Value>, hdr: &[(&str, &str)]) -> (StatusCode, HeaderMap, Vec<u8>) {
    let mut r = Request::builder().method(method).uri(uri);
    for (k, v) in hdr { r = r.header(*k, *v); }
    let b = match body { Some(v) => { r = r.header("content-type", "application/json"); Body::from(v.to_string()) } None => Body::empty() };
    let resp = app.clone().oneshot(r.body(b).unwrap()).await.unwrap();
    let (st, h) = (resp.status(), resp.headers().clone());
    (st, h, to_bytes(resp.into_body(), 16 << 20).await.unwrap().to_vec())
}
async fn call(app: &Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let (st, _, b) = raw(app, method, uri, body, &[]).await;
    (st, serde_json::from_slice(&b).unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&b).into())))
}

async fn run_job(a: &Arc<App>, r: &Router, body: Value) -> (String, Value) {
    let (st, v) = call(r, "POST", "/api/jobs", Some(body)).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let id = v["id"].as_str().unwrap().to_string();
    assert!(a.run_next().await.unwrap());
    let (_, d) = call(r, "GET", &format!("/api/jobs/{id}"), None).await;
    (id, d)
}

fn tokens(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut rest = text;
    while let Some(i) = rest.find("Zq") {
        let t: String = rest[i..].chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
        rest = &rest[i + 2..];
        out.insert(t);
    }
    out
}
fn pdf_text(a: &App, id: &str) -> String {
    String::from_utf8_lossy(&Command::new("pdftotext").arg(a.out.join(id).join("resume.pdf")).arg("-").output().unwrap().stdout).into()
}
fn docx_text(a: &App, id: &str) -> String {
    String::from_utf8_lossy(&Command::new("unzip").arg("-p").arg(a.out.join(id).join("resume.docx")).arg("word/document.xml").output().unwrap().stdout).into()
}

#[tokio::test]
async fn target_pages_are_honoured_and_docx_matches_pdf() {
    // Force LaTeX/tectonic by removing the native engine override
    std::env::remove_var("RESUME_PDF_ENGINE");
    let (a, r, _d) = setup(long_resume(), Arc::new(MockProvider::default()));
    let (st, _) = call(&r, "PUT", "/api/settings", Some(json!({"prior_resume_pages": 2}))).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    assert_eq!(call(&r, "GET", "/api/settings", None).await.1["prior_resume_pages"], 2);
    assert_eq!(call(&r, "PUT", "/api/settings", Some(json!({"prior_resume_pages": 12}))).await.0, StatusCode::BAD_REQUEST);

    let (id, d) = run_job(&a, &r, json!({"jd_text": JD, "provider": "mock"})).await;
    assert_eq!(d["job"]["status"], "done", "{d}");
    assert_eq!(d["decisions"]["target_pages"], 2, "{}", d["decisions"]["why"][0]);
    assert!(!d["decisions"]["why"].as_array().unwrap().is_empty());
    assert_eq!(d["fit"]["pages"], 2);
    assert_eq!(pdf_pages(&a.out.join(&id).join("resume.pdf")).unwrap(), 2);
    let (p, x) = (tokens(&pdf_text(&a, &id)), tokens(&docx_text(&a, &id)));
    assert!(!p.is_empty() && p == x, "docx and pdf come from the same fitted resume");
    assert!(d["coverage"]["semantic"].is_array());
    assert!(d["clusters"]["points"].as_array().is_some_and(|p| !p.is_empty()));
    let ids: BTreeSet<&str> = d["graph"]["nodes"].as_array().unwrap().iter().map(|n| n["id"].as_str().unwrap()).collect();
    assert!(d["clusters"]["points"].as_array().unwrap().iter().all(|p| ids.contains(p["id"].as_str().unwrap())));
    assert!(d["near_duplicates"].is_array() && d["facts_summary"]["top_skills"].is_array() && d["redaction_summary"]["total"].is_u64());
    let evs = call(&r, "GET", &format!("/api/jobs/{id}"), None).await.1;
    assert!(evs["steps"].as_array().unwrap().iter().all(|s| s["status"] == "done"));
    let m = resume_server::events_after(&a.conn.lock().unwrap(), &id, 0).unwrap();
    assert!(m.iter().any(|e| e["step"] == "select" && e["message"].as_str().unwrap().contains("local semantic model + keywords")));
    let fit_steps = d["fit"]["steps"].as_array().unwrap();
    for s in fit_steps { assert!(m.iter().any(|e| e["message"] == *s && e["local_only"] == true)); }

    // importing the produced PDF records its page count as the prior resume length
    call(&r, "PUT", "/api/settings", Some(json!({"prior_resume_pages": null}))).await;
    let pdf = std::fs::read(a.out.join(&id).join("resume.pdf")).unwrap();
    let req = Request::builder().method("POST").uri("/api/import").header("x-filename", "old.pdf").body(Body::from(pdf)).unwrap();
    let resp = r.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(call(&r, "GET", "/api/settings", None).await.1["prior_resume_pages"], 2);

    // an explicit 1-page ask wins
    let (id1, d1) = run_job(&a, &r, json!({"jd_text": JD, "provider": "mock", "pages": 1})).await;
    assert_eq!(d1["job"]["status"], "done", "{d1}");
    assert_eq!(d1["decisions"]["target_pages"], 1);
    assert_eq!(d1["fit"]["pages"], 1);
    assert_eq!(pdf_pages(&a.out.join(&id1).join("resume.pdf")).unwrap(), 1);
    assert_eq!(tokens(&pdf_text(&a, &id1)), tokens(&docx_text(&a, &id1)));
    assert_eq!(call(&r, "POST", "/api/jobs", Some(json!({"jd_text": JD, "provider": "mock", "pages": 3}))).await.0, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn pii_masking_reveal_always_and_candidates() {
    std::env::set_var("RESUME_PDF_ENGINE", "native");
    let (a, r, _d) = setup(short_resume(), Arc::new(MockProvider::default()));
    let real = ["Jane", "Doe", "jane.doe", "Acme", "Globex", "010 4477", "0104477", "dmitri", "Volkov", "Initech"];
    let (st, h, b) = raw(&r, "GET", "/api/pii", None, &[]).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(h.get("cache-control").unwrap(), "no-store");
    let body = String::from_utf8(b).unwrap();
    for x in real { assert!(!body.contains(x), "leaked {x}"); }
    let v: Value = serde_json::from_str(&body).unwrap();
    let ent = v["entries"].as_array().unwrap();
    let email = ent.iter().find(|e| e["kind"] == "email" && e["length"] == 20).expect("profile email");
    assert_eq!(email["masked"], "j•••@e•••.org");
    assert!(ent.iter().any(|e| e["kind"] == "org" && e["token"].as_str().unwrap().starts_with("ORG_") && e["source"] == "experience" && e["occurrences"].as_array().unwrap().iter().any(|o| o["area"] == "experience")));
    assert!(ent.iter().find(|e| e["kind"] == "phone").unwrap()["masked"].as_str().unwrap().ends_with("477"));
    let cands = v["candidates"].as_array().unwrap();
    let ce = cands.iter().find(|c| c["kind"] == "email").expect("unlisted email detected");
    let cn = cands.iter().find(|c| c["kind"] == "name").expect("name next to a cue");
    assert_eq!(ce["where"], "resume");

    // reveal needs the confirmation header
    let id = ce["id"].as_str().unwrap();
    assert_eq!(raw(&r, "POST", "/api/pii/reveal", Some(json!({"id": id})), &[]).await.0, StatusCode::BAD_REQUEST);
    let (st, h, b) = raw(&r, "POST", "/api/pii/reveal", Some(json!({"id": id})), &[("x-confirm", "reveal")]).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(h.get("cache-control").unwrap(), "no-store");
    assert_eq!(serde_json::from_slice::<Value>(&b).unwrap()["value"], "dmitri.volkov@example.net");

    // always-redact: accept a candidate, add a custom value, dismiss another
    let (st, acc) = call(&r, "POST", &format!("/api/pii/candidates/{}/accept", cn["id"].as_str().unwrap()), None).await;
    assert_eq!(st, StatusCode::OK, "{acc}");
    let (_, c) = call(&r, "POST", "/api/pii/always", Some(json!({"value": "Initech Solutions"}))).await;
    assert_eq!(call(&r, "POST", "/api/pii/always", Some(json!({"value": "ab"}))).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(call(&r, "POST", &format!("/api/pii/candidates/{}/dismiss", id), None).await.0, StatusCode::NO_CONTENT);
    let (_, v) = call(&r, "GET", "/api/pii", None).await;
    assert_eq!(v["always"].as_array().unwrap().len(), 2);
    assert!(v["candidates"].as_array().unwrap().iter().all(|c| c["kind"] != "name" && c["id"].as_str() != Some(id)));
    assert!(v["always"].as_array().unwrap().iter().any(|x| x["kind"] == "custom" && x["id"] == c["id"]));
    let raw_db = std::fs::read(a.conn.lock().unwrap().path().unwrap()).unwrap();
    assert!(!String::from_utf8_lossy(&raw_db).contains("Initech Solutions"), "always values are encrypted at rest");

    let (jid, d) = run_job(&a, &r, json!({"jd_text": JD, "provider": "mock"})).await;
    assert_eq!(d["job"]["status"], "done", "{d}");
    let payload = d["audit"]["redacted_payload"].to_string();
    for x in ["Initech Solutions", "Dmitri", "Volkov"] { assert!(!payload.contains(x), "payload leaked {x}"); }
    assert!(d["redaction_summary"]["counts"]["ORG"].as_u64().unwrap() > 0 && d["redaction_summary"]["counts"]["PERSON"].as_u64().unwrap() > 0);
    assert!(d["tailored"].to_string().contains("Initech Solutions"), "restored locally");
    assert!(pdf_text(&a, &jid).contains("Initech"));

    // removing an always entry
    assert_eq!(call(&r, "DELETE", &format!("/api/pii/always/{}", c["id"].as_str().unwrap()), None).await.0, StatusCode::NO_CONTENT);
    assert_eq!(call(&r, "DELETE", "/api/pii/always/nope", None).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn heuristics_api_get_put_delete() {
    let (a, r, _d) = setup(short_resume(), Arc::new(MockProvider::default()));
    let (_, v) = call(&r, "GET", "/api/heuristics", None).await;
    assert_eq!(v["effective"], v["defaults"]);
    let bias = v["defaults"]["bias"].clone();
    for bad in [json!({"nope": 1}), json!({"bias": "x"}), json!({"certs_two": 500}), json!({"certs_two": 1.5}), json!({"bias": 1000}), json!({"fit_max_renders": 0}), json!({"signal_hits_full": 0}), json!([1])] {
        assert_eq!(call(&r, "PUT", "/api/heuristics", Some(bad.clone())).await.0, StatusCode::BAD_REQUEST, "{bad}");
    }
    assert!(!a.heuristics_path.exists());
    let (st, v) = call(&r, "PUT", "/api/heuristics", Some(json!({"bias": 0.25, "certs_two": 7}))).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!((v["effective"]["bias"].clone(), v["effective"]["certs_two"].clone(), v["defaults"]["bias"].clone()), (json!(0.25), json!(7), bias));
    assert_eq!(a.heuristics().certs_two, 7);
    let (_, v) = call(&r, "PUT", "/api/heuristics", Some(json!({"w_years": 0.1}))).await;
    assert_eq!((v["effective"]["bias"].clone(), v["effective"]["w_years"].clone()), (json!(0.25), json!(0.1)), "partial updates merge");
    assert!(a.heuristics_path.exists());
    let (_, v) = call(&r, "DELETE", "/api/heuristics", None).await;
    assert_eq!(v["effective"], v["defaults"]);
    assert!(!a.heuristics_path.exists());
}

/// Rewrites bullets: one invents a number, one a JD-only technology, one is a legitimate rephrase.
struct Rewriter;
#[async_trait]
impl Provider for Rewriter {
    async fn complete(&self, _s: &str, user: &str) -> Result<Reply, ProviderError> {
        let v: Value = serde_json::from_str(user).map_err(|e| ProviderError::Permanent(e.to_string()))?;
        let bullets: Vec<Value> = v["bullets"].as_array().into_iter().flatten().map(|b| {
            let t = b["text"].as_str().unwrap();
            let n = if t.contains("latency") { format!("{t} and costs by 95%") } else if t.contains("Terraform") { format!("{t} with GraphQL tooling") } else if t.contains("billing") { t.replacen("Maintained", "Sustained", 1) } else { t.to_string() };
            json!({"id": b["id"], "text": n})
        }).collect();
        let text = json!({"summary": ["Senior Backend Engineer with **5+ years** of experience building **Python** and **Kubernetes** platforms that keep services dependable, scalable and easy to operate for fast growing product teams and customers."], "bullets": bullets}).to_string();
        Ok(Reply { tokens_in: 1, tokens_out: 1, text })
    }
}

#[tokio::test]
async fn grounding_reverts_invented_claims_and_reports_them() {
    std::env::set_var("RESUME_PDF_ENGINE", "native");
    let (a, r, _d) = setup(short_resume(), Arc::new(Rewriter));
    let jd = "Senior Backend Engineer\nRequirements:\n- Python and Kubernetes\n- Experience with GraphQL\n- Terraform\n";
    let (id, d) = run_job(&a, &r, json!({"jd_text": jd, "provider": "mock"})).await;
    assert_eq!(d["job"]["status"], "done", "{d}");
    let g = d["grounding"].as_array().unwrap();
    let kinds: Vec<String> = g.iter().flat_map(|e| e["violations"].as_array().unwrap().iter().map(|v| v["kind"].as_str().unwrap().to_string())).collect();
    assert!(kinds.contains(&"NewNumber".to_string()) && kinds.contains(&"NewTechnology".to_string()), "{g:?}");
    assert_eq!(g.len(), 2, "the legitimate rephrase passes: {g:?}");
    let t = d["tailored"].to_string();
    assert!(!t.contains("95%") && !t.contains("GraphQL tooling") && t.contains("Sustained C# and .NET billing code"));
    assert!(t.contains("cutting latency 40%"));
    for real in ["Jane", "Globex", "Acme"] { assert!(!g.iter().any(|e| e.to_string().contains(real))); }
    let m = resume_server::events_after(&a.conn.lock().unwrap(), &id, 0).unwrap();
    let w: Vec<_> = m.iter().filter(|e| e["level"] == "warn" && e["message"].as_str().unwrap().starts_with("Reverted rewrite of")).collect();
    assert_eq!(w.len(), 2);
    assert!(w.iter().all(|e| e["local_only"] == true));

    let (st, h, b) = raw(&r, "GET", "/api/profile/facts", None, &[]).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(h.get("cache-control").unwrap(), "no-store");
    let f: Value = serde_json::from_slice(&b).unwrap();
    assert!(f["skills"].is_array() && f["skill_timeline"].is_array() && f["skill_matrix"][0]["proficiency"].is_number() && f["years"].is_object());
}

#[tokio::test]
async fn tailored_skill_rows_are_jd_ordered_with_reasons() {
    std::env::set_var("RESUME_PDF_ENGINE", "native");
    let mut res = short_resume();
    res["skills"] = json!([{"label": "Other", "skills": ["Git", "Linux", "Bash"]}, {"label": "Cloud", "skills": ["Python", "Terraform", "Kubernetes"]}]);
    let (a, r, _d) = setup(res, Arc::new(MockProvider::default()));
    let (_, d) = run_job(&a, &r, json!({"jd_text": JD, "provider": "mock"})).await;
    let labels: Vec<_> = d["tailored"]["skills"].as_array().unwrap().iter().map(|c| c["label"].as_str().unwrap().to_string()).collect();
    assert_eq!(labels.first().map(String::as_str), Some("Cloud"), "{labels:?}");
    assert_eq!(d["arrangement"]["skill_rows_order"][0], "Cloud");
    let why = |id: &str| d["arrangement"]["reasons"].as_array().unwrap().iter().find(|x| x["item_id"] == id).map(|x| x["why"].to_string());
    assert_eq!(why("role:0").as_deref(), Some("\"newest first\""), "{}", d["arrangement"]);
    assert_eq!(why("skill_row:0").as_deref(), Some("\"JD relevance\""));
}
