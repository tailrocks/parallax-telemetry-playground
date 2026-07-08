//! Inventory HTTP service — reserves stock for a SKU. Chaos knobs:
//!   ?slow=<ms>  slow "DB query" latency
//!   ?fail=1     reservation failure → 503 + ERROR span (B2)
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::{Json, Router, extract::Query, routing::get};
use serde::Deserialize;
use serde_json::json;
use tracing::Instrument;

#[derive(Deserialize)]
struct Reserve {
    sku: String,
    #[serde(default = "one")]
    quantity: u32,
    #[serde(default)]
    slow: u64,
    #[serde(default, deserialize_with = "de_flag")]
    fail: bool,
}
fn one() -> u32 {
    1
}
fn de_flag<'de, D: serde::Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    let s = String::deserialize(d)?;
    Ok(matches!(s.as_str(), "1" | "true" | "yes" | "on"))
}

async fn reserve(headers: HeaderMap, Query(p): Query<Reserve>) -> impl IntoResponse {
    let span = tracing::info_span!("reserve", otel.kind = "server");
    playground_telemetry::set_parent_from_headers(&span, &headers);
    reserve_inner(p).instrument(span).await
}

async fn reserve_inner(p: Reserve) -> impl IntoResponse {
    if p.slow > 0 {
        tracing::info!(ms = p.slow, "slow db query (chaos)");
        tokio::time::sleep(std::time::Duration::from_millis(p.slow)).await;
    }
    if p.fail {
        playground_telemetry::mark_span_error("out_of_stock");
        tracing::error!(sku = %p.sku, "reservation failed (chaos)");
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "out of stock", "sku": p.sku })),
        );
    }
    tracing::info!(sku = %p.sku, quantity = p.quantity, "reserved");
    (
        StatusCode::OK,
        Json(json!({ "sku": p.sku, "reserved": p.quantity, "in_stock": true })),
    )
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let telemetry = playground_telemetry::init("inventory")?;
    let app = Router::new()
        .route("/reserve", get(reserve))
        .route("/healthz", get(|| async { "ok" }));
    let addr = std::env::var("ADDR").unwrap_or_else(|_| "0.0.0.0:8089".into());
    tracing::info!(%addr, "inventory HTTP listening");
    axum::serve(tokio::net::TcpListener::bind(&addr).await?, app)
        .with_graceful_shutdown(playground_telemetry::shutdown_signal())
        .await?;
    telemetry.shutdown();
    Ok(())
}
