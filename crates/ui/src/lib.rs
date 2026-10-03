//! Serves the built web UI (embedded, or from $RESUME_UI_DIR) with SPA fallback to index.html.
use axum::{
    body::Body,
    http::{header, StatusCode, Uri},
    response::{IntoResponse, Response},
    Router,
};
use std::borrow::Cow;

#[cfg(feature = "embed-ui")]
#[derive(rust_embed::RustEmbed)]
#[folder = "../../apps/web/dist"]
struct Dist;

fn load(path: &str) -> Option<Cow<'static, [u8]>> {
    if let Some(dir) = std::env::var_os("RESUME_UI_DIR") {
        if path.split('/').any(|s| s == "..") {
            return None;
        }
        return std::fs::read(std::path::Path::new(&dir).join(path)).ok().map(Cow::Owned);
    }
    #[cfg(feature = "embed-ui")]
    return Dist::get(path).map(|f| f.data);
    #[cfg(not(feature = "embed-ui"))]
    None
}

fn mime(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript",
        "css" => "text/css",
        "json" | "map" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "txt" => "text/plain",
        _ => "application/octet-stream",
    }
}

async fn serve(uri: Uri) -> Response {
    let p = uri.path().trim_start_matches('/');
    if p == "api" || p.starts_with("api/") {
        return StatusCode::NOT_FOUND.into_response();
    }
    let (p, data) = match load(p).filter(|_| !p.is_empty()) {
        Some(d) => (p, d),
        None => match load("index.html") {
            Some(d) => ("index.html", d),
            None => return (StatusCode::NOT_FOUND, "web UI not built (npm run build in apps/web)").into_response(),
        },
    };
    Response::builder()
        .header(header::CONTENT_TYPE, mime(p))
        .header(header::CONTENT_SECURITY_POLICY, "default-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self' data:; frame-ancestors 'none'").body(Body::from(data.into_owned())).unwrap()
}

/// Add the UI as the router fallback (API routes keep precedence).
pub fn with_ui(r: Router) -> Router {
    r.fallback(serve)
}
