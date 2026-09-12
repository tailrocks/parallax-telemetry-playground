use crate::messaging::{
    App, OrderMessage, ROUTING_KEY, SYNTHETIC_EXCHANGE, inject_context, publish_confirm,
};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header};
use axum::{
    Json, Router,
    extract::{Query, State},
    routing::{get, post},
};
use opentelemetry::{Context, KeyValue, global};
use playground_telemetry::semconv;
use serde::Deserialize;
use serde_json::{Value, json};
use tower_http::cors::{AllowOrigin, CorsLayer};
use tracing::Instrument;
use tracing_opentelemetry::OpenTelemetrySpanExt;

#[derive(Debug, Deserialize)]
struct PublishQuery {
    #[serde(default, deserialize_with = "de_flag")]
    poison: bool,
    #[serde(default, deserialize_with = "de_flag")]
    orphan: bool,
    #[serde(default)]
    lag_ms: u64,
    tenant_id: Option<String>,
    #[serde(default = "default_customer")]
    customer_id: String,
}

pub(crate) fn default_customer() -> String {
    "customer-acme-ava".to_owned()
}

fn de_flag<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    let value = String::deserialize(deserializer)?;
    Ok(matches!(value.as_str(), "1" | "true" | "yes" | "on"))
}

async fn publish(
    headers: HeaderMap,
    State(state): State<App>,
    Query(query): Query<PublishQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let parent = playground_telemetry::extract_context(&headers);
    let tenant_id =
        playground_telemetry::resolve_http_tenant_identity(&headers, query.tenant_id.as_deref())
            .map_err(tenant_identity_error)?;
    let message_id = uuid::Uuid::new_v4().to_string();
    let span = tracing::info_span!(
        "orders.publish",
        otel.kind = semconv::SPAN_KIND_PRODUCER,
        "messaging.system" = "rabbitmq",
        "messaging.destination.name" = SYNTHETIC_EXCHANGE,
        "messaging.operation.name" = "send",
        "messaging.message.id" = %message_id,
        tenant.id = %tenant_id,
        job.id = %message_id,
        job.type = semconv::JOB_TYPE_ORDER_DISPATCH,
        messaging.orphan = tracing::field::Empty,
    );
    span.record("messaging.orphan", query.orphan);
    if query.orphan {
        // Keep the orphan fixture on a valid, detached trace so RabbitMQ's
        // required carrier contract still holds and the consumer can prove
        // that it received a root rather than an invalid context.
        let _ = span.set_parent(Context::new());
    } else {
        playground_telemetry::set_parent_from_headers(&span, &headers);
    }
    playground_telemetry::stamp_business_baggage(&span, &parent);
    async move {
        let context = if query.orphan {
            playground_telemetry::with_business_context(
                &tracing::Span::current().context(),
                &tenant_id,
                "standard",
                "standard",
                "us-east-1",
                "normal",
            )
        } else {
            playground_telemetry::with_business_context_from_parent(
                &tracing::Span::current().context(),
                &parent,
                &tenant_id,
                "standard",
                "standard",
                "us-east-1",
                "normal",
            )
        };
        let message = OrderMessage {
            event_id: message_id.clone(),
            order_id: message_id.clone(),
            tenant_id,
            customer_id: query.customer_id,
            event_type: "order.requested".to_owned(),
            poison: query.poison
                || playground_telemetry::feature_flag("poisonMessage", "POISON_MESSAGE").await,
            lag_ms: query.lag_ms.min(30_000),
            attempt: 1,
        };
        let payload = serde_json::to_vec(&message).map_err(internal_error)?;
        let mut carrier = lapin::types::FieldTable::default();
        inject_context(&context, &mut carrier);
        carrier.insert(
            "x-order-attempt".into(),
            lapin::types::AMQPValue::LongUInt(1),
        );
        if query.orphan {
            carrier.insert(
                "messaging.orphan".into(),
                lapin::types::AMQPValue::LongString("true".into()),
            );
        }
        let channel = state.channel.read().await.clone();
        publish_confirm(
            &channel,
            ROUTING_KEY,
            &payload,
            carrier,
            &message.order_id,
            SYNTHETIC_EXCHANGE,
        )
        .await
        .map_err(internal_error)?;
        global::meter("playground.messaging")
            .u64_counter("messaging.published")
            .build()
            .add(
                1,
                &[
                    KeyValue::new("messaging.system", "rabbitmq"),
                    KeyValue::new("messaging.destination.name", SYNTHETIC_EXCHANGE),
                ],
            );
        tracing::info!(order_id = %message.order_id, poison = message.poison, "order event publisher confirmed");
        Ok(Json(json!({"order_id": message.order_id, "status":"published", "messaging_system":"rabbitmq"})))
    }
    .instrument(span)
    .await
}

fn tenant_identity_error(
    error: playground_telemetry::TenantIdentityError,
) -> (StatusCode, Json<Value>) {
    let (status, code) = match error {
        playground_telemetry::TenantIdentityError::Missing => {
            (StatusCode::BAD_REQUEST, "tenant_required")
        }
        playground_telemetry::TenantIdentityError::Invalid => {
            (StatusCode::BAD_REQUEST, "invalid_tenant")
        }
        playground_telemetry::TenantIdentityError::Conflicting => {
            (StatusCode::CONFLICT, "identity_conflict")
        }
    };
    (
        status,
        Json(json!({"error": code, "message": error.to_string()})),
    )
}

fn internal_error(error: impl std::fmt::Display) -> (StatusCode, Json<Value>) {
    tracing::error!(error = %error, "RabbitMQ order operation failed");
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error":"messaging_unavailable"})),
    )
}

fn cors_layer() -> CorsLayer {
    let origin = std::env::var("WEB_ORIGIN")
        .ok()
        .and_then(|value| value.parse::<HeaderValue>().ok())
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

async fn health(State(state): State<App>) -> impl axum::response::IntoResponse {
    let rabbitmq = state.channel.read().await.status().connected();
    let consumer = state
        .consumer_ready
        .load(std::sync::atomic::Ordering::Acquire);
    let durable_inbox = state.inbox.health().await;
    match rabbitmq && consumer && durable_inbox {
        true => (
            StatusCode::OK,
            Json(json!({
                "status":"UP",
                "messaging":"rabbitmq",
                "consumer":"ready",
                "durable_inbox":"ready"
            })),
        ),
        false => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "status":"DOWN",
                "messaging": if rabbitmq { "rabbitmq" } else { "unavailable" },
                "consumer": if consumer { "ready" } else { "stopped" },
                "durable_inbox": if durable_inbox { "ready" } else { "unavailable" }
            })),
        ),
    }
}

pub(crate) fn app(state: App) -> Router {
    Router::new()
        .route("/order", post(publish))
        .route("/healthz", get(health))
        .with_state(state)
        .layer(cors_layer())
        .layer(axum::middleware::from_fn(
            playground_telemetry::http_server_observability,
        ))
}
