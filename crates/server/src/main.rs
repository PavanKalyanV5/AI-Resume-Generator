use resume_server::{api::router, App};
use std::path::Path;
#[cfg(feature = "embeddings")]
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let db = std::env::var("RESUME_DB").unwrap_or_else(|_| "data/app.db".into());
    let port: u16 = std::env::var("PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(8787);
    let app = App::open(Path::new(&db), Path::new("data/out"))?;
    #[cfg(feature = "embeddings")]
    {
        let a = app.clone();
        tokio::spawn(async move {
            match tokio::task::spawn_blocking(resume_core::embed::FastEmbedder::try_new).await {
                Ok(Ok(e)) => {
                    a.set_embedder(Some(Arc::new(e)));
                    eprintln!("embedding model ready");
                    a.notify("success", "model.ready", "Local matching model ready", "Semantic matching is now on.");
                }
                Ok(Err(e)) => {
                    eprintln!("keyword scoring only: {e}");
                    a.notify("warn", "model.download_failed", "Local model unavailable", "Using keyword matching only; semantic matching needs the model download.");
                }
                Err(e) => eprintln!("keyword scoring only: {e}"),
            }
        });
    }
    tokio::spawn(app.clone().sweeper_loop());
    for _ in 0..2 {
        tokio::spawn(app.clone().worker_loop());
    }
    resume_server::remote::autostart(&app).await;
    let l = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    eprintln!("listening on 127.0.0.1:{port}");
    axum::serve(l, resume_ui::with_ui(router(app))).await?;
    Ok(())
}
