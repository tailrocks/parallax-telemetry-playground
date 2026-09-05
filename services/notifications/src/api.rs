use crate::domain::NotificationRequest;
use crate::store::{AppState, StoreError, record_delivery};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::{
    Json, Router,
    routing::{get, post},
};
use playground_telemetry::semconv;
use serde_json::{Value, json};
use tracing::Instrument;

async fn health(State(state): State<AppState>) -> impl IntoResponse {
    match tokio::time::timeout(crate::store::DB_TIMEOUT, state.pool.get()).await {
        Ok(Ok(client)) => match client.query_one("SELECT 1", &[]).await {
            Ok(_)
                if state
                    .worker_ready
                    .load(std::sync::atomic::Ordering::Acquire) =>
            {
                (
                    StatusCode::OK,
                    Json(json!({
                        "status":"UP",
                        "database":"UP",
                        "worker":"ready"
                    })),
                )
            }
            Ok(_) => (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({
                    "status":"DOWN",
                    "database":"UP",
                    "worker":"starting"
                })),
            ),
            Err(error) => (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"status":"DOWN", "error":error.to_string()})),
            ),
        },
        Ok(Err(error)) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"status":"DOWN", "error":error.to_string()})),
        ),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"status":"DOWN", "error":"database pool timeout"})),
        ),
    }
}

async fn notify(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(request): Json<NotificationRequest>,
) -> impl IntoResponse {
    let span = tracing::info_span!(
        "notifications.deliver",
        otel.kind = semconv::SPAN_KIND_SERVER
    );
    let parent = playground_telemetry::extract_context(&headers);
    playground_telemetry::set_parent_from_headers(&span, &headers);
    playground_telemetry::stamp_business_baggage(&span, &parent);
    async move { notify_inner(state, request, headers).await }
        .instrument(span)
        .await
}

async fn notify_inner(
    state: AppState,
    mut request: NotificationRequest,
    headers: HeaderMap,
) -> (StatusCode, Json<Value>) {
    let tenant_id = match playground_telemetry::resolve_http_tenant_identity(
        &headers,
        request.tenant_id.as_deref(),
    ) {
        Ok(tenant_id) => tenant_id,
        Err(error) => return tenant_identity_error(error),
    };
    request.tenant_id = Some(tenant_id);
    if request.order_id.trim().is_empty()
        || !matches!(request.channel.as_str(), "email" | "webhook" | "in_app")
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":"invalid_notification"})),
        );
    }
    let event_key = if request.event_key.trim().is_empty() {
        format!("notification:{}:{}", request.order_id, request.channel)
    } else {
        request.event_key.clone()
    };
    let payload = if request.payload.is_object() {
        request.payload.clone()
    } else {
        json!({"value": request.payload})
    };
    let record = match record_delivery(&state, &request, &event_key, &payload, &headers).await {
        Ok(record) => record,
        Err(StoreError::Timeout) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error":"database_timeout"})),
            );
        }
        Err(StoreError::Invalid(error)) => {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": error})));
        }
        Err(StoreError::Conflict) => {
            return (
                StatusCode::CONFLICT,
                Json(json!({"error":"notification_identity_conflict"})),
            );
        }
        Err(StoreError::Database(error)) => return db_error(error),
    };
    tracing::info!(
        delivery_id = %record.id,
        %event_key,
        attempts = record.attempts,
        "notification delivery queued"
    );
    (
        StatusCode::ACCEPTED,
        Json(json!({
            "delivery_id": record.id,
            "event_key": event_key,
            "status": record.status,
            "attempts": record.attempts,
            "channel": record.channel,
            "provider": record.provider
        })),
    )
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

fn db_error(error: impl std::fmt::Display) -> (StatusCode, Json<Value>) {
    tracing::error!(error = %error, "notification persistence failed");
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error":"notification_store_unavailable"})),
    )
}

pub(crate) fn app_with_state(state: AppState) -> Router {
    Router::new()
        .route("/notify", post(notify))
        .route("/healthz", get(health))
        .with_state(state)
        .layer(axum::middleware::from_fn(
            playground_telemetry::http_server_observability,
        ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::NotificationRequest;
    use crate::store::{AppState, postgres_pool};

    fn disconnected_state() -> AppState {
        AppState::new(
            postgres_pool("postgres://postgres:playground@127.0.0.1:1/playground")
                .expect("build postgres pool"),
        )
        .expect("build notification state")
    }

    fn request(tenant_id: Option<&str>) -> NotificationRequest {
        NotificationRequest {
            tenant_id: tenant_id.map(str::to_owned),
            event_key: "order-1:notification".to_owned(),
            order_id: "order-1".to_owned(),
            channel: "in_app".to_owned(),
            payload: json!({"status":"confirmed"}),
        }
    }

    #[tokio::test]
    async fn notify_rejects_missing_tenant_before_database_access() {
        let (status, _) = notify_inner(disconnected_state(), request(None), HeaderMap::new()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn notify_rejects_conflicting_propagated_tenant() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "baggage",
            "tenant.id=tenant-nova".parse().expect("baggage header"),
        );
        let (status, _) =
            notify_inner(disconnected_state(), request(Some("tenant-acme")), headers).await;
        assert_eq!(status, StatusCode::CONFLICT);
    }
}
