//! Orders service — the async branch. POST an order → a PRODUCER span publishes
//! it to a queue; a background worker consumes it in a CONSUMER span that carries
//! a **span link** back to the producer (the messaging causal edge backends are
//! compared on). An in-process channel stands in for the broker here; the full
//! version uses the compose `broker` (Kafka). Span kinds + link are real OTel.
//!
//! Chaos: POST /order?poison=1 → the consumer fails repeatedly with redelivery
//! (B8); POST /order?lag_ms=<n> → slow consumer to build queue depth (B7).

use axum::{Json, Router, extract::Query, extract::State, routing::post};
use opentelemetry::Context;
use opentelemetry::trace::TraceContextExt;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tracing_opentelemetry::OpenTelemetrySpanExt;

struct Msg {
    order_id: String,
    producer_cx: Context,
    poison: bool,
    lag_ms: u64,
}

#[derive(Clone)]
struct App {
    tx: mpsc::Sender<Msg>,
}

#[derive(Deserialize)]
struct Publish {
    #[serde(default, deserialize_with = "de_flag")]
    poison: bool,
    #[serde(default)]
    lag_ms: u64,
}
fn de_flag<'de, D: serde::Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    let s = String::deserialize(d)?;
    Ok(matches!(s.as_str(), "1" | "true" | "yes" | "on"))
}

#[tracing::instrument(skip(state, p), fields(otel.kind = "producer"))]
async fn publish(State(state): State<App>, Query(p): Query<Publish>) -> Json<Value> {
    let producer_cx = tracing::Span::current().context();
    let order_id = format!("order-{}", std::process::id());
    let _ = state
        .tx
        .send(Msg {
            order_id: order_id.clone(),
            producer_cx,
            poison: p.poison,
            lag_ms: p.lag_ms,
        })
        .await;
    tracing::info!(%order_id, poison = p.poison, "order published");
    Json(json!({ "order_id": order_id, "status": "queued" }))
}

#[tracing::instrument(skip(producer_cx), fields(otel.kind = "consumer"))]
async fn consume(order_id: &str, producer_cx: Context, poison: bool, lag_ms: u64, attempt: u32) {
    // The CONSUMER span links to the PRODUCER span — the async causal edge.
    tracing::Span::current().add_link(producer_cx.span().span_context().clone());
    if lag_ms > 0 {
        tokio::time::sleep(std::time::Duration::from_millis(lag_ms)).await; // B7 consumer lag
    }
    if poison {
        // B8: poison message — fails and is redelivered up to a dead-letter cap.
        tracing::error!(%order_id, attempt, "poison message — consume failed, redelivering");
    } else {
        tracing::info!(%order_id, "order consumed (linked to producer)");
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let telemetry = playground_telemetry::init("orders")?;
    let (tx, mut rx) = mpsc::channel::<Msg>(256);
    tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if msg.poison {
                // Redeliver up to 3 attempts, then dead-letter.
                for attempt in 1..=3 {
                    consume(
                        &msg.order_id,
                        msg.producer_cx.clone(),
                        true,
                        msg.lag_ms,
                        attempt,
                    )
                    .await;
                }
                tracing::error!(order_id = %msg.order_id, "dead-lettered after 3 attempts");
            } else {
                consume(&msg.order_id, msg.producer_cx, false, msg.lag_ms, 1).await;
            }
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
