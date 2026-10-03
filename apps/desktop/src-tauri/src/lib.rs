//! Desktop/Android shell: runs a local server in-process on 127.0.0.1:<free port> and points the webview at it (same
//! origin as the UI, so no CORS and SSE just works). Desktop: the full resume server. Android (or feature `companion`):
//! only the companion proxy, which forwards to a paired desktop.
use std::path::Path;
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

#[cfg(any(target_os = "android", feature = "companion"))]
fn start_server(dir: &Path) -> anyhow::Result<(u16, String)> {
    let (p, k, _) = tauri::async_runtime::block_on(resume_companion::serve(dir))?;
    Ok((p, format!("?k={k}")))
}

#[cfg(not(any(target_os = "android", feature = "companion")))]
use full::start_server;
#[cfg(not(any(target_os = "android", feature = "companion")))]
mod full {
use std::{net::TcpListener, path::Path, sync::Arc};
/// Open the app under `dir`, spawn background loops, serve on a free (or $PORT) port. Returns the port.
pub fn start_server(dir: &Path) -> anyhow::Result<(u16, String)> {
    std::fs::create_dir_all(dir)?;
    std::env::set_var("RESUME_MODEL_DIR", dir.join("models"));
    let app = resume_server::App::open(&dir.join("app.db"), &dir.join("out"))?;
    let port = std::env::var("PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(0);
    let l = TcpListener::bind(("127.0.0.1", port))?;
    l.set_nonblocking(true)?;
    let port = l.local_addr()?.port();
    tauri::async_runtime::spawn(async move {
        {
            let a = app.clone();
            tokio::spawn(async move {
                if let Ok(Ok(e)) = tokio::task::spawn_blocking(resume_core::embed::FastEmbedder::try_new).await {
                    a.set_embedder(Some(Arc::new(e)));
                }
            });
        }
        tokio::spawn(app.clone().sweeper_loop());
        for _ in 0..2 {
            tokio::spawn(app.clone().worker_loop());
        }
        let l = tokio::net::TcpListener::from_std(l).expect("listener");
        let _ = axum_serve(l, app).await;
    });
    Ok((port, String::new()))
}

async fn axum_serve(l: tokio::net::TcpListener, app: Arc<resume_server::App>) -> std::io::Result<()> {
    axum::serve(l, resume_ui::with_ui(resume_server::api::router(app))).await
}

}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // `--serve-only`: headless server for smoke tests (data dir: $RESUME_DATA_DIR or ./resume-data).
    if std::env::args().any(|a| a == "--serve-only") {
        let dir = std::env::var_os("RESUME_DATA_DIR").unwrap_or("resume-data".into());
        tauri::async_runtime::block_on(async {
            let (port, _) = start_server(Path::new(&dir)).expect("server");
            println!("listening on http://127.0.0.1:{port}/");
            std::future::pending::<()>().await
        });
        return;
    }
    let mut b = tauri::Builder::default();
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        b = b.plugin(tauri_plugin_single_instance::init(|app, _, _| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.set_focus();
            }
        }));
    }
    b.setup(|app| {
        let dir = app.path().app_data_dir()?;
        let (port, q) = start_server(&dir)?;
        let url = format!("http://127.0.0.1:{port}/{q}").parse()?;
        let w = WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url)).title("Resume Generator");
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        let w = w.inner_size(1200.0, 850.0).min_inner_size(800.0, 600.0);
        w.build()?;
        Ok(())
    })
    .run(tauri::generate_context!())
    .expect("error while running tauri application");
}
