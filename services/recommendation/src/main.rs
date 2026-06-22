//! Recommendation HTTP service — related SKUs (cache-backed in the full design).
//! Chaos: ?leak=<n> grows a process-held buffer to emulate a cache/memory leak
//! (B6) and adds latency, so the slow degradation is visible over repeated calls.
use axum::{Json, Router, extract::Query, routing::get};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::{Mutex, OnceLock};

fn leak_store() -> &'static Mutex<Vec<Vec<u8>>> {
    static STORE: OnceLock<Mutex<Vec<Vec<u8>>>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(Vec::new()))
}

#[derive(Deserialize)]
struct Recommend {
    sku: String,
    #[serde(default)]
    leak: usize,
    /// B13: slow "asset"/response latency.
    #[serde(default)]
    slow: u64,
}

#[tracing::instrument(skip(p), fields(otel.kind = "server"))]
async fn recommend(Query(p): Query<Recommend>) -> Json<Value> {
    if p.slow > 0 {
        tokio::time::sleep(std::time::Duration::from_millis(p.slow)).await;
    }
    if p.leak > 0 {
        let mut store = leak_store().lock().unwrap();
        store.push(vec![0u8; p.leak * 1024]); // never freed → leak
        tracing::warn!(kb = p.leak, held = store.len(), "cache leak (chaos)");
    }
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
