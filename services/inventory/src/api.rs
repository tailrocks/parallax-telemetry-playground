use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::{
    Json, Router,
    routing::{get, post},
};
use deadpool_postgres::Pool;
use playground_telemetry::semconv;
use serde_json::{Value, json};
use tracing::Instrument;

use crate::application::{self, ApplicationError};
use crate::domain::{ConsumeBody, ReleaseBody, ReservationConflict, ReserveBody, ReserveQuery};
use crate::infrastructure;

#[derive(Clone)]
struct AppState {
    pool: Pool,
}

type InventoryResponse = (StatusCode, Json<Value>);

pub fn router(pool: Pool) -> Router {
    Router::new()
        .route("/reserve", get(reserve_get).post(reserve_post))
        .route("/release", post(release_post))
        .route("/consume", post(consume_post))
        .route("/healthz", get(health))
        .with_state(AppState { pool })
        .layer(axum::middleware::from_fn(
            playground_telemetry::http_server_observability,
        ))
}

async fn reserve_get(
    headers: HeaderMap,
    State(state): State<AppState>,
    Query(params): Query<ReserveQuery>,
) -> impl IntoResponse {
    let mut params = params;
    let parent = playground_telemetry::extract_context(&headers);
    let tenant_id = match playground_telemetry::resolve_http_tenant_identity(
        &headers,
        (!params.tenant_id.trim().is_empty()).then_some(params.tenant_id.as_str()),
    ) {
        Ok(tenant_id) => tenant_id,
        Err(error) => return tenant_identity_error(error),
    };
    params.tenant_id = tenant_id;
    let span = tracing::info_span!("inventory.reserve", otel.kind = semconv::SPAN_KIND_SERVER);
    playground_telemetry::set_parent_from_headers(&span, &headers);
    playground_telemetry::stamp_business_baggage(&span, &parent);
    async move { reserve_inner(state, params).await }
        .instrument(span)
        .await
}

async fn reserve_post(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(body): Json<ReserveBody>,
) -> impl IntoResponse {
    let params = ReserveQuery {
        tenant_id: body.tenant_id,
        reservation_id: body.reservation_id,
        sku: body.sku,
        quantity: body.quantity,
        slow: 0,
        db_n1: 0,
        hold_ms: 0,
        fail: false,
        checkout_request_id: body.checkout_request_id,
        checkout_lease_token: body.checkout_lease_token,
    };
    reserve_get(headers, State(state), Query(params)).await
}

async fn consume_post(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(body): Json<ConsumeBody>,
) -> impl IntoResponse {
    let mut body = body;
    let parent = playground_telemetry::extract_context(&headers);
    let tenant_id = match playground_telemetry::resolve_http_tenant_identity(
        &headers,
        (!body.tenant_id.trim().is_empty()).then_some(body.tenant_id.as_str()),
    ) {
        Ok(tenant_id) => tenant_id,
        Err(error) => return tenant_identity_error(error),
    };
    body.tenant_id = tenant_id;
    let span = tracing::info_span!("inventory.consume", otel.kind = semconv::SPAN_KIND_SERVER);
    playground_telemetry::set_parent_from_headers(&span, &headers);
    playground_telemetry::stamp_business_baggage(&span, &parent);
    async move { consume_inner(state, body).await }
        .instrument(span)
        .await
}

async fn consume_inner(state: AppState, body: ConsumeBody) -> InventoryResponse {
    let result = application::consume(&state.pool, &body).await;
    match result {
        Err(ApplicationError::Invalid(error)) => (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "invalid_consume_request",
                "field": error.field,
                "message": error.reason,
            })),
        ),
        Ok(outcome) => {
            tracing::info!(
                tenant_id = %body.tenant_id,
                reservation_count = body.reservations.len(),
                consumed = outcome.consumed,
                status = outcome.status,
                "inventory reservations consumed"
            );
            (
                StatusCode::OK,
                Json(json!({
                    "tenant_id": body.tenant_id,
                    "consumed": outcome.consumed,
                    "reservation_count": body.reservations.len(),
                    "status": outcome.status,
                })),
            )
        }
        Err(ApplicationError::Operation(error))
            if error.downcast_ref::<ReservationConflict>().is_some() =>
        {
            let conflict = error
                .downcast_ref::<ReservationConflict>()
                .expect("checked reservation conflict");
            playground_telemetry::mark_span_error(conflict.code);
            let status = if conflict.code == "reservation_not_found" {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::CONFLICT
            };
            (
                status,
                Json(json!({
                    "error": conflict.code,
                    "tenant_id": body.tenant_id,
                    "message": conflict.message,
                })),
            )
        }
        Err(ApplicationError::Operation(error)) => {
            playground_telemetry::mark_span_error("postgres_error");
            tracing::error!(
                tenant_id = %body.tenant_id,
                error = %error,
                "inventory consume transaction failed"
            );
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({
                    "error": "inventory_unavailable",
                    "tenant_id": body.tenant_id,
                })),
            )
        }
    }
}

async fn release_post(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(body): Json<ReleaseBody>,
) -> impl IntoResponse {
    let mut body = body;
    let parent = playground_telemetry::extract_context(&headers);
    let tenant_id = match playground_telemetry::resolve_http_tenant_identity(
        &headers,
        (!body.tenant_id.trim().is_empty()).then_some(body.tenant_id.as_str()),
    ) {
        Ok(tenant_id) => tenant_id,
        Err(error) => return tenant_identity_error(error),
    };
    body.tenant_id = tenant_id;
    let span = tracing::info_span!("inventory.release", otel.kind = semconv::SPAN_KIND_SERVER);
    playground_telemetry::set_parent_from_headers(&span, &headers);
    playground_telemetry::stamp_business_baggage(&span, &parent);
    async move { release_inner(state, body).await }
        .instrument(span)
        .await
}

async fn release_inner(state: AppState, body: ReleaseBody) -> InventoryResponse {
    let result = application::release(&state.pool, &body).await;

    match result {
        Err(ApplicationError::Invalid(error)) => (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "invalid_release_request",
                "field": error.field,
                "message": error.reason,
            })),
        ),
        Ok(Some(outcome)) => {
            tracing::info!(
                reservation_id = %body.reservation_id,
                sku = %body.sku,
                quantity = body.quantity,
                released = outcome.released,
                location_id = %outcome.location_id,
                "inventory reservation released"
            );
            (
                StatusCode::OK,
                Json(json!({
                    "tenant_id": body.tenant_id,
                    "reservation_id": body.reservation_id,
                    "sku": body.sku,
                    "location_id": outcome.location_id,
                    "requested": body.quantity,
                    "released": outcome.released,
                    "reserved_remaining": outcome.reserved_remaining,
                    "available": outcome.available,
                    "status": outcome.status,
                })),
            )
        }
        Ok(None) => {
            playground_telemetry::mark_span_error("reservation_not_found");
            tracing::warn!(
                tenant_id = %body.tenant_id,
                reservation_id = %body.reservation_id,
                sku = %body.sku,
                "inventory reservation was not found"
            );
            (
                StatusCode::NOT_FOUND,
                Json(json!({
                    "error": "reservation_not_found",
                    "tenant_id": body.tenant_id,
                    "reservation_id": body.reservation_id,
                    "sku": body.sku,
                    "message": "reservation does not exist for the tenant",
                })),
            )
        }
        Err(ApplicationError::Operation(error))
            if error.downcast_ref::<ReservationConflict>().is_some() =>
        {
            let conflict = error
                .downcast_ref::<ReservationConflict>()
                .expect("checked reservation conflict");
            playground_telemetry::mark_span_error(conflict.code);
            (
                StatusCode::CONFLICT,
                Json(json!({
                    "error": conflict.code,
                    "tenant_id": body.tenant_id,
                    "reservation_id": body.reservation_id,
                    "sku": body.sku,
                    "message": conflict.message,
                })),
            )
        }
        Err(ApplicationError::Operation(error)) => {
            playground_telemetry::mark_span_error("postgres_error");
            tracing::error!(
                tenant_id = %body.tenant_id,
                reservation_id = %body.reservation_id,
                sku = %body.sku,
                error = %error,
                "inventory release transaction failed"
            );
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({
                    "error": "inventory_unavailable",
                    "tenant_id": body.tenant_id,
                    "reservation_id": body.reservation_id,
                    "sku": body.sku,
                })),
            )
        }
    }
}

async fn reserve_inner(state: AppState, params: ReserveQuery) -> InventoryResponse {
    let result = application::reserve(&state.pool, &params).await;

    match result {
        Err(ApplicationError::Invalid(error)) => (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "invalid_reserve_request",
                "field": error.field,
                "message": error.reason,
            })),
        ),
        Ok(Some(outcome)) => {
            tracing::info!(
                reservation_id = %params.reservation_id,
                sku = %params.sku,
                quantity = params.quantity,
                location_id = %outcome.location_id,
                remaining = outcome.remaining,
                "inventory reserved"
            );
            (
                StatusCode::OK,
                Json(json!({
                    "tenant_id": params.tenant_id,
                    "reservation_id": params.reservation_id,
                    "sku": params.sku,
                    "reserved": params.quantity,
                    "location_id": outcome.location_id,
                    "remaining": outcome.remaining,
                    "status": outcome.status
                })),
            )
        }
        Ok(None) => {
            playground_telemetry::mark_span_error("out_of_stock");
            tracing::warn!(reservation_id = %params.reservation_id, sku = %params.sku, quantity = params.quantity, "inventory unavailable");
            (
                StatusCode::CONFLICT,
                Json(
                    json!({"error": "out_of_stock", "tenant_id": params.tenant_id, "reservation_id": params.reservation_id, "sku": params.sku}),
                ),
            )
        }
        Err(ApplicationError::Operation(error))
            if error.downcast_ref::<ReservationConflict>().is_some() =>
        {
            let conflict = error
                .downcast_ref::<ReservationConflict>()
                .expect("checked reservation conflict");
            playground_telemetry::mark_span_error(conflict.code);
            (
                StatusCode::CONFLICT,
                Json(json!({
                    "error": conflict.code,
                    "tenant_id": params.tenant_id,
                    "reservation_id": params.reservation_id,
                    "sku": params.sku,
                    "message": conflict.message,
                })),
            )
        }
        Err(ApplicationError::Operation(error))
            if error.to_string().contains("fault injection") =>
        {
            playground_telemetry::mark_span_error("reservation_rejected");
            tracing::warn!(reservation_id = %params.reservation_id, sku = %params.sku, "inventory fault scenario");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(
                    json!({"error": "reservation_rejected", "tenant_id": params.tenant_id, "reservation_id": params.reservation_id, "sku": params.sku}),
                ),
            )
        }
        Err(ApplicationError::Operation(error)) => {
            playground_telemetry::mark_span_error("postgres_error");
            tracing::error!(reservation_id = %params.reservation_id, sku = %params.sku, error = %error, "inventory database operation failed");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(
                    json!({"error": "inventory_unavailable", "tenant_id": params.tenant_id, "reservation_id": params.reservation_id, "sku": params.sku}),
                ),
            )
        }
    }
}

async fn health(State(state): State<AppState>) -> impl IntoResponse {
    match infrastructure::health_check(&state.pool).await {
        Ok(()) => (
            StatusCode::OK,
            Json(json!({"status": "UP", "database": "UP"})),
        ),
        Err(error) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"status": "DOWN", "error": error})),
        ),
    }
}

fn tenant_identity_error(error: playground_telemetry::TenantIdentityError) -> InventoryResponse {
    let (status, code, message) = match error {
        playground_telemetry::TenantIdentityError::Missing => (
            StatusCode::BAD_REQUEST,
            "tenant_required",
            "tenant_id is required in the request or tenant.id baggage",
        ),
        playground_telemetry::TenantIdentityError::Invalid => (
            StatusCode::BAD_REQUEST,
            "invalid_identity",
            "tenant identity is invalid",
        ),
        playground_telemetry::TenantIdentityError::Conflicting => (
            StatusCode::CONFLICT,
            "identity_conflict",
            "tenant identity sources conflict",
        ),
    };
    playground_telemetry::mark_span_error(code);
    (
        status,
        Json(json!({"error": code, "field": "tenant_id", "message": message})),
    )
}
