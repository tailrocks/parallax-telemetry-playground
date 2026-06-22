//! Recommendation HTTP service — returns related SKUs (cache-backed in the full
//! design). A SERVER span in the orchestrated checkout trace.
use axum::{Json, Router, extract::Query, routing::get};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
struct Recommend {
    sku: String,
}

#[tracing::instrument(skip(p), fields(otel.kind = "server"))]
async fn recommend(Query(p): Query<Recommend>) -> Json<Value> {
    let recs = vec![format!("{}-ACCESSORY", p.sku), "WIDGET-2".to_string()];
    tracing::info!(sku = %p.sku, count = recs.len(), "recommended");
    Json(json!({ "sku": p.sku, "recommended": recs }))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let telemetry = playground_telemetry::init("recommendation")?;
    let app = Router::new()
        .route("/recommend", get(recommend))
        .route("/healthz", get(|| async { "ok" }));
    let addr = std::env::var("ADDR").unwrap_or_else(|_| "0.0.0.0:8090".into());
    tracing::info!(%addr, "recommendation HTTP listening");
    axum::serve(tokio::net::TcpListener::bind(&addr).await?, app).await?;
    telemetry.shutdown();
    Ok(())
}
