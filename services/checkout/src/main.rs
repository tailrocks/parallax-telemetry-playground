//! Checkout HTTP service (axum) — the orchestrator / trace spine. One
//! `GET /checkout` fans out to pricing (gRPC), inventory (HTTP) and
//! recommendation (HTTP), producing a multi-service distributed trace
//! (HTTP SERVER → gRPC CLIENT + HTTP CLIENT spans → each downstream SERVER span).
//!
//! Deliberate-chaos + canary knobs are per-request query params (so scenarios are
//! deterministic) and also honor ambient flags:
//!   ?fail=1      payment failure → 502 + error issue        (B1)
//!   ?slow=<ms>   injected latency                            (B11)
//!   ?canary=1    plant a redaction canary corpus in span/log (A18)

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Json, Router, extract::Query, routing::get};
use playground_proto::pricing::v1::QuoteRequest;
use playground_proto::pricing::v1::pricing_client::PricingClient;
use serde::Deserialize;
use serde_json::{Value, json};

/// Query flags arrive as `1`/`true`/`yes`/`on`; serde's bool wants `true`/`false`.
fn de_flag<'de, D: serde::Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    let s = String::deserialize(d)?;
    Ok(matches!(s.as_str(), "1" | "true" | "yes" | "on"))
}

#[derive(Deserialize)]
struct CheckoutParams {
    #[serde(default = "default_sku")]
    sku: String,
    #[serde(default = "default_qty")]
    quantity: u32,
    #[serde(default, deserialize_with = "de_flag")]
    fail: bool,
    #[serde(default)]
    slow: u64,
    #[serde(default, deserialize_with = "de_flag")]
    canary: bool,
}

fn default_sku() -> String {
    "WIDGET-1".into()
}
fn default_qty() -> u32 {
    1
}

fn flag(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

#[tracing::instrument(skip(p), fields(otel.kind = "server"))]
async fn checkout(Query(p): Query<CheckoutParams>) -> impl IntoResponse {
    if p.slow > 0 {
        tokio::time::sleep(std::time::Duration::from_millis(p.slow)).await;
    }
    if p.canary || flag("CANARY") {
        // A18: plant a redaction canary corpus so backends can be compared on
        // raw-vs-scrubbed. These are FAKE secrets for redaction testing only.
        tracing::warn!(
            canary.email = "alice@example.com",
            canary.token = "sk-live-CANARY1234567890",
            canary.card = "4111111111111111",
            canary.jwt = "eyJhbGciOiJIUzI1NiJ9.CANARY.sig",
            "canary payload planted for redaction comparison"
        );
    }
    if p.fail || flag("PAYMENT_FAILURE") {
        // B1: deliberate failure → error issue + ERROR span status (502).
        tracing::error!(sku = %p.sku, "payment failure (chaos flag)");
        return (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": "payment failed", "sku": p.sku })),
        );
    }

    let pricing = quote(&p.sku, p.quantity).await;
    let inventory = reserve(&p.sku, p.quantity).await;
    let recommendation = recommend(&p.sku).await;

    match pricing {
        Ok((total, currency)) => {
            tracing::info!(sku = %p.sku, total, "checkout ok");
            (
                StatusCode::OK,
                Json(json!({
                    "sku": p.sku,
                    "quantity": p.quantity,
                    "total_minor": total,
                    "currency": currency,
                    "inventory": inventory.unwrap_or(json!({"error": "unavailable"})),
                    "recommendation": recommendation.unwrap_or(json!({"error": "unavailable"})),
                })),
            )
        }
        Err(err) => {
            tracing::error!(error = %err, "pricing call failed");
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": err.to_string() })),
            )
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
