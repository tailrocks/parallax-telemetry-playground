//! Checkout HTTP service (axum) — the orchestrator. A `GET /checkout` call
//! quotes a price from the pricing gRPC service, producing a distributed trace
//! (HTTP SERVER span → gRPC CLIENT span → pricing SERVER span).

use axum::{Json, Router, extract::Query, routing::get};
use playground_proto::pricing::v1::QuoteRequest;
use playground_proto::pricing::v1::pricing_client::PricingClient;
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
struct CheckoutParams {
    #[serde(default = "default_sku")]
    sku: String,
    #[serde(default = "default_qty")]
    quantity: u32,
}

fn default_sku() -> String {
    "WIDGET-1".into()
}
fn default_qty() -> u32 {
    1
}

#[tracing::instrument(skip(params), fields(otel.kind = "server"))]
async fn checkout(Query(params): Query<CheckoutParams>) -> Json<Value> {
    match quote(&params.sku, params.quantity).await {
        Ok((total, currency)) => {
            tracing::info!(sku = %params.sku, total, "checkout ok");
            Json(json!({
                "sku": params.sku,
                "quantity": params.quantity,
                "total_minor": total,
                "currency": currency,
            }))
        }
        Err(err) => {
            tracing::error!(error = %err, "pricing call failed");
            Json(json!({ "error": err.to_string() }))
        }
    }
}

#[tracing::instrument(fields(otel.kind = "client"))]
async fn quote(sku: &str, quantity: u32) -> anyhow::Result<(u64, String)> {
    let endpoint = std::env::var("PRICING_ENDPOINT")
        .unwrap_or_else(|_| "http://pricing:50051".into());
    let mut client = PricingClient::connect(endpoint).await?;
    let response = client
        .quote(QuoteRequest {
            sku: sku.to_string(),
            quantity,
        })
        .await?
        .into_inner();
    Ok((response.total_minor, response.currency))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let telemetry = playground_telemetry::init("checkout")?;
    let app = Router::new()
        .route("/checkout", get(checkout))
        .route("/healthz", get(|| async { "ok" }));
    let addr = std::env::var("CHECKOUT_ADDR").unwrap_or_else(|_| "0.0.0.0:8088".into());
    tracing::info!(%addr, "checkout HTTP listening");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;
    telemetry.shutdown();
    Ok(())
}
