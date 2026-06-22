//! inventory HTTP service (scaffold) — telemetry-wired axum server. Flesh out the
//! domain behavior per docs (DB spans / cache / reverse-hop) at implementation.
use axum::{Router, routing::get};

#[tracing::instrument(fields(otel.kind = "server"))]
async fn handle() -> &'static str {
    tracing::info!("inventory handled request");
    "inventory ok"
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let telemetry = playground_telemetry::init("inventory")?;
    let app = Router::new()
        .route("/", get(handle))
        .route("/healthz", get(|| async { "ok" }));
    let addr = std::env::var("ADDR").unwrap_or_else(|_| "0.0.0.0:8089".into());
    tracing::info!(%addr, "inventory HTTP listening");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;
    telemetry.shutdown();
    Ok(())
}
