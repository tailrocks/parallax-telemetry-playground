//! HTTP transport and request/response adapters.

use crate::application::*;
use crate::domain::*;
use crate::infrastructure::*;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header};
use axum::response::IntoResponse;
use axum::{
    Json, Router,
    routing::{get, post, put},
};
use playground_proto::pricing::v1::{QuoteItem, QuoteRequest, pricing_client::PricingClient};
use playground_telemetry::semconv;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::future::Future;
use std::time::Duration;
use tokio_stream::StreamExt;
use tonic::{Request, transport::Channel};
use tonic_health::pb::{
    HealthCheckRequest, health_check_response::ServingStatus, health_client::HealthClient,
};
use tower_http::cors::{AllowOrigin, CorsLayer};
use tracing::Instrument;
use tracing_opentelemetry::OpenTelemetrySpanExt;

const PRICING_GRPC_HEALTH_SERVICE: &str = "playground.pricing.v1.Pricing";
const PAYMENT_GRPC_HEALTH_SERVICE: &str = "playground.payment.v1.Payment";

pub(crate) async fn checkout_post(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(input): Json<CheckoutInput>,
) -> ApiResult<Json<Value>> {
    checkout_request(headers, state, input).await
}

pub(crate) async fn checkout_request(
    headers: HeaderMap,
    state: AppState,
    input: CheckoutInput,
) -> ApiResult<Json<Value>> {
    let parent = playground_telemetry::extract_context(&headers);
    let input = normalize_checkout_input(&headers, &parent, input, "checkout")?;
    let span = tracing::info_span!(
        "checkout",
        otel.kind = semconv::SPAN_KIND_SERVER,
        "http.route" = "/checkout",
        "commerce.tenant_id" = %input.tenant_id,
    );
    playground_telemetry::set_parent_from_headers(&span, &headers);
    async move {
        let context = playground_telemetry::with_business_context_from_parent(
            &tracing::Span::current().context(),
            &parent,
            &input.tenant_id,
            &input.tier,
            &input.segment,
            &input.region,
            &input.priority,
        );
        let context = context_with_session_id(&context, input.session_id.as_deref());
        playground_telemetry::stamp_business_baggage(&tracing::Span::current(), &context);
        checkout_inner(state, input, context).await
    }
    .instrument(span)
    .await
}

pub(crate) async fn quote_stream(
    headers: HeaderMap,
    State(state): State<AppState>,
    Query(query): Query<QuoteStreamQuery>,
) -> ApiResult<Json<Value>> {
    let parent = playground_telemetry::extract_context(&headers);
    let input = normalize_checkout_input(&headers, &parent, query.into(), "stream")?;
    let span = tracing::info_span!(
        "checkout.quote_stream",
        otel.kind = semconv::SPAN_KIND_SERVER
    );
    playground_telemetry::set_parent_from_headers(&span, &headers);
    async move {
        let context = playground_telemetry::with_business_context_from_parent(
            &tracing::Span::current().context(),
            &parent,
            &input.tenant_id,
            &input.tier,
            &input.segment,
            &input.region,
            &input.priority,
        );
        let context = context_with_session_id(&context, input.session_id.as_deref());
        playground_telemetry::stamp_business_baggage(&tracing::Span::current(), &context);
        let mut client = PricingClient::new(pricing_channel(&state).await?);
        let request = QuoteRequest {
            request_id: input
                .request_id
                .unwrap_or_else(|| format!("stream-{}", uuid::Uuid::new_v4())),
            tenant_id: input.tenant_id,
            customer_id: input.customer_id,
            items: input
                .items
                .iter()
                .map(|item| QuoteItem {
                    sku: item.sku.clone(),
                    quantity: item.quantity,
                })
                .collect(),
            currency_code: input.currency_code,
            context: HashMap::from([
                ("customer_segment".to_owned(), input.segment.clone()),
                ("customer_tier".to_owned(), input.tier.clone()),
                ("region".to_owned(), input.region.clone()),
                ("request_priority".to_owned(), input.priority.clone()),
                ("pricing_strategy".to_owned(), "standard".to_owned()),
            ]),
            payment_method_type: None,
        };
        let mut request = tonic::Request::new(request);
        request.set_timeout(Duration::from_millis(input.timeout_ms.clamp(1, 30_000)));
        playground_telemetry::inject_grpc_metadata_with_context(&context, request.metadata_mut());
        let mut stream = client
            .quote_stream(request)
            .await
            .map_err(pricing_error)?
            .into_inner();
        let mut count = 0_u32;
        let mut cancelled = false;
        let cancel_after = input.delay_ms;
        let started = std::time::Instant::now();
        while let Some(result) = stream.next().await {
            match result {
                Ok(_) => {
                    count += 1;
                    if cancel_after > 0 && started.elapsed() >= Duration::from_millis(cancel_after)
                    {
                        cancelled = true;
                        break;
                    }
                }
                Err(status) => return Err(pricing_error(status)),
            }
        }
        Ok(Json(
            json!({"streamed_quotes": count, "cancelled": cancelled}),
        ))
    }
    .instrument(span)
    .await
}

fn normalize_checkout_input(
    headers: &HeaderMap,
    parent: &opentelemetry::Context,
    mut input: CheckoutInput,
    request_prefix: &str,
) -> ApiResult<CheckoutInput> {
    let request_id = input
        .request_id
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("{request_prefix}-{}", uuid::Uuid::new_v4()));
    let tenant_id = resolve_http_tenant_id(headers, &input.tenant_id)?;
    let session_id = resolve_session_id(parent, input.session_id.as_deref())?
        .unwrap_or_else(|| format!("session-{request_id}"));
    let session_id = if session_id.trim().is_empty() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_session",
            "session_id is invalid",
        ));
    } else {
        session_id
    };
    input.tenant_id = tenant_id;
    input.session_id = Some(session_id);
    input.request_id = Some(request_id);
    validate_input(&input)?;
    Ok(input)
}

fn resolve_http_tenant_id(headers: &HeaderMap, requested: &str) -> ApiResult<String> {
    playground_telemetry::resolve_http_tenant_identity(
        headers,
        (!requested.trim().is_empty()).then_some(requested),
    )
    .map_err(|error| {
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
        ApiError::new(status, code, message)
    })
}

pub(crate) async fn list_orders(
    headers: HeaderMap,
    State(state): State<AppState>,
    Query(query): Query<OrderQuery>,
) -> ApiResult<Json<Value>> {
    let parent = playground_telemetry::extract_context(&headers);
    let tenant_id = resolve_http_tenant_id(&headers, &query.tenant_id)?;
    let customer_id = require_customer_id(query.customer_id.as_deref())?;
    let session_id = resolve_session_id(&parent, query.session_id.as_deref())?;
    let client = acquire_db(&state.pool).await?;
    let rows = client.query("SELECT o.id, o.order_number, o.customer_id, o.status, o.currency, o.total_minor, o.created_at, c.session_id FROM orders o LEFT JOIN carts c ON c.tenant_id=o.tenant_id AND c.id=o.cart_id WHERE o.tenant_id=$1 AND o.customer_id=$2 AND ($3::text IS NULL OR c.session_id=$3) ORDER BY o.created_at DESC LIMIT 100", &[&tenant_id, &customer_id, &session_id]).await.map_err(db_error)?;
    let orders = rows.iter().map(|row| json!({
        "id": row.get::<_, String>(0),
        "order_number": row.get::<_, String>(1),
        "tenant_id": tenant_id,
        "customer_id": row.get::<_, Option<String>>(2),
        "session_id": row.get::<_, Option<String>>(7),
        "status": row.get::<_, String>(3),
        "currency": row.get::<_, String>(4),
        "total_minor": row.get::<_, i32>(5),
        "created_at": row.get::<_, std::time::SystemTime>(6).duration_since(std::time::UNIX_EPOCH).ok().map(|duration| duration.as_secs())
    })).collect::<Vec<_>>();
    Ok(Json(
        json!({"orders": orders, "tenant_id": tenant_id, "customer_id": customer_id, "session_id": session_id}),
    ))
}

pub(crate) async fn get_order(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(order_id): Path<String>,
    Query(query): Query<CartQuery>,
) -> ApiResult<Json<Value>> {
    let parent = playground_telemetry::extract_context(&headers);
    let tenant_id = resolve_http_tenant_id(&headers, &query.tenant_id)?;
    let customer_id = require_customer_id(query.customer_id.as_deref())?;
    let session_id = resolve_session_id(&parent, query.session_id.as_deref())?;
    let client = acquire_db(&state.pool).await?;
    let order = client.query_opt("SELECT o.id, o.order_number, o.customer_id, o.status, o.currency, o.subtotal_minor, o.discount_minor, o.tax_minor, o.shipping_minor, o.total_minor, EXTRACT(EPOCH FROM o.created_at)::BIGINT, p.id, p.status, c.session_id FROM orders o LEFT JOIN carts c ON c.tenant_id=o.tenant_id AND c.id=o.cart_id LEFT JOIN LATERAL (SELECT id, status FROM payments WHERE tenant_id=o.tenant_id AND order_id=o.id ORDER BY created_at DESC LIMIT 1) p ON TRUE WHERE o.tenant_id=$1 AND o.id=$2 AND o.customer_id=$3 AND ($4::text IS NULL OR c.session_id=$4)", &[&tenant_id, &order_id, &customer_id, &session_id]).await.map_err(db_error)?.ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "order_not_found", "order was not found"))?;
    let items = client.query("SELECT sku, product_name, quantity, unit_price_minor, discount_minor, line_total_minor FROM order_items WHERE tenant_id=$1 AND order_id=$2 ORDER BY created_at", &[&tenant_id, &order_id]).await.map_err(db_error)?;
    Ok(Json(json!({
        "id": order.get::<_, String>(0), "order_number": order.get::<_, String>(1), "tenant_id": tenant_id, "customer_id": order.get::<_, Option<String>>(2), "session_id": order.get::<_, Option<String>>(13),
        "status": order.get::<_, String>(3), "currency": order.get::<_, String>(4), "subtotal_minor": order.get::<_, i32>(5),
        "discount_minor": order.get::<_, i32>(6), "tax_minor": order.get::<_, i32>(7), "shipping_minor": order.get::<_, i32>(8), "total_minor": order.get::<_, i32>(9),
        "created_at": order.get::<_, i64>(10),
        "payment_id": order.get::<_, Option<String>>(11), "payment_status": order.get::<_, Option<String>>(12),
        "items": items.iter().map(|row| json!({"sku": row.get::<_, String>(0), "product_name": row.get::<_, String>(1), "quantity": row.get::<_, i32>(2), "unit_price_minor": row.get::<_, i32>(3), "discount_minor": row.get::<_, i32>(4), "line_total_minor": row.get::<_, i32>(5)})).collect::<Vec<_>>()
    })))
}

pub(crate) async fn get_cart(
    headers: HeaderMap,
    State(state): State<AppState>,
    Query(query): Query<CartQuery>,
) -> ApiResult<Json<Value>> {
    let parent = playground_telemetry::extract_context(&headers);
    let tenant_id = resolve_http_tenant_id(&headers, &query.tenant_id)?;
    let customer_id = require_customer_id(query.customer_id.as_deref())?;
    let session_id = resolve_session_id(&parent, query.session_id.as_deref())?;
    let client = acquire_db(&state.pool).await?;
    let cart = client.query_opt("SELECT id, status, currency, updated_at, session_id FROM carts WHERE tenant_id=$1 AND customer_id=$2 AND status='active' AND ($3::text IS NULL OR session_id=$3) ORDER BY updated_at DESC LIMIT 1", &[&tenant_id, &customer_id, &session_id]).await.map_err(db_error)?;
    let Some(cart) = cart else {
        return Ok(Json(
            json!({"cart": null, "tenant_id": tenant_id, "session_id": session_id}),
        ));
    };
    let cart_id: String = cart.get(0);
    let items = client.query("SELECT pv.sku, p.name, ci.quantity, ci.unit_price_minor, ci.line_total_minor FROM cart_items ci JOIN product_variants pv ON pv.tenant_id=ci.tenant_id AND pv.id=ci.variant_id JOIN products p ON p.tenant_id=pv.tenant_id AND p.id=pv.product_id WHERE ci.tenant_id=$1 AND ci.cart_id=$2 ORDER BY ci.added_at", &[&tenant_id, &cart_id]).await.map_err(db_error)?;
    Ok(Json(
        json!({"cart": {"id": cart_id, "status": cart.get::<_, String>(1), "currency": cart.get::<_, String>(2), "items": items.iter().map(|row| json!({"sku": row.get::<_, String>(0), "product_name": row.get::<_, String>(1), "quantity": row.get::<_, i32>(2), "unit_price_minor": row.get::<_, i32>(3), "line_total_minor": row.get::<_, i32>(4)})).collect::<Vec<_>>()}, "tenant_id": tenant_id, "session_id": cart.get::<_, String>(4)}),
    ))
}

pub(crate) async fn add_cart_item(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(input): Json<AddCartItemInput>,
) -> ApiResult<Json<Value>> {
    let parent = playground_telemetry::extract_context(&headers);
    let mut input = input;
    input.tenant_id = resolve_http_tenant_id(&headers, &input.tenant_id)?;
    input.session_id = resolve_session_id(&parent, input.session_id.as_deref())?
        .or_else(|| Some(format!("session-{}", uuid::Uuid::new_v4())));
    input.customer_id = require_customer_id(Some(&input.customer_id))?;
    if input.currency_code.len() != 3
        || input.currency_code != input.currency_code.to_ascii_uppercase()
    {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_currency",
            "currency_code must be three uppercase letters",
        ));
    }
    if input.quantity == 0 || input.quantity > MAX_QUANTITY || input.sku.trim().is_empty() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_cart_item",
            "sku and quantity are required",
        ));
    }
    let mut client = acquire_db(&state.pool).await?;
    let transaction = client.transaction().await.map_err(db_error)?;
    let explicit_cart_id = input.cart_id.is_some();
    let requested_session_id = input.session_id.clone();
    let mut session_id = requested_session_id
        .clone()
        .unwrap_or_else(|| format!("session-{}", uuid::Uuid::new_v4()));
    let (cart_id, insert_cart) = if let Some(cart_id) = input.cart_id {
        (cart_id, true)
    } else {
        let lock_key = format!("cart:{}:{}", input.tenant_id, input.customer_id);
        transaction
            .query_one(
                "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))",
                &[&lock_key],
            )
            .await
            .map_err(db_error)?;

        let session_cart = if let Some(session_id) = requested_session_id.as_deref() {
            transaction
                .query_opt(
                    "SELECT id
                     FROM carts
                     WHERE tenant_id=$1
                       AND customer_id=$2
                       AND session_id=$3
                       AND currency=$4
                       AND status='active'
                     ORDER BY updated_at DESC
                     LIMIT 1
                     FOR UPDATE",
                    &[
                        &input.tenant_id,
                        &input.customer_id,
                        &session_id,
                        &input.currency_code,
                    ],
                )
                .await
                .map_err(db_error)?
        } else {
            None
        };
        let existing_cart = match session_cart {
            Some(row) => Some(row),
            None => transaction
                .query_opt(
                    "SELECT id
                     FROM carts
                     WHERE tenant_id=$1
                       AND customer_id=$2
                       AND currency=$3
                       AND status='active'
                     ORDER BY updated_at DESC
                     LIMIT 1
                     FOR UPDATE",
                    &[&input.tenant_id, &input.customer_id, &input.currency_code],
                )
                .await
                .map_err(db_error)?,
        };

        if let Some(row) = existing_cart {
            (row.get(0), false)
        } else {
            (format!("cart-{}", uuid::Uuid::new_v4()), true)
        }
    };

    if insert_cart {
        let inserted = transaction
            .query_opt(
                "INSERT INTO carts
                    (id, tenant_id, customer_id, session_id, status, currency, expires_at)
                 VALUES ($1,$2,$3,$4,'active',$5,CURRENT_TIMESTAMP + INTERVAL '7 days')
                 ON CONFLICT DO NOTHING
                 RETURNING id",
                &[
                    &cart_id,
                    &input.tenant_id,
                    &input.customer_id,
                    &session_id,
                    &input.currency_code,
                ],
            )
            .await
            .map_err(db_error)?;

        if inserted.is_none() && !explicit_cart_id {
            session_id = format!("session-{}", uuid::Uuid::new_v4());
            transaction
                .execute(
                    "INSERT INTO carts
                        (id, tenant_id, customer_id, session_id, status, currency, expires_at)
                     VALUES ($1,$2,$3,$4,'active',$5,CURRENT_TIMESTAMP + INTERVAL '7 days')",
                    &[
                        &cart_id,
                        &input.tenant_id,
                        &input.customer_id,
                        &session_id,
                        &input.currency_code,
                    ],
                )
                .await
                .map_err(db_error)?;
        }
    }
    let cart = transaction
        .query_opt(
            "SELECT customer_id, status, currency, session_id FROM carts WHERE tenant_id=$1 AND id=$2 FOR UPDATE",
            &[&input.tenant_id, &cart_id],
        )
        .await
        .map_err(db_error)?
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                "cart_not_found",
                "cart was not found for this tenant",
            )
        })?;
    let cart_customer: Option<String> = cart.get(0);
    let cart_status: String = cart.get(1);
    let cart_currency: String = cart.get(2);
    let cart_session: String = cart.get(3);
    if cart_customer.as_deref() != Some(input.customer_id.as_str())
        || cart_status != "active"
        || cart_currency != input.currency_code
        || cart_session != session_id
    {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "cart_not_found",
            "cart was not found for this customer",
        ));
    }
    let row = transaction.query_opt("SELECT pv.id, p.name, pv.name, pr.amount_minor FROM product_variants pv JOIN products p ON p.tenant_id=pv.tenant_id AND p.id=pv.product_id JOIN prices pr ON pr.tenant_id=pv.tenant_id AND pr.variant_id=pv.id AND pr.currency=$3 AND pr.is_default AND pr.valid_from <= CURRENT_TIMESTAMP AND (pr.valid_to IS NULL OR pr.valid_to > CURRENT_TIMESTAMP) WHERE pv.tenant_id=$1 AND pv.sku=$2 AND pv.status='active' ORDER BY pr.valid_from DESC LIMIT 1", &[&input.tenant_id, &input.sku, &input.currency_code]).await.map_err(db_error)?.ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "product_not_found", "SKU has no active price"))?;
    let variant_id: String = row.get(0);
    let unit_price: i32 = row.get(3);
    let quantity = i32::try_from(input.quantity).map_err(|_| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_cart_quantity",
            "quantity exceeds the database integer range",
        )
    })?;
    let max_quantity = i32::try_from(MAX_QUANTITY).unwrap_or(i32::MAX);
    let changed = transaction.execute("INSERT INTO cart_items (id, tenant_id, cart_id, variant_id, quantity, unit_price_minor, discount_minor) VALUES ($1,$2,$3,$4,$5,$6,0) ON CONFLICT (tenant_id, cart_id, variant_id) DO UPDATE SET quantity=cart_items.quantity + EXCLUDED.quantity, unit_price_minor=EXCLUDED.unit_price_minor WHERE cart_items.quantity + EXCLUDED.quantity <= $7", &[&format!("cart-item-{}", uuid::Uuid::new_v4()), &input.tenant_id, &cart_id, &variant_id, &quantity, &unit_price, &max_quantity]).await.map_err(db_error)?;
    if changed != 1 {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "cart_quantity_limit",
            format!("cart item quantity cannot exceed {MAX_QUANTITY}"),
        ));
    }
    transaction
        .execute(
            "UPDATE carts SET updated_at=CURRENT_TIMESTAMP WHERE tenant_id=$1 AND id=$2",
            &[&input.tenant_id, &cart_id],
        )
        .await
        .map_err(db_error)?;
    transaction.commit().await.map_err(db_error)?;
    Ok(Json(
        json!({"cart_id": cart_id, "session_id": session_id, "sku": input.sku, "quantity_added": input.quantity, "unit_price_minor": unit_price}),
    ))
}

pub(crate) async fn replace_cart_item(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(sku): Path<String>,
    Json(input): Json<ReplaceCartItemInput>,
) -> ApiResult<Json<Value>> {
    let parent = playground_telemetry::extract_context(&headers);
    let mut input = input;
    input.scope.tenant_id = resolve_http_tenant_id(&headers, &input.scope.tenant_id)?;
    let session_id = resolve_session_id(&parent, None)?;
    let quantity = input.validate(&sku)?;
    let quantity = i32::try_from(quantity).map_err(|_| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_cart_quantity",
            format!("quantity must be between 1 and {MAX_QUANTITY}"),
        )
    })?;
    let mut client = acquire_db(&state.pool).await?;
    let transaction = client.transaction().await.map_err(db_error)?;
    let cart_id = &input.scope.cart_id;

    transaction
        .query_opt(
            "SELECT id FROM carts WHERE tenant_id=$1 AND id=$2 AND customer_id=$3 AND status='active' AND ($4::text IS NULL OR session_id=$4) FOR UPDATE",
            &[&input.scope.tenant_id, cart_id, &input.scope.customer_id, &session_id],
        )
        .await
        .map_err(db_error)?
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                "cart_not_found",
                "active cart was not found",
            )
        })?;

    let row = transaction
        .query_opt(
            "UPDATE cart_items AS ci
             SET quantity=$4
             FROM product_variants AS pv
             WHERE ci.tenant_id=$1
               AND ci.cart_id=$2
               AND ci.variant_id=pv.id
               AND pv.tenant_id=$1
               AND pv.sku=$3
             RETURNING ci.quantity, ci.unit_price_minor, ci.line_total_minor",
            &[&input.scope.tenant_id, cart_id, &sku, &quantity],
        )
        .await
        .map_err(db_error)?
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                "cart_item_not_found",
                "cart item was not found",
            )
        })?;

    transaction
        .execute(
            "UPDATE carts SET updated_at=CURRENT_TIMESTAMP WHERE tenant_id=$1 AND id=$2",
            &[&input.scope.tenant_id, cart_id],
        )
        .await
        .map_err(db_error)?;
    transaction.commit().await.map_err(db_error)?;

    Ok(Json(json!({
        "cart_id": cart_id,
        "sku": sku,
        "quantity": row.get::<_, i32>(0),
        "unit_price_minor": row.get::<_, i32>(1),
        "line_total_minor": row.get::<_, i32>(2),
    })))
}

pub(crate) async fn remove_cart_item(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(sku): Path<String>,
    Json(scope): Json<CartItemScope>,
) -> ApiResult<Json<Value>> {
    let parent = playground_telemetry::extract_context(&headers);
    let mut scope = scope;
    scope.tenant_id = resolve_http_tenant_id(&headers, &scope.tenant_id)?;
    let session_id = resolve_session_id(&parent, None)?;
    scope.validate()?;
    validate_cart_sku(&sku)?;
    let mut client = acquire_db(&state.pool).await?;
    let transaction = client.transaction().await.map_err(db_error)?;
    let cart_id = &scope.cart_id;

    transaction
        .query_opt(
            "SELECT id FROM carts WHERE tenant_id=$1 AND id=$2 AND customer_id=$3 AND status='active' AND ($4::text IS NULL OR session_id=$4) FOR UPDATE",
            &[&scope.tenant_id, cart_id, &scope.customer_id, &session_id],
        )
        .await
        .map_err(db_error)?
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                "cart_not_found",
                "active cart was not found",
            )
        })?;

    let row = transaction
        .query_opt(
            "DELETE FROM cart_items AS ci
             USING product_variants AS pv
             WHERE ci.tenant_id=$1
               AND ci.cart_id=$2
               AND ci.variant_id=pv.id
               AND pv.tenant_id=$1
               AND pv.sku=$3
             RETURNING ci.quantity, ci.unit_price_minor, ci.line_total_minor",
            &[&scope.tenant_id, cart_id, &sku],
        )
        .await
        .map_err(db_error)?
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                "cart_item_not_found",
                "cart item was not found",
            )
        })?;

    transaction
        .execute(
            "UPDATE carts SET updated_at=CURRENT_TIMESTAMP WHERE tenant_id=$1 AND id=$2",
            &[&scope.tenant_id, cart_id],
        )
        .await
        .map_err(db_error)?;
    transaction.commit().await.map_err(db_error)?;

    Ok(Json(json!({
        "cart_id": cart_id,
        "sku": sku,
        "removed": true,
        "quantity_removed": row.get::<_, i32>(0),
        "unit_price_minor": row.get::<_, i32>(1),
        "line_total_minor": row.get::<_, i32>(2),
    })))
}

struct HealthSnapshot {
    database: bool,
    rabbit: bool,
    catalog: bool,
    pricing: bool,
    payment: bool,
    inventory: bool,
    recommendation: bool,
}

async fn health_snapshot(state: &AppState) -> HealthSnapshot {
    let database = match tokio::time::timeout(DB_TIMEOUT, state.pool.get()).await {
        Ok(Ok(client)) => client.query_one("SELECT 1", &[]).await.is_ok(),
        _ => false,
    };
    let rabbit = state.rabbit.is_ready();
    let catalog_health = sibling_endpoint(&state.catalog_url, "/actuator/health");
    let inventory_health = sibling_endpoint(&state.inventory_url, "/healthz");
    let recommendation_health = sibling_endpoint(&state.recommendation_url, "/readyz");
    let (catalog, pricing, payment, inventory, recommendation) = tokio::join!(
        http_dependency_ready(&state.http, &catalog_health),
        grpc_dependency_ready(pricing_channel(state), PRICING_GRPC_HEALTH_SERVICE),
        grpc_dependency_ready(payment_channel(state), PAYMENT_GRPC_HEALTH_SERVICE),
        http_dependency_ready(&state.http, &inventory_health),
        http_dependency_ready(&state.http, &recommendation_health),
    );
    HealthSnapshot {
        database,
        rabbit,
        catalog,
        pricing,
        payment,
        inventory,
        recommendation,
    }
}

fn health_response(snapshot: HealthSnapshot, require_messaging: bool) -> impl IntoResponse {
    let status = snapshot.database
        && snapshot.catalog
        && snapshot.pricing
        && snapshot.payment
        && snapshot.inventory
        && snapshot.recommendation
        && (!require_messaging || snapshot.rabbit);
    (
        if status {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        Json(json!({
            "status": if status {"UP"} else {"DOWN"},
            "database": if snapshot.database {"UP"} else {"DOWN"},
            "messaging": if snapshot.rabbit {"UP"} else {"DOWN"},
            "catalog": if snapshot.catalog {"UP"} else {"DOWN"},
            "pricing": if snapshot.pricing {"UP"} else {"DOWN"},
            "payment": if snapshot.payment {"UP"} else {"DOWN"},
            "inventory": if snapshot.inventory {"UP"} else {"DOWN"},
            "recommendation": if snapshot.recommendation {"UP"} else {"DOWN"}
        })),
    )
}

pub(crate) async fn health(State(state): State<AppState>) -> impl IntoResponse {
    health_response(health_snapshot(&state).await, false)
}

pub(crate) async fn readiness(State(state): State<AppState>) -> impl IntoResponse {
    health_response(health_snapshot(&state).await, true)
}

async fn http_dependency_ready(http: &reqwest::Client, endpoint: &str) -> bool {
    tokio::time::timeout(Duration::from_secs(2), http.get(endpoint).send())
        .await
        .ok()
        .and_then(Result::ok)
        .is_some_and(|response| response.status().is_success())
}

async fn grpc_dependency_ready<F>(channel: F, service: &'static str) -> bool
where
    F: Future<Output = ApiResult<Channel>>,
{
    let Ok(channel) = channel.await else {
        return false;
    };
    let response = tokio::time::timeout(
        Duration::from_secs(2),
        HealthClient::new(channel).check(Request::new(HealthCheckRequest {
            service: service.to_owned(),
        })),
    )
    .await;
    match response {
        Ok(Ok(response)) => response.into_inner().status == ServingStatus::Serving as i32,
        _ => false,
    }
}

fn sibling_endpoint(endpoint: &str, path: &str) -> String {
    reqwest::Url::parse(endpoint)
        .map(|mut url| {
            url.set_path(path);
            url.set_query(None);
            url.to_string()
        })
        .unwrap_or_else(|_| format!("{}{}", endpoint.trim_end_matches('/'), path))
}

pub(crate) fn cors_layer() -> CorsLayer {
    let origin = std::env::var("WEB_ORIGIN")
        .ok()
        .and_then(|value| value.parse::<HeaderValue>().ok())
        .map(AllowOrigin::exact)
        .unwrap_or_else(|| AllowOrigin::exact(HeaderValue::from_static("http://localhost:5173")));
    CorsLayer::new()
        .allow_origin(origin)
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
        ])
        .allow_headers([
            header::CONTENT_TYPE,
            HeaderName::from_static("traceparent"),
            HeaderName::from_static("tracestate"),
            HeaderName::from_static("baggage"),
        ])
}

pub(crate) fn app(state: AppState) -> Router {
    Router::new()
        .route("/checkout", post(checkout_post))
        .route("/quote-stream", get(quote_stream))
        .route("/api/orders", get(list_orders))
        .route("/api/orders/{order_id}", get(get_order))
        .route("/api/cart", get(get_cart))
        .route("/api/cart/items", post(add_cart_item))
        .route(
            "/api/cart/items/{sku}",
            put(replace_cart_item)
                .patch(replace_cart_item)
                .delete(remove_cart_item),
        )
        .route("/healthz", get(health))
        .route("/readyz", get(readiness))
        .layer(cors_layer())
        .layer(axum::middleware::from_fn(
            playground_telemetry::http_server_observability,
        ))
        .with_state(state)
}
