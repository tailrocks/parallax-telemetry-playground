//! Orders service — the async branch. POST an order → a PRODUCER span publishes
//! it to a queue; a background worker consumes it in a CONSUMER span that carries
//! a **span link** back to the producer (the messaging causal edge backends are
//! compared on). An in-process channel stands in for the broker here; the full
//! version uses the compose `broker` (Kafka). Span kinds + link are real OTel.

use axum::{Json, Router, extract::State, routing::post};
use opentelemetry::Context;
use opentelemetry::trace::TraceContextExt;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tracing_opentelemetry::OpenTelemetrySpanExt;

#[derive(Clone)]
struct App {
    tx: mpsc::Sender<(String, Context)>,
}

#[tracing::instrument(skip(state), fields(otel.kind = "producer"))]
async fn publish(State(state): State<App>) -> Json<Value> {
    // Capture the producer span context to link from the consumer.
    let producer_cx = tracing::Span::current().context();
    let order_id = format!("order-{}", std::process::id());
    let _ = state.tx.send((order_id.clone(), producer_cx)).await;
    tracing::info!(%order_id, "order published");
    Json(json!({ "order_id": order_id, "status": "queued" }))
}

#[tracing::instrument(fields(otel.kind = "consumer"))]
async fn consume(order_id: &str, producer_cx: Context) {
    // The CONSUMER span links to the PRODUCER span — the async causal edge.
    tracing::Span::current().add_link(producer_cx.span().span_context().clone());
    tracing::info!(%order_id, "order consumed (linked to producer)");
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let telemetry = playground_telemetry::init("orders")?;
    let (tx, mut rx) = mpsc::channel::<(String, Context)>(64);
    tokio::spawn(async move {
        while let Some((order_id, producer_cx)) = rx.recv().await {
            consume(&order_id, producer_cx).await;
        }
    });
    let app = Router::new()
        .route("/order", post(publish))
        .with_state(App { tx });
    let addr = std::env::var("ADDR").unwrap_or_else(|_| "0.0.0.0:8092".into());
    tracing::info!(%addr, "orders HTTP listening");
    axum::serve(tokio::net::TcpListener::bind(&addr).await?, app).await?;
    telemetry.shutdown();
    Ok(())
}
