//! JD fetcher against a local fake site. RESUME_ALLOW_PRIVATE_FETCH=1 (set once, tests only) lets the route reach 127.0.0.1.
use axum::{body::{to_bytes, Body}, http::{Request, StatusCode}, response::{Html, IntoResponse, Redirect}, routing::get, Router};
use resume_server::{api::router, App};
use serde_json::{json, Value};
use tower::ServiceExt;

fn long(s: &str) -> String {
    format!("{s} {}", "We build reliable data platforms with Python and Kubernetes for our customers. ".repeat(4))
}

async fn site() -> String {
    let ld = format!(r#"<html><head><title>x</title><script type="application/ld+json">{{"@context":"https://schema.org","@graph":[{{"@type":"JobPosting","title":"Staff Engineer","hiringOrganization":{{"@type":"Organization","name":"Initech"}},"description":"&lt;p&gt;{}&lt;/p&gt;&lt;ul&gt;&lt;li&gt;Python&lt;/li&gt;&lt;/ul&gt;"}}]}}</script></head><body>ignored</body></html>"#, long("Join us."));
    let html = format!("<html><head><title>Jobs</title><style>.a{{}}</style></head><body><nav>HOME ABOUT LOGIN</nav><main><h1>Backend Dev at Hooli</h1><p>{}</p><ul><li>Rust</li><li>SQL</li></ul><script>var x=1;</script></main><footer>COPYRIGHT NOISE</footer></body></html>", long("Work with us."));
    let app = Router::new()
        .route("/ld", get(move || { let h = ld.clone(); async move { Html(h) } }))
        .route("/html", get(move || { let h = html.clone(); async move { Html(h) } }))
        .route("/wall", get(|| async { Html("<html><body><h1>Please sign in</h1></body></html>") }))
        .route("/r1", get(|| async { Redirect::temporary("/r2") }))
        .route("/r2", get(|| async { Redirect::temporary("/html") }))
        .route("/loop", get(|| async { Redirect::temporary("/loop") }))
        .route("/big", get(|| async { "a".repeat(2 * 1024 * 1024 + 10).into_response() }))
        .route("/slow-404", get(|| async { StatusCode::NOT_FOUND }));
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    url
}

async fn fetch(url: &str) -> (StatusCode, Value) {
    std::env::set_var("RESUME_ALLOW_PRIVATE_FETCH", "1");
    let dir = std::env::temp_dir().join(format!("rs-fetch-{}", uuid::Uuid::new_v4()));
    let app = router(App::open(&dir.join("a.db"), &dir.join("out")).unwrap());
    let r = Request::builder().method("POST").uri("/api/jd/fetch").header("content-type", "application/json").body(Body::from(json!({"url": url}).to_string())).unwrap();
    let resp = app.oneshot(r).await.unwrap();
    let st = resp.status();
    let b = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    (st, serde_json::from_slice(&b).unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&b).into())))
}

#[tokio::test]
async fn jsonld_job_posting_is_preferred() {
    let u = site().await;
    let (st, v) = fetch(&format!("{u}/ld")).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!((v["source"].as_str(), v["title"].as_str(), v["company"].as_str()), (Some("jsonld"), Some("Staff Engineer"), Some("Initech")));
    let t = v["text"].as_str().unwrap();
    assert!(t.starts_with("Staff Engineer\nCompany: Initech") && t.contains("reliable data platforms") && t.contains("- Python") && !t.contains("<p>"), "{t}");
}

#[tokio::test]
async fn html_main_text_without_chrome_and_redirects_followed() {
    let u = site().await;
    for path in ["/html", "/r1"] {
        let (st, v) = fetch(&format!("{u}{path}")).await;
        assert_eq!(st, StatusCode::OK, "{v}");
        assert_eq!((v["source"].as_str(), v["title"].as_str(), v["company"].as_str()), (Some("html"), Some("Backend Dev at Hooli"), Some("Hooli")));
        let t = v["text"].as_str().unwrap();
        assert!(t.contains("reliable data platforms") && t.contains("- Rust\n- SQL"), "{t}");
        for noise in ["HOME ABOUT", "COPYRIGHT", "var x"] { assert!(!t.contains(noise), "{noise}"); }
    }
}

#[tokio::test]
async fn failures_are_clear() {
    let u = site().await;
    let (st, v) = fetch(&format!("{u}/wall")).await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(v.as_str().unwrap().contains("paste"));
    assert_eq!(fetch(&format!("{u}/loop")).await.0, StatusCode::BAD_GATEWAY, "redirect cap");
    assert_eq!(fetch(&format!("{u}/big")).await.0, StatusCode::BAD_GATEWAY, "2MB cap");
    assert_eq!(fetch(&format!("{u}/slow-404")).await.0, StatusCode::BAD_GATEWAY);
    assert_eq!(fetch("not a url").await.0, StatusCode::BAD_REQUEST);
}
