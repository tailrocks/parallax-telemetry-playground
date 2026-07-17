//! Orders service — the async branch. POST an order → a PRODUCER span publishes
//! it to a queue; a background worker consumes it in a CONSUMER span that carries
//! a **span link** back to the producer (the messaging causal edge backends are
//! compared on). An in-process channel stands in for the broker here; the full
//! version uses the compose `broker` (Kafka). Span kinds + link are real OTel.
//!
//! Chaos: POST /order?poison=1 → the consumer fails repeatedly with redelivery
//! (B8); POST /order?lag_ms=<n> → slow consumer to build queue depth (B7).

use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, header};
use axum::{
    Json, Router,
    extract::Query,
    extract::State,
    routing::{get, post},
};
use opentelemetry::trace::TraceContextExt;
use opentelemetry::{Context, global};
use playground_telemetry::semconv;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tracing::Instrument;
use tracing_opentelemetry::OpenTelemetrySpanExt;

const BATCH_MAX: usize = 10;
const BATCH_WINDOW: Duration = Duration::from_millis(50);

struct Msg {
    order_id: String,
    job_id: String,
    producer_cx: Context,
    poison: bool,
    lag_ms: u64,
    batch: bool,
    orphan: bool,
    enqueued_at: Instant,
}

#[derive(Clone)]
struct App {
    tx: mpsc::Sender<Msg>,
    queue_depth: Arc<AtomicI64>,
}

#[derive(Deserialize)]
struct Publish {
    #[serde(default, deserialize_with = "de_flag")]
    poison: bool,
    #[serde(default, deserialize_with = "de_flag")]
    batch: bool,
    #[serde(default, deserialize_with = "de_flag")]
    orphan: bool,
    #[serde(default)]
    lag_ms: u64,
}
fn de_flag<'de, D: serde::Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    let s = String::deserialize(d)?;
    Ok(matches!(s.as_str(), "1" | "true" | "yes" | "on"))
}

async fn publish(
    headers: HeaderMap,
    State(state): State<App>,
    Query(p): Query<Publish>,
) -> Json<Value> {
    let job_id = uuid::Uuid::new_v4().to_string();
    let span = tracing::info_span!(
        "send orders",
        otel.kind = semconv::SPAN_KIND_PRODUCER,
        "messaging.system" = "inprocess",
        "messaging.destination.name" = "orders",
        "messaging.operation.name" = "send",
        "messaging.operation.type" = "send",
        job.id = %job_id,
        job.type = semconv::JOB_TYPE_ORDER_DISPATCH,
    );
    playground_telemetry::set_parent_from_headers(&span, &headers);
    publish_inner(state, p, job_id).instrument(span).await
}

async fn publish_inner(state: App, p: Publish, job_id: String) -> Json<Value> {
    let poison_message_flag =
        playground_telemetry::feature_flag("poisonMessage", "POISON_MESSAGE").await;
    let poison = p.poison || poison_message_flag;
    let producer_cx = if p.orphan {
        Context::new()
    } else {
        tracing::Span::current().context()
    };
    let order_id = next_order_id();
    tracing::Span::current().set_attribute(semconv::MESSAGING_MESSAGE_ID, order_id.clone());
    let msg = Msg {
        order_id: order_id.clone(),
        job_id,
        producer_cx,
        poison,
        lag_ms: p.lag_ms,
        batch: p.batch,
        orphan: p.orphan,
        enqueued_at: Instant::now(),
    };
    state.queue_depth.fetch_add(1, Ordering::Relaxed);
    if state.tx.send(msg).await.is_err() {
        state.queue_depth.fetch_sub(1, Ordering::Relaxed);
        playground_telemetry::mark_span_error("queue_closed");
        tracing::error!(%order_id, "order queue closed");
        return Json(json!({ "order_id": order_id, "status": "queue_closed" }));
    }
    tracing::info!(%order_id, poison, flagd = poison_message_flag, batch = p.batch, orphan = p.orphan, "order published");
    Json(json!({ "order_id": order_id, "status": "queued" }))
}

async fn consume(msg: Msg, attempt: u32) {
    let delivery_lag_ms = msg.enqueued_at.elapsed().as_millis() as i64;
    let span = tracing::info_span!(
        "process orders",
        otel.kind = semconv::SPAN_KIND_CONSUMER,
        order_id = %msg.order_id,
        attempt,
        "messaging.system" = "inprocess",
        "messaging.destination.name" = "orders",
        "messaging.operation.name" = "process",
        "messaging.operation.type" = "process",
        "messaging.message.id" = %msg.order_id,
        "messaging.delivery.lag_ms" = delivery_lag_ms,
        "messaging.orphan" = msg.orphan,
        job.id = %msg.job_id,
        job.type = semconv::JOB_TYPE_ORDER_DISPATCH,
        outcome = if msg.poison { semconv::OUTCOME_FAILURE } else { semconv::OUTCOME_SUCCESS },
    );
    // The normal CONSUMER span links to the PRODUCER span. Orphan messages
    // deliberately carry no context so evidence-gap detectors have raw data.
    if !msg.orphan {
        span.add_link(msg.producer_cx.span().span_context().clone());
    }
    async move {
        if msg.lag_ms > 0 {
            tokio::time::sleep(Duration::from_millis(msg.lag_ms)).await; // B7 consumer lag
        }
        if msg.poison {
            // B8: poison message — fails and is redelivered up to a dead-letter cap.
            playground_telemetry::mark_span_error("poison_message");
            tracing::error!(order_id = %msg.order_id, attempt, "poison message; consume failed, redelivering");
        } else {
            playground_telemetry::emit_event(
                "order.consumed",
                &[
                    ("order_id", msg.order_id.clone()),
                    ("poison", msg.poison.to_string()),
                ],
            );
            tracing::info!(order_id = %msg.order_id, orphan = msg.orphan, "order consumed");
        }
    }
    .instrument(span)
    .await;
}

async fn consume_batch(batch: Vec<Msg>) {
    let message_count = batch.len();
    let max_delivery_lag_ms = batch
        .iter()
        .map(|msg| msg.enqueued_at.elapsed().as_millis() as i64)
        .max()
        .unwrap_or(0);
    let span = tracing::info_span!(
        "process orders",
        otel.kind = semconv::SPAN_KIND_CONSUMER,
        "messaging.system" = "inprocess",
        "messaging.destination.name" = "orders",
        "messaging.operation.name" = "process",
        "messaging.operation.type" = "process",
        "messaging.batch.message_count" = message_count as i64,
        "messaging.delivery.lag_ms" = max_delivery_lag_ms,
    );
    for msg in &batch {
        if !msg.orphan {
            span.add_link(msg.producer_cx.span().span_context().clone());
        }
    }
    async move {
        let order_ids = batch
            .iter()
            .map(|msg| msg.order_id.as_str())
            .collect::<Vec<_>>()
            .join(",");
        tracing::info!(message_count, %order_ids, "batch consumed");
    }
    .instrument(span)
    .await;
}

fn batch_eligible(msg: &Msg) -> bool {
    msg.batch && !msg.poison
}

async fn consume_single(msg: Msg) {
    if msg.poison {
        // Redeliver up to 3 attempts, then dead-letter.
        let order_id = msg.order_id.clone();
        let job_id = msg.job_id.clone();
        let producer_cx = msg.producer_cx.clone();
        let lag_ms = msg.lag_ms;
        let orphan = msg.orphan;
        let enqueued_at = msg.enqueued_at;
        for attempt in 1..=3 {
            consume(
                Msg {
                    order_id: order_id.clone(),
                    job_id: job_id.clone(),
                    producer_cx: producer_cx.clone(),
                    poison: true,
                    lag_ms,
                    batch: false,
                    orphan,
                    enqueued_at,
                },
                attempt,
            )
            .await;
        }
        let span = tracing::error_span!(
            "process orders",
            otel.kind = semconv::SPAN_KIND_CONSUMER,
            "messaging.system" = "inprocess",
            "messaging.destination.name" = "orders",
            "messaging.operation.name" = "process",
            "messaging.operation.type" = "process",
            "messaging.message.id" = %order_id,
            job.id = %job_id,
            job.type = semconv::JOB_TYPE_ORDER_DISPATCH,
            outcome = semconv::OUTCOME_FAILURE,
        );
        let _guard = span.enter();
        playground_telemetry::mark_span_error("dead_letter");
        tracing::error!(order_id = %order_id, "dead-lettered after 3 attempts");
    } else {
        consume(msg, 1).await;
    }
}

async fn drain_batch(
    first: Msg,
    rx: &mut mpsc::Receiver<Msg>,
    queue_depth: &AtomicI64,
) -> Vec<Msg> {
    let mut batch = vec![first];
    let deadline = tokio::time::sleep(BATCH_WINDOW);
    tokio::pin!(deadline);
    while batch.len() < BATCH_MAX {
        tokio::select! {
            maybe_msg = rx.recv() => {
                let Some(msg) = maybe_msg else {
                    break;
                };
                queue_depth.fetch_sub(1, Ordering::Relaxed);
                if batch_eligible(&msg) {
                    batch.push(msg);
                } else {
                    consume_single(msg).await;
                }
            }
            _ = &mut deadline => break,
        }
    }
    batch
}

fn spawn_queue_depth_metrics(queue_depth: Arc<AtomicI64>) {
    tokio::spawn(async move {
        // Custom lab metric: this in-process queue has no stable OTel lag
        // semantic convention, so emit a plain gauge for dashboards.
        let gauge = global::meter("playground.messaging")
            .i64_gauge("messaging.queue.depth")
            .with_description("Orders messages waiting in the in-process async queue")
            .build();
        let mut ticker = tokio::time::interval(Duration::from_secs(5));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            gauge.record(queue_depth.load(Ordering::Relaxed), &[]);
        }
    });
}

fn next_order_id() -> String {
    static SEQ: AtomicU64 = AtomicU64::new(1);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("order-{}-{seq}", std::process::id())
}

fn cors_layer() -> CorsLayer {
    let origin = std::env::var("WEB_ORIGIN")
        .ok()
        .and_then(|origin| origin.parse::<HeaderValue>().ok())
        .map(AllowOrigin::exact)
        .unwrap_or_else(AllowOrigin::mirror_request);
    CorsLayer::new()
        .allow_origin(origin)
        .allow_methods([Method::POST])
        .allow_headers([
            header::CONTENT_TYPE,
            HeaderName::from_static("traceparent"),
            HeaderName::from_static("tracestate"),
            HeaderName::from_static("baggage"),
        ])
}

fn app(state: App) -> Router {
    Router::new()
        .route("/order", post(publish))
        .route("/healthz", get(|| async { "ok" }))
        .with_state(state)
        .layer(cors_layer())
        .layer(axum::middleware::from_fn(
            playground_telemetry::http_server_observability,
        ))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let telemetry = playground_telemetry::init("orders")?;
    let (tx, mut rx) = mpsc::channel::<Msg>(256);
    let queue_depth = Arc::new(AtomicI64::new(0));
    spawn_queue_depth_metrics(queue_depth.clone());
    let consumer_depth = queue_depth.clone();
    tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            consumer_depth.fetch_sub(1, Ordering::Relaxed);
            if batch_eligible(&msg) {
                let batch = drain_batch(msg, &mut rx, &consumer_depth).await;
                consume_batch(batch).await;
                continue;
            }
            consume_single(msg).await;
        }
    });
    let app = app(App { tx, queue_depth });
    let addr = std::env::var("ADDR").unwrap_or_else(|_| "0.0.0.0:8092".into());
    tracing::info!(%addr, "orders HTTP listening");
    axum::serve(tokio::net::TcpListener::bind(&addr).await?, app)
        .with_graceful_shutdown(playground_telemetry::shutdown_signal())
        .await?;
    telemetry.shutdown();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;

    fn test_msg(batch: bool) -> Msg {
        test_msg_with_poison(batch, false)
    }

    fn test_msg_with_poison(batch: bool, poison: bool) -> Msg {
        Msg {
            order_id: next_order_id(),
            job_id: uuid::Uuid::new_v4().to_string(),
            producer_cx: Context::new(),
            poison,
            lag_ms: 0,
            batch,
            orphan: false,
            enqueued_at: Instant::now(),
        }
    }

    #[tokio::test]
    async fn drain_batch_caps_at_ten_and_decrements_depth() {
        let (tx, mut rx) = mpsc::channel::<Msg>(16);
        let depth = AtomicI64::new(12);
        for _ in 0..12 {
            tx.send(test_msg(true)).await.unwrap();
        }
        let first = rx.recv().await.unwrap();
        depth.fetch_sub(1, Ordering::Relaxed);
        let batch = drain_batch(first, &mut rx, &depth).await;
        assert_eq!(batch.len(), BATCH_MAX);
        assert_eq!(depth.load(Ordering::Relaxed), 2);
    }

    #[tokio::test]
    async fn drain_batch_waits_for_window_when_queue_is_short() {
        let (_tx, mut rx) = mpsc::channel::<Msg>(16);
        let depth = AtomicI64::new(0);
        let started = Instant::now();
        let batch = drain_batch(test_msg(true), &mut rx, &depth).await;
        assert_eq!(batch.len(), 1);
        assert!(started.elapsed() >= BATCH_WINDOW);
    }

    #[tokio::test]
    async fn batch_flag_does_not_hide_poison_messages() {
        assert!(batch_eligible(&test_msg_with_poison(true, false)));
        assert!(!batch_eligible(&test_msg_with_poison(true, true)));

        let (tx, mut rx) = mpsc::channel::<Msg>(16);
        let depth = AtomicI64::new(1);
        tx.send(test_msg_with_poison(true, true)).await.unwrap();
        drop(tx);

        let batch = drain_batch(test_msg(true), &mut rx, &depth).await;

        assert_eq!(batch.len(), 1);
        assert_eq!(depth.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn published_message_carries_the_producer_job_id() {
        let (tx, mut rx) = mpsc::channel::<Msg>(1);
        let state = App {
            tx,
            queue_depth: Arc::new(AtomicI64::new(0)),
        };
        let job_id = "job-fixed".to_string();
        let _ = publish_inner(
            state,
            Publish {
                poison: false,
                batch: false,
                orphan: false,
                lag_ms: 0,
            },
            job_id.clone(),
        )
        .await;
        let msg = rx.recv().await.expect("published message");
        assert_eq!(msg.job_id, job_id, "consumer attempt shares the job id");
    }

    #[tokio::test]
    async fn serves_health_and_enqueues_orders_over_http() {
        let (tx, mut rx) = mpsc::channel::<Msg>(1);
        let state = App {
            tx,
            queue_depth: Arc::new(AtomicI64::new(0)),
        };

        let health = app(state.clone())
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .expect("health request"),
            )
            .await
            .expect("health response");
        assert_eq!(health.status(), StatusCode::OK);

        let response = app(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/order")
                    .body(Body::empty())
                    .expect("order request"),
            )
            .await
            .expect("order response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body");
        assert!(
            std::str::from_utf8(&body)
                .expect("UTF-8 body")
                .contains("queued")
        );

        let queued = rx.recv().await.expect("queued message");
        assert!(queued.order_id.starts_with("order-"));
    }

    #[tokio::test]
    async fn serves_health_over_a_real_loopback_listener() {
        let (tx, _rx) = mpsc::channel::<Msg>(1);
        let state = App {
            tx,
            queue_depth: Arc::new(AtomicI64::new(0)),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind orders listener");
        let address = listener.local_addr().expect("orders listener address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app(state))
                .await
                .expect("serve orders");
        });

        let response = tokio::time::timeout(
            Duration::from_secs(3),
            reqwest::get(format!("http://{address}/healthz")),
        )
        .await
        .expect("orders health timeout")
        .expect("orders health response");
        assert_eq!(response.status(), StatusCode::OK);
        server.abort();
    }
}
