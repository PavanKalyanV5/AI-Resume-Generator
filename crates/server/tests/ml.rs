//! Learning loop through the HTTP API: rewrite skipping, ranker feedback, labels, applications, recommendations (fake data only).
use axum::{body::{to_bytes, Body}, http::{Request, StatusCode}, Router};
use resume_core::{embed::HashEmbedder, ml::{ranker::BulletRanker, ModelBundle}, payload::build_payload};
use resume_server::{api::router, ctx, providers::mock::MockProvider, App};
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Arc};
use tower::ServiceExt;

const JD: &str = "Backend Engineer at Initech\nRequirements:\n- Python and Kubernetes\n- Terraform\n";
const STRONG: &str = "Built Python services on Kubernetes with Terraform pipelines, cutting latency 40%";

fn resume() -> Value {
    json!({"profile": {"name": "Jane Doe", "email": "jane.doe@example.org", "phone": "+1 555 010 4477", "role": "Engineer", "location": "Springfield"},
        "summary": ["Engineer who ships."],
        "experience": [{"id": "x1", "role": "Backend Engineer", "organization": "Acme Robotics Pvt Ltd", "dateLabel": "Jan 2020 - Dec 2024",
            "bullets": [STRONG, "Maintained billing code for enterprise customers", "Wrote deployment scripts for the platform team"]}],
        "skills": [{"label": "Cloud", "skills": ["Python", "Terraform", "Kubernetes"]}]})
}

fn open(dir: &PathBuf) -> Arc<App> {
    let a = App::open(&dir.join("app.db"), &dir.join("out")).unwrap();
    *a.provider_override.lock().unwrap() = Some(Arc::new(MockProvider::default()));
    a.set_embedder(Some(Arc::new(HashEmbedder::default())));
    a
}
fn setup() -> (Arc<App>, Router, PathBuf) {
    let dir = std::env::temp_dir().join(format!("rs-ml-{}", uuid::Uuid::new_v4()));
    let a = open(&dir);
    a.conn.lock().unwrap().execute("INSERT INTO resume VALUES(1,?)", [resume().to_string()]).unwrap();
    let r = router(a.clone());
    (a, r, dir)
}

async fn call(app: &Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut r = Request::builder().method(method).uri(uri);
    let b = match body { Some(v) => { r = r.header("content-type", "application/json"); Body::from(v.to_string()) } None => Body::empty() };
    let resp = app.clone().oneshot(r.body(b).unwrap()).await.unwrap();
    let st = resp.status();
    let b = to_bytes(resp.into_body(), 16 << 20).await.unwrap();
    (st, serde_json::from_slice(&b).unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&b).into())))
}
async fn new_job(r: &Router, jd: &str) -> String {
    let (st, v) = call(r, "POST", "/api/jobs", Some(json!({"jd_text": jd, "provider": "mock"}))).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    v["id"].as_str().unwrap().to_string()
}
async fn detail(r: &Router, id: &str) -> Value {
    call(r, "GET", &format!("/api/jobs/{id}"), None).await.1
}

#[tokio::test]
async fn strong_bullets_are_not_sent_and_job_still_completes() {
    let (a, r, _d) = setup();
    let id = new_job(&r, JD).await;
    assert!(a.run_next().await.unwrap());
    let d = detail(&r, &id).await;
    assert_eq!(d["job"]["status"], "done", "{d}");
    let user = d["audit"]["redacted_payload"]["user"].as_str().unwrap();
    assert!(!user.contains("Terraform pipelines") && user.contains("billing code"), "{user}");
    let cx = ctx(&resume().to_string(), JD, &[]).unwrap();
    let full = build_payload(&cx.resume, &cx.sel, &cx.jd, &cx.vault).unwrap().user;
    assert!(user.len() < full.len(), "{} !< {}", user.len(), full.len());
    let out: Value = a.conn.lock().unwrap().query_row("SELECT output_json FROM job_steps WHERE job_id=? AND name='build_payload'", [&id], |r| r.get::<_, String>(0)).map(|s| serde_json::from_str(&s).unwrap()).unwrap();
    assert_eq!(out["skipped"], 1);
    assert!(out["tokens_saved_est"].as_u64().unwrap() > 0);
    let evs = resume_server::events_after(&a.conn.lock().unwrap(), &id, 0).unwrap();
    assert!(evs.iter().any(|e| e["message"].as_str().unwrap().contains("Skipping 1 bullets already strong, saved ~")));
    // the skipped bullet is kept verbatim in the output
    assert!(d["tailored"]["experience"][0]["bullets"].as_array().unwrap().iter().any(|b| b == STRONG));
    // sent bullets were observed as outcomes
    assert_eq!(a.ml.lock().unwrap().rewrite.n(), 2);
    assert_eq!(call(&r, "POST", "/api/ml/retrain", None).await.0, StatusCode::ACCEPTED);
}

#[tokio::test]
async fn feedback_shifts_ranker_and_persists_across_reopen() {
    let (a, r, dir) = setup();
    let id = new_job(&r, JD).await;
    let url = format!("/api/jobs/{id}/feedback");
    assert_eq!(call(&r, "POST", &url, Some(json!({"bullet_id": "e0.b0", "action": "nope"}))).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(call(&r, "POST", &url, Some(json!({"bullet_id": "e9.b9", "action": "keep"}))).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(a.ml.lock().unwrap().ranker.confidence(), 0.0);
    for i in 0..12 {
        let (st, v) = call(&r, "POST", &url, Some(json!({"bullet_id": format!("e0.b{}", i % 3), "action": if i % 3 == 0 { "keep" } else { "remove" }}))).await;
        assert_eq!(st, StatusCode::OK, "{v}");
    }
    assert!(call(&r, "POST", &url, Some(json!({"bullet_id": "e0.b1", "action": "edit"}))).await.0 == StatusCode::OK);
    {
        let m = a.ml.lock().unwrap();
        assert_eq!(m.ranker.n(), 12, "edit is stored but is not a keep/remove label");
        assert!(m.ranker.confidence() > 0.0);
        assert_ne!(m.ranker.model, BulletRanker::prior());
    }
    let cards = call(&r, "GET", "/api/ml/cards", None).await.1;
    assert_eq!(cards["signals"]["feedback"], 13);
    assert_eq!(cards["cards"].as_array().unwrap().len(), 7);
    // selection still works with a confident ranker
    assert!(a.run_next().await.unwrap());
    assert_eq!(detail(&r, &id).await["job"]["status"], "done");

    let (w, conf) = { let m = a.ml.lock().unwrap(); (m.ranker.model.w.clone(), m.ranker.confidence()) };
    let b = open(&dir);
    let m = b.ml.lock().unwrap();
    assert_eq!((m.ranker.n(), m.ranker.model.w.clone(), m.ranker.confidence()), (12, w, conf));
}

#[tokio::test]
async fn bundle_round_trips_through_settings_and_corrupt_falls_back() {
    let (a, _r, dir) = setup();
    a.ml_do(|b| b.jd.correct("rust embedded firmware rtos", "embedded", "mid")).await.unwrap();
    let want = a.ml.lock().unwrap().clone();
    let stored: String = a.conn.lock().unwrap().query_row("SELECT value FROM settings WHERE key='ml_bundle'", [], |r| r.get(0)).unwrap();
    assert_eq!(ModelBundle::from_json(&stored).unwrap(), want);
    assert_eq!(*open(&dir).ml.lock().unwrap(), want);
    a.conn.lock().unwrap().execute("UPDATE settings SET value='{not json' WHERE key='ml_bundle'", []).unwrap();
    assert_eq!(*open(&dir).ml.lock().unwrap(), ModelBundle::default());
}

#[tokio::test]
async fn jd_class_neighbours_and_correction() {
    let (a, r, dir) = setup();
    let be = "Backend Engineer\nRequirements:\n- Java, Spring, microservices, PostgreSQL, Kafka\n- REST APIs and scalable backend services\n";
    let be2 = "Senior Backend Developer\nRequirements:\n- Java, Spring Boot, Kafka, PostgreSQL\n- Microservices and REST APIs\n";
    let fe = "Frontend Developer\nRequirements:\n- React, TypeScript, CSS, accessibility\n- Responsive UI components in the browser\n";
    let ids = [new_job(&r, be).await, new_job(&r, be2).await, new_job(&r, fe).await];
    for _ in 0..3 { assert!(a.run_next().await.unwrap()); }
    let d = detail(&r, &ids[0]).await;
    assert_eq!(d["jd_class"]["family"][0][0], "backend", "{}", d["jd_class"]);
    assert!(d["jd_class"]["seniority"].is_array() && d["jd_class"]["evidence"].is_array() && d["jd_class"]["confirmed"].is_null());
    let sim = d["similar_jobs"].as_array().unwrap();
    assert_eq!(sim[0]["id"], ids[1].as_str(), "{sim:?}");
    assert!(sim[0]["similarity"].as_f64().unwrap() > 0.15 && sim[0]["company"].is_string() && sim[0]["role"].is_string());
    assert!(!sim.iter().any(|s| s["id"] == ids[0].as_str()));
    let kinds: Vec<String> = a.conn.lock().unwrap().prepare("SELECT DISTINCT vec_kind FROM job_ml").unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
    assert_eq!(kinds.len(), 1, "one vector kind, never mixed");
    let n: i64 = a.conn.lock().unwrap().query_row("SELECT count(*) FROM jd_corpus", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 3);

    let url = format!("/api/jobs/{}/jd-class", ids[2]);
    assert_eq!(call(&r, "POST", &url, Some(json!({"family": "bogus", "seniority": "mid"}))).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(call(&r, "POST", &url, Some(json!({"family": "frontend", "seniority": "junior"}))).await.0, StatusCode::NO_CONTENT);
    assert_eq!(detail(&r, &ids[2]).await["jd_class"]["confirmed"], json!({"family": "frontend", "seniority": "junior"}));
    let b = open(&dir);
    assert_eq!(b.ml.lock().unwrap().jd.corrections, 1);
    assert_eq!(detail(&router(b), &ids[2]).await["jd_class"]["confirmed"]["family"], "frontend");
}

#[tokio::test]
async fn applications_upsert_funnel_and_outcome_threshold() {
    let (_a, r, _d) = setup();
    let ids = futures_ids(&r, 25).await;
    assert_eq!(call(&r, "POST", "/api/applications", Some(json!({"job_id": ids[0], "stage": "maybe"}))).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(call(&r, "POST", "/api/applications", Some(json!({"job_id": "nope", "stage": "applied"}))).await.0, StatusCode::NOT_FOUND);
    assert_eq!(call(&r, "POST", "/api/applications", Some(json!({"job_id": ids[0], "stage": "applied", "note": "via referral"}))).await.0, StatusCode::OK);
    assert_eq!(call(&r, "POST", "/api/applications", Some(json!({"job_id": ids[0], "stage": "interview"}))).await.0, StatusCode::OK);
    let list = call(&r, "GET", "/api/applications", None).await.1;
    let l = list.as_array().unwrap();
    assert_eq!(l.len(), 1);
    assert_eq!((l[0]["stage"].as_str(), l[0]["note"].as_str(), l[0]["company"].as_str(), l[0]["history"].as_array().unwrap().len()), (Some("interview"), Some("via referral"), Some("Initech"), 2));
    call(&r, "POST", "/api/applications", Some(json!({"job_id": ids[1], "stage": "ghosted"}))).await;
    let f = call(&r, "GET", "/api/applications/funnel", None).await.1;
    assert_eq!((f["total"].clone(), f["interviews"].clone(), f["ghosted"].clone(), f["responded"].clone()), (json!(2), json!(1), json!(1), json!(1)), "{f}");

    // outcome prediction appears only from 25 labelled applications
    assert!(detail(&r, &ids[2]).await["outcome"].is_null());
    for (i, id) in ids.iter().enumerate().skip(2) {
        call(&r, "POST", "/api/applications", Some(json!({"job_id": id, "stage": if i % 2 == 0 { "response" } else { "ghosted" }}))).await;
        let o = detail(&r, &ids[2]).await["outcome"].clone();
        if i < 24 { assert!(o.is_null(), "{i}: {o}"); } else { assert!(o["p"].is_f64() && o["lo"].is_f64() && o["hi"].is_f64(), "{o}"); }
    }
}

async fn futures_ids(r: &Router, n: usize) -> Vec<String> {
    let mut v = vec![];
    for _ in 0..n { v.push(new_job(r, JD).await); }
    v
}

#[tokio::test]
async fn recommendations_shapes() {
    let (a, r, _d) = setup();
    for (i, t) in [json!(["python", "docker", "go"]), json!(["python", "kubernetes", "docker"]), json!(["docker", "go", "aws"]), json!(["python", "terraform", "aws"])].iter().enumerate() {
        a.conn.lock().unwrap().execute("INSERT INTO jd_corpus VALUES(?,?,?)", rusqlite::params![format!("j{i}"), t.to_string(), format!("2026-0{}-15T00:00:00Z", i + 1)]).unwrap();
    }
    let (st, v) = call(&r, "GET", "/api/ml/recommendations", None).await;
    assert_eq!(st, StatusCode::OK);
    let learn = v["learn_next"].as_array().unwrap();
    assert!(learn.iter().any(|x| x["skill"] == "docker") && learn[0]["why"].is_string() && learn[0]["demand_share"].is_f64());
    assert_eq!(v["trends"][0].as_array().unwrap().len(), 2, "[skill, [[month, count]..]]");
    assert!(v["resume_additions"].is_array());
}
