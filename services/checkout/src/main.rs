//! Checkout HTTP service (axum) — the orchestrator / trace spine. One
//! `GET /checkout` fans out to pricing (gRPC), inventory (HTTP) and
//! recommendation (HTTP), producing a multi-service distributed trace
//! (HTTP SERVER → gRPC CLIENT + HTTP CLIENT spans → each downstream SERVER span).

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
    // Fan out to the three downstreams; each is its own child span.
    let pricing = quote(&params.sku, params.quantity).await;
    let inventory = reserve(&params.sku, params.quantity).await;
    let recommendation = recommend(&params.sku).await;

    match pricing {
        Ok((total, currency)) => {
            tracing::info!(sku = %params.sku, total, "checkout ok");
            Json(json!({
                "sku": params.sku,
                "quantity": params.quantity,
                "total_minor": total,
                "currency": currency,
                "inventory": inventory.unwrap_or(json!({"error": "unavailable"})),
                "recommendation": recommendation.unwrap_or(json!({"error": "unavailable"})),
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
    let endpoint =
        std::env::var("PRICING_ENDPOINT").unwrap_or_else(|_| "http://pricing:50051".into());
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

#[tracing::instrument(fields(otel.kind = "client"))]
async fn reserve(sku: &str, quantity: u32) -> anyhow::Result<Value> {
    let base = std::env::var("INVENTORY_URL").unwrap_or_else(|_| "http://inventory:8089".into());
    let url = format!("{base}/reserve?sku={sku}&quantity={quantity}");
    Ok(reqwest::get(&url).await?.json::<Value>().await?)
}

#[tracing::instrument(fields(otel.kind = "client"))]
async fn recommend(sku: &str) -> anyhow::Result<Value> {
    let base =
        std::env::var("RECOMMENDATION_URL").unwrap_or_else(|_| "http://recommendation:8090".into());
    let url = format!("{base}/recommend?sku={sku}");
    Ok(reqwest::get(&url).await?.json::<Value>().await?)
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
