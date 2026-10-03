//! Heuristics config API: effective values + defaults, validated partial updates, reset.
use crate::App;
use axum::{extract::State, http::StatusCode, routing::get, Json, Router};
use resume_core::heuristics::Heuristics;
use serde_json::{json, Value};
use std::sync::Arc;

type E = (StatusCode, String);

pub fn routes() -> Router<Arc<App>> {
    Router::new().route("/api/heuristics", get(show).put(update).delete(reset))
}

/// f32 fields print as their shortest decimal (-0.3, not -0.30000001...).
fn val(h: &Heuristics) -> Value {
    serde_json::from_str(&serde_json::to_string(h).unwrap()).unwrap()
}

fn out(a: &App) -> Json<Value> {
    Json(json!({"effective": val(&a.heuristics()), "defaults": val(&Heuristics::default())}))
}

async fn show(State(a): State<Arc<App>>) -> Json<Value> {
    out(&a)
}

/// Merge `patch` over `base`; unknown keys, non-numbers and out-of-range values are errors.
pub fn merge(base: &Heuristics, patch: &Value) -> Result<Heuristics, String> {
    let (mut cur, def) = (val(base), val(&Heuristics::default()));
    let Some(p) = patch.as_object() else { return Err("body must be a JSON object".into()) };
    for (k, v) in p {
        let d = def.get(k).ok_or_else(|| format!("unknown key {k}"))?;
        if d.is_boolean() {
            if !v.is_boolean() {
                return Err(format!("{k} must be true or false"));
            }
            cur[k] = v.clone();
            continue;
        }
        let n = v.as_f64().ok_or_else(|| format!("{k} must be a number"))?;
        if d.is_u64() {
            let (lo, hi) = if k == "fit_max_renders" { (1.0, 30.0) } else { (0.0, 50.0) };
            if v.as_u64().is_none() || !(lo..=hi).contains(&n) {
                return Err(format!("{k} must be an integer in {lo}..={hi}"));
            }
        } else if !n.is_finite() || !(-100.0..=100.0).contains(&n) || (k == "signal_hits_full" && n <= 0.0) {
            return Err(format!("{k} out of range (-100..=100{})", if k == "signal_hits_full" { ", > 0" } else { "" }));
        }
        cur[k] = v.clone();
    }
    serde_json::from_value(cur).map_err(|e| e.to_string())
}

async fn update(State(a): State<Arc<App>>, Json(b): Json<Value>) -> Result<Json<Value>, E> {
    let h = merge(&a.heuristics(), &b).map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    let (path, text) = (a.heuristics_path.clone(), serde_json::to_string_pretty(&val(&h)).unwrap());
    tokio::task::spawn_blocking(move || -> std::io::Result<()> {
        if let Some(d) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(d)?;
        }
        let tmp = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4()));
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, &path)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(out(&a))
}

async fn reset(State(a): State<Arc<App>>) -> Result<Json<Value>, E> {
    match std::fs::remove_file(&a.heuristics_path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string())),
    }
    Ok(out(&a))
}
