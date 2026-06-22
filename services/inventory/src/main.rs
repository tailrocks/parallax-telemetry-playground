//! Inventory HTTP service — reserves stock for a SKU. A SERVER span; checkout
//! calls it as part of the orchestrated trace. (DB-backed reservation is the
//! next step per the design doc.)
use axum::{Json, Router, extract::Query, routing::get};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
struct Reserve {
    sku: String,
    #[serde(default = "one")]
    quantity: u32,
}
fn one() -> u32 {
    1
}

#[tracing::instrument(skip(p), fields(otel.kind = "server"))]
async fn reserve(Query(p): Query<Reserve>) -> Json<Value> {
    let in_stock = !p.sku.is_empty();
    tracing::info!(sku = %p.sku, quantity = p.quantity, in_stock, "reserved");
    Json(json!({ "sku": p.sku, "reserved": p.quantity, "in_stock": in_stock }))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let telemetry = playground_telemetry::init("inventory")?;
    let app = Router::new()
        .route("/reserve", get(reserve))
        .route("/healthz", get(|| async { "ok" }));
    let addr = std::env::var("ADDR").unwrap_or_else(|_| "0.0.0.0:8089".into());
    tracing::info!(%addr, "inventory HTTP listening");
    axum::serve(tokio::net::TcpListener::bind(&addr).await?, app).await?;
    telemetry.shutdown();
    Ok(())
}
