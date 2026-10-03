//! SSRF guard. This binary never sets RESUME_ALLOW_PRIVATE_FETCH, so the production guard is active.
use axum::{body::{to_bytes, Body}, http::{Request, StatusCode}, response::Redirect, routing::get, Router};
use resume_server::{api::router, fetch::{fetch_jd, is_public, public_only, FetchErr}, App};
use serde_json::json;
use std::net::IpAddr;
use tower::ServiceExt;

#[test]
fn address_classes() {
    for bad in ["127.0.0.1", "10.1.2.3", "172.16.0.1", "192.168.1.1", "169.254.169.254", "100.64.0.1", "0.0.0.0", "224.0.0.1", "::1", "::", "fe80::1", "fd00::1", "::ffff:127.0.0.1", "::ffff:169.254.169.254", "64:ff9b::a00:1"] {
        assert!(!is_public(bad.parse::<IpAddr>().unwrap()), "{bad}");
    }
    for ok in ["8.8.8.8", "1.1.1.1", "93.184.216.34", "2606:4700:4700::1111"] {
        assert!(is_public(ok.parse::<IpAddr>().unwrap()), "{ok}");
    }
    assert!(!public_only("localhost", "8.8.8.8".parse().unwrap()) && !public_only("foo.localhost", "8.8.8.8".parse().unwrap()));
}

#[tokio::test]
async fn route_rejects_private_and_non_http() {
    let dir = std::env::temp_dir().join(format!("rs-ssrf-{}", uuid::Uuid::new_v4()));
    let app = router(App::open(&dir.join("a.db"), &dir.join("out")).unwrap());
    for u in ["http://127.0.0.1:9/", "http://10.0.0.5/x", "http://169.254.169.254/latest/meta-data/", "http://localhost:9/", "http://[::1]:9/", "http://100.64.0.1/", "http://2130706433/", "file:///etc/passwd", "ftp://example.com/x", "gopher://127.0.0.1/"] {
        let r = Request::builder().method("POST").uri("/api/jd/fetch").header("content-type", "application/json").body(Body::from(json!({"url": u}).to_string())).unwrap();
        let resp = app.clone().oneshot(r).await.unwrap();
        let st = resp.status();
        let b = String::from_utf8_lossy(&to_bytes(resp.into_body(), 1 << 16).await.unwrap()).to_string();
        assert_eq!(st, StatusCode::BAD_REQUEST, "{u}: {b}");
    }
}

#[tokio::test]
async fn every_redirect_hop_is_checked() {
    let app = Router::new()
        .route("/meta", get(|| async { Redirect::temporary("http://169.254.169.254/latest/meta-data/") }))
        .route("/lan", get(|| async { Redirect::temporary("http://10.0.0.1/admin") }))
        .route("/file", get(|| async { Redirect::temporary("file:///etc/passwd") }));
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let u = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    // first hop (the loopback fake) is allowed by this test guard; the redirect targets are not
    let guard = |_: &str, ip: IpAddr| ip == "127.0.0.1".parse::<IpAddr>().unwrap();
    for p in ["/meta", "/lan", "/file"] {
        let e = fetch_jd(&format!("{u}{p}"), &guard).await.err().unwrap();
        assert!(matches!(e, FetchErr::Bad(_)), "{p}: {e:?}");
    }
}
