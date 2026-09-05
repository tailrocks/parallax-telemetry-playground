//! Checkout use cases and cross-service orchestration.

use crate::domain::*;
use crate::infrastructure::*;
use axum::Json;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use open_feature::EvaluationContext;
use opentelemetry::baggage::BaggageExt;
use opentelemetry::trace::TraceContextExt;
use opentelemetry::{Context, KeyValue};
use playground_proto::payment::v1::{
    AuthorizeRequest, CaptureRequest, GetPaymentRequest, Money as PaymentMoney,
    PaymentFailureReason, PaymentMethod, PaymentMethodType, PaymentOperationStatus, PaymentStatus,
    RefundReason, RefundRequest, VoidReason, VoidRequest, payment_client::PaymentClient,
};
use playground_proto::pricing::v1::{
    Money as QuoteMoney, QuoteItem, QuoteRequest, QuoteResponse, QuoteStatus,
    pricing_client::PricingClient,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::time::Duration;
use tonic::Code;
use tracing_opentelemetry::OpenTelemetrySpanExt;
pub(crate) async fn checkout_inner(
    state: AppState,
    input: CheckoutInput,
    context: Context,
) -> ApiResult<Json<Value>> {
    let request_id = input
        .request_id
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("checkout-{}", uuid::Uuid::new_v4()));
    let (tenant_id, session_id) = resolve_checkout_identity(
        &context,
        &input.tenant_id,
        input.session_id.as_deref(),
        &format!("session-{request_id}"),
    )?;
    let mut input = input;
    input.tenant_id = tenant_id;
    input.session_id = Some(session_id.clone());
    input.request_id = Some(request_id.clone());
    validate_input(&input)?;
    require_payment_credentials(&input)?;
    let context = context_with_identity(&context, &input.tenant_id, &session_id);

    let tenant_id = input.tenant_id.clone();
    let fingerprint = checkout_request_fingerprint(&input);
    let lease_token =
        match claim_checkout_attempt(&state, &tenant_id, &request_id, &fingerprint).await? {
            CheckoutAttemptClaim::New { lease_token } => lease_token,
            CheckoutAttemptClaim::Pending(payload) => {
                wake_pending_payment_reconciliation(&state, &tenant_id, &request_id).await?;
                return Ok(Json(payload));
            }
            CheckoutAttemptClaim::Replay(payload) => return Ok(Json(payload)),
            CheckoutAttemptClaim::InProgress => {
                return Err(ApiError::new(
                    StatusCode::CONFLICT,
                    "checkout_in_progress",
                    "request_id is already being processed",
                ));
            }
            CheckoutAttemptClaim::Failed(error) => return Err(error),
        };

    let lease = CheckoutLease {
        tenant_id: tenant_id.clone(),
        request_id: request_id.clone(),
        token: lease_token,
    };
    let result = checkout_flow(state.clone(), input, context, &lease).await;
    match result {
        Ok(Json(payload)) => {
            let status = match payload.get("status").and_then(Value::as_str) {
                Some("payment_pending") => "pending",
                Some("degraded") => "degraded",
                Some("paid") => "paid",
                _ => "failed",
            };
            let order_id = payload.get("order_id").and_then(Value::as_str);
            record_checkout_response(
                &state,
                &tenant_id,
                &request_id,
                &lease.token,
                status,
                order_id,
                &payload,
            )
            .await
            .inspect_err(|error| {
                tracing::error!(
                    tenant_id = %tenant_id,
                    request_id = %request_id,
                    error = %error.message,
                    "checkout idempotency response persistence is authoritative"
                );
            })?;
            if status == "pending" {
                wake_pending_payment_reconciliation(&state, &tenant_id, &request_id).await?;
            }
            Ok(Json(payload))
        }
        Err(error) => {
            if let Err(record_error) =
                record_checkout_failure(&state, &tenant_id, &request_id, &lease.token, &error).await
            {
                tracing::error!(
                    tenant_id = %tenant_id,
                    request_id = %request_id,
                    error = %record_error.message,
                    "checkout idempotency failure persistence did not own the active lease"
                );
                if record_error.code != "checkout_lease_lost" {
                    return Err(record_error);
                }
            }
            if error.code == "checkout_post_consume_reconciliation" {
                wake_pending_payment_reconciliation(&state, &tenant_id, &request_id).await?;
            }
            Err(error)
        }
    }
}

pub(crate) async fn checkout_flow(
    state: AppState,
    input: CheckoutInput,
    context: Context,
    lease: &CheckoutLease,
) -> ApiResult<Json<Value>> {
    let request_id = lease.request_id.as_str();
    let (payment_method_token, payment_method_type) = require_payment_credentials(&input)?;
    let fence = CheckoutFence::Attempt(lease.clone());
    assert_checkout_fence(&state, &fence).await?;
    if input.unbounded_fault_requested() {
        tracing::warn!("bounded checkout scenario requested");
    }
    let delay_ms = input.delay_ms.max(input.slow).min(30_000);
    if delay_ms > 0 {
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
    }

    let checkout_variant = tokio::time::timeout(
        Duration::from_millis(500),
        playground_telemetry::feature_variant(
            "checkoutFlow",
            "orchestrated",
            "CHECKOUT_FLOW",
            checkout_evaluation_context(&input),
        ),
    )
    .await
    .ok()
    .unwrap_or_else(|| "orchestrated".to_owned());
    tracing::Span::current().set_attribute("feature_flag.key", "checkoutFlow");
    tracing::Span::current().set_attribute("feature_flag.variant", checkout_variant.clone());
    let context = context_with_checkout_variant(&context, &checkout_variant);

    let catalog = validate_catalog(&state, &context, &input).await?;
    let quote = quote_with_retry(&state, &context, &input, request_id, &checkout_variant).await?;
    let pending = create_pending_order(&state, &input, &quote, lease).await?;

    let authorization = match with_checkout_fence(&state, &fence, || async {
        authorize_payment(
            &state,
            &context,
            &pending,
            request_id,
            payment_method_token,
            payment_method_type,
            input.timeout_ms,
        )
        .await
    })
    .await
    {
        Ok(authorization) => authorization,
        Err(error) => {
            if error.code == "checkout_lease_lost" {
                queue_checkout_finalization_recovery(
                    &state,
                    &pending,
                    lease,
                    None,
                    payment_method_type,
                    &checkout_variant,
                    &context,
                )
                .await?;
                return Err(error);
            }
            if !error.is_ambiguous_payment_failure() {
                cancel_order_fenced(&state, &pending, &fence).await?;
                return Err(error);
            }

            match with_checkout_fence(&state, &fence, || async {
                recover_authorization(
                    &state,
                    &context,
                    &pending,
                    request_id,
                    payment_method_token,
                    payment_method_type,
                    input.timeout_ms,
                    &fence,
                )
                .await
            })
            .await
            {
                Ok(AuthorizationRecovery::Recovered(authorization)) => authorization,
                Ok(AuthorizationRecovery::Absent) => {
                    cancel_order_fenced(&state, &pending, &fence).await?;
                    if input.degrade {
                        return Ok(Json(json!({
                            "status": "degraded",
                            "order_id": pending.id,
                            "catalog": catalog,
                            "quote": quote_json(&quote),
                            "payment": {"status": "unavailable", "error": error.code},
                            "payment_status": "unavailable"
                        })));
                    }
                    return Err(error);
                }
                Ok(AuthorizationRecovery::Pending) => {
                    register_pending_payment_reconciliation(
                        &state,
                        &pending,
                        lease,
                        None,
                        payment_method_type,
                        &checkout_variant,
                        &context,
                    )
                    .await?;
                    let reconciliation_error = payment_reconciliation_error(
                        "payment authorization outcome is not yet observable",
                    );
                    if input.degrade {
                        return Ok(Json(json!({
                            "status": "payment_pending",
                            "order_status": "pending",
                            "order_id": pending.id,
                            "order_number": pending.order_number,
                            "tenant_id": pending.tenant_id,
                            "customer_id": pending.customer_id,
                            "currency": pending.currency,
                            "total_minor": pending.total_minor,
                            "payment_id": Value::Null,
                            "payment_status": "unknown",
                            "payment_operation_status": PaymentOperationStatus::Unspecified.as_str_name(),
                            "payment_failure_reason": PaymentFailureReason::ProviderUnavailable.as_str_name(),
                            "payment_reconciliation": "pending",
                            "degraded": true,
                            "feature_variant": checkout_variant,
                            "catalog": catalog,
                            "quote": quote_json(&quote)
                        })));
                    }
                    return Err(reconciliation_error);
                }
                Ok(AuthorizationRecovery::Failed(recovery_error)) => {
                    cancel_order_fenced(&state, &pending, &fence).await?;
                    return Err(recovery_error);
                }
                Err(recovery_error) => {
                    tracing::error!(
                        order_id = %pending.id,
                        error = %recovery_error.message,
                        "payment authorization outcome could not be reconciled; leaving order pending"
                    );
                    return Err(error);
                }
            }
        }
    };

    if authorization.operation_status == PaymentOperationStatus::Pending {
        register_pending_payment_reconciliation(
            &state,
            &pending,
            lease,
            Some(&authorization.payment_id),
            payment_method_type,
            &checkout_variant,
            &context,
        )
        .await?;
        return Ok(Json(json!({
            "status": "payment_pending",
            "order_status": "pending",
            "order_id": pending.id,
            "order_number": pending.order_number,
            "tenant_id": pending.tenant_id,
            "customer_id": pending.customer_id,
            "currency": pending.currency,
            "total_minor": pending.total_minor,
            "payment_id": authorization.payment_id,
            "payment_status": "pending",
            "payment_operation_status": authorization.operation_status.as_str_name(),
            "payment_failure_reason": authorization.failure_reason.as_str_name(),
            "feature_variant": checkout_variant,
            "catalog": catalog,
            "quote": quote_json(&quote)
        })));
    }
    let payment_id = authorization.payment_id;

    let reservations =
        match reserve_inventory(&state, &context, &pending, &fence, input.timeout_ms).await {
            Ok(reservations) => reservations,
            Err(error) => {
                if error.code == "checkout_lease_lost" {
                    queue_checkout_finalization_recovery(
                        &state,
                        &pending,
                        lease,
                        Some(&payment_id),
                        payment_method_type,
                        &checkout_variant,
                        &context,
                    )
                    .await?;
                    return Err(error);
                }
                enqueue_payment_compensation_task(
                    &state,
                    &pending,
                    &payment_id,
                    request_id,
                    &context,
                    &fence,
                )
                .await?;
                let payment_compensated = compensate_payment_with_fence(
                    &state,
                    &context,
                    &pending,
                    &payment_id,
                    request_id,
                    input.timeout_ms,
                    &fence,
                )
                .await?;
                if !payment_compensated {
                    tracing::error!(
                        order_id = %pending.id,
                        "payment authorization compensation was not confirmed"
                    );
                } else {
                    resolve_compensation_intent(
                        &state,
                        &pending,
                        &payment_compensation_task_key(&payment_id),
                        &fence,
                    )
                    .await?;
                    cancel_order_if_compensated(&state, &pending, &fence).await?;
                }
                return Err(error);
            }
        };

    if let Err(error) = with_checkout_fence(&state, &fence, || async {
        capture_payment(
            &state,
            &context,
            &pending,
            &payment_id,
            request_id,
            input.timeout_ms,
        )
        .await
    })
    .await
    {
        if error.code == "checkout_lease_lost" {
            queue_checkout_finalization_recovery(
                &state,
                &pending,
                lease,
                Some(&payment_id),
                payment_method_type,
                &checkout_variant,
                &context,
            )
            .await?;
            return Err(error);
        }
        let inventory_compensated = match release_inventory_with_recovery(
            &state,
            &context,
            &pending,
            &reservations,
            &fence,
            input.timeout_ms,
        )
        .await
        {
            Ok(()) => true,
            Err(compensation_error) => {
                tracing::error!(
                    order_id = %pending.id,
                    error = %compensation_error.message,
                    "inventory compensation failed after payment capture error"
                );
                false
            }
        };
        enqueue_payment_compensation_task(
            &state,
            &pending,
            &payment_id,
            request_id,
            &context,
            &fence,
        )
        .await?;
        let payment_compensated = compensate_payment_with_fence(
            &state,
            &context,
            &pending,
            &payment_id,
            request_id,
            input.timeout_ms,
            &fence,
        )
        .await?;
        if !payment_compensated {
            tracing::error!(
                order_id = %pending.id,
                "payment capture compensation was not confirmed"
            );
        }
        if payment_compensated {
            resolve_compensation_intent(
                &state,
                &pending,
                &payment_compensation_task_key(&payment_id),
                &fence,
            )
            .await?;
        }
        if inventory_compensated && payment_compensated {
            cancel_order_if_compensated(&state, &pending, &fence).await?;
        }
        return Err(error);
    }

    if let Err(error) = consume_inventory(
        &state,
        &context,
        &pending,
        &reservations,
        &fence,
        input.timeout_ms,
    )
    .await
    {
        if error.code == "checkout_lease_lost" {
            queue_checkout_finalization_recovery(
                &state,
                &pending,
                lease,
                Some(&payment_id),
                payment_method_type,
                &checkout_variant,
                &context,
            )
            .await?;
            return Err(error);
        }
        if is_retryable_inventory_consume_error(&error) {
            register_pending_payment_reconciliation(
                &state,
                &pending,
                lease,
                Some(&payment_id),
                payment_method_type,
                &checkout_variant,
                &context,
            )
            .await?;
            return Err(ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "checkout_post_consume_reconciliation",
                "inventory consume outcome is ambiguous; durable checkout reconciliation is queued",
            ));
        }
        let inventory_compensated = match release_inventory_with_recovery(
            &state,
            &context,
            &pending,
            &reservations,
            &fence,
            input.timeout_ms,
        )
        .await
        {
            Ok(()) => true,
            Err(compensation_error) => {
                tracing::error!(
                    order_id = %pending.id,
                    error = %compensation_error.message,
                    "inventory compensation failed after inventory consume error"
                );
                false
            }
        };
        enqueue_payment_compensation_task(
            &state,
            &pending,
            &payment_id,
            request_id,
            &context,
            &fence,
        )
        .await?;
        let payment_compensated = compensate_payment_with_fence(
            &state,
            &context,
            &pending,
            &payment_id,
            request_id,
            input.timeout_ms,
            &fence,
        )
        .await?;
        if payment_compensated {
            resolve_compensation_intent(
                &state,
                &pending,
                &payment_compensation_task_key(&payment_id),
                &fence,
            )
            .await?;
        }
        if inventory_compensated && payment_compensated {
            cancel_order_if_compensated(&state, &pending, &fence).await?;
        }
        return Err(error);
    }

    if let Err(error) = finalize_order(
        &state,
        &context,
        &pending,
        &payment_id,
        request_id,
        &checkout_variant,
        &fence,
    )
    .await
    {
        // A lost response from COMMIT is not evidence of rollback. Reconcile
        // the durable order before issuing any refund or inventory release.
        if error.code == "checkout_lease_lost" {
            queue_checkout_finalization_recovery(
                &state,
                &pending,
                lease,
                Some(&payment_id),
                payment_method_type,
                &checkout_variant,
                &context,
            )
            .await?;
            return Err(error);
        }
        let committed = match finalization_committed(&state, &pending).await {
            Ok(committed) => committed,
            Err(reconciliation_error)
                if reconciliation_error.code == "finalization_reconciliation_pending" =>
            {
                false
            }
            Err(reconciliation_error) => return Err(reconciliation_error),
        };
        if committed {
            tracing::warn!(
                order_id = %pending.id,
                "finalization commit response was ambiguous; durable paid state won"
            );
        } else {
            register_pending_payment_reconciliation(
                &state,
                &pending,
                lease,
                Some(&payment_id),
                payment_method_type,
                &checkout_variant,
                &context,
            )
            .await?;
            return Err(ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "checkout_post_consume_reconciliation",
                "inventory is already consumed; durable order finalization reconciliation is queued",
            ));
        }
    }

    let recommendation = if !checkout_variant_includes_recommendation(&checkout_variant) {
        tracing::info!("checkoutFlow control variant skips recommendation topology");
        Value::Null
    } else {
        recommendation(
            &state,
            &context,
            &pending.tenant_id,
            &input.segment,
            &input.items[0].sku,
        )
        .await
        .unwrap_or_else(|error| json!({"error": error.message}))
    };
    playground_telemetry::emit_event(
        "checkout.completed",
        &[
            ("order_id", pending.id.clone()),
            ("tenant_id", pending.tenant_id.clone()),
            ("total_minor", pending.total_minor.to_string()),
            ("payment_id", payment_id.clone()),
        ],
    );
    let event_key = format!("{}:paid", pending.id);
    Ok(Json(json!({
        "status": "paid",
        "order_id": pending.id,
        "order_number": pending.order_number,
        "tenant_id": pending.tenant_id,
        "customer_id": pending.customer_id,
        "currency": pending.currency,
        "subtotal_minor": pending.subtotal_minor,
        "discount_minor": pending.discount_minor,
        "total_minor": pending.total_minor,
        "payment_id": payment_id,
        "payment_status": "captured",
        "event_key": event_key,
        "feature_variant": checkout_variant,
        "catalog": catalog,
        "quote": quote_json(&quote),
        "recommendation": recommendation
    })))
}

pub(crate) fn checkout_request_fingerprint(input: &CheckoutInput) -> String {
    let canonical = json!({
        "tenant_id": input.tenant_id,
        "customer_id": input.customer_id,
        "session_id": input.session_id,
        "cart_id": input.cart_id,
        "items": input.items,
        "currency_code": input.currency_code,
        "promotion_code": input.promotion_code,
        "segment": input.segment,
        "tier": input.tier,
        "region": input.region,
        "priority": input.priority,
        "payment_method_token": input.payment_method_token,
        "payment_method_type": input.payment_method_type,
        "delay_ms": input.delay_ms,
        "slow": input.slow,
        "retry": input.retry,
        "timeout_ms": input.timeout_ms,
        "degrade": input.degrade,
    });
    let digest = Sha256::digest(canonical.to_string().as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(crate) async fn claim_checkout_attempt(
    state: &AppState,
    tenant_id: &str,
    request_id: &str,
    fingerprint: &str,
) -> ApiResult<CheckoutAttemptClaim> {
    let mut client = acquire_db(&state.pool).await?;
    let transaction = client.transaction().await.map_err(db_error)?;
    let lease_token = uuid::Uuid::new_v4().to_string();
    let inserted = transaction
        .execute(
            "INSERT INTO checkout_attempts (tenant_id, request_id, request_fingerprint, status, lease_token) VALUES ($1,$2,$3,'started',$4) ON CONFLICT (tenant_id, request_id) DO NOTHING",
            &[&tenant_id, &request_id, &fingerprint, &lease_token],
        )
        .await
        .map_err(db_error)?;
    if inserted == 1 {
        transaction.commit().await.map_err(db_error)?;
        return Ok(CheckoutAttemptClaim::New { lease_token });
    }

    let row = transaction
        .query_opt(
            "SELECT request_fingerprint, status, response_payload::text, error_status, error_code, error_message, status = 'started' AND COALESCE(lease_expires_at, updated_at + ($3::double precision * INTERVAL '1 second')) <= CURRENT_TIMESTAMP AS stale_started, lease_token, order_id, (SELECT o.status FROM orders o WHERE o.tenant_id=checkout_attempts.tenant_id AND o.id=checkout_attempts.order_id) AS order_status FROM checkout_attempts WHERE tenant_id=$1 AND request_id=$2 FOR UPDATE",
            &[
                &tenant_id,
                &request_id,
                &(CHECKOUT_ATTEMPT_STALE_AFTER.as_secs() as f64),
            ],
        )
        .await
        .map_err(db_error)?
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "checkout_idempotency_corrupt",
                "checkout idempotency record disappeared",
            )
        })?;
    let stored_fingerprint: String = row.get(0);
    if stored_fingerprint != fingerprint {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "checkout_request_conflict",
            "request_id was already used for different checkout input",
        ));
    }

    let status: String = row.get(1);
    let claim = match status.as_str() {
        "started" => {
            let stale_started = row.get::<_, bool>(6);
            if !stale_started {
                CheckoutAttemptClaim::InProgress
            } else {
                let previous_lease_token: String = row.get(7);
                let order_id: Option<String> = row.get(8);
                let order_status: Option<String> = row.get(9);
                if order_status.as_deref() == Some("paid") {
                    let paid_order_id = order_id.as_deref().ok_or_else(|| {
                        ApiError::new(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            "checkout_idempotency_corrupt",
                            "paid checkout attempt has no order linkage",
                        )
                    })?;
                    let paid_order = transaction
                        .query_one(
                            "SELECT o.order_number, o.customer_id, o.currency, o.total_minor, p.id, p.status FROM orders o LEFT JOIN LATERAL (SELECT id, status FROM payments WHERE tenant_id=o.tenant_id AND order_id=o.id ORDER BY created_at DESC LIMIT 1) p ON TRUE WHERE o.tenant_id=$1 AND o.id=$2 AND o.status='paid'",
                            &[&tenant_id, &paid_order_id],
                        )
                        .await
                        .map_err(db_error)?;
                    let payload = json!({
                        "status": "paid",
                        "order_status": "paid",
                        "order_id": paid_order_id,
                        "order_number": paid_order.get::<_, String>(0),
                        "tenant_id": tenant_id,
                        "customer_id": paid_order.get::<_, String>(1),
                        "currency": paid_order.get::<_, String>(2),
                        "total_minor": i64::from(paid_order.get::<_, i32>(3)),
                        "payment_id": paid_order.get::<_, Option<String>>(4),
                        "payment_status": paid_order.get::<_, Option<String>>(5).unwrap_or_else(|| "captured".to_owned()),
                        "payment_recovered": true
                    });
                    let updated = transaction
                        .execute(
                            "UPDATE checkout_attempts SET status='paid', response_payload=$3::jsonb, error_status=NULL, error_code=NULL, error_message=NULL, lease_expires_at=NULL, updated_at=CURRENT_TIMESTAMP WHERE tenant_id=$1 AND request_id=$2 AND lease_token=$4 AND status='started' AND order_id=$5",
                            &[&tenant_id, &request_id, &payload, &previous_lease_token, &paid_order_id],
                        )
                        .await
                        .map_err(db_error)?;
                    if updated != 1 {
                        return Err(ApiError::new(
                            StatusCode::CONFLICT,
                            "checkout_lease_lost",
                            "paid checkout reclaim lost its lease",
                        ));
                    }
                    transaction.commit().await.map_err(db_error)?;
                    return Ok(CheckoutAttemptClaim::Replay(payload));
                }
                if !stale_reclaim_order_is_safe(order_id.as_deref(), order_status.as_deref()) {
                    return Err(ApiError::new(
                        StatusCode::CONFLICT,
                        "checkout_reconciliation_required",
                        "stale checkout attempt is linked to a non-pending order",
                    ));
                }
                let new_lease_token = uuid::Uuid::new_v4().to_string();
                let reclaimed = transaction
                    .execute(
                        "UPDATE checkout_attempts SET status='started', lease_token=$4, lease_expires_at=CURRENT_TIMESTAMP + ($3::double precision * INTERVAL '1 second'), response_payload=NULL, error_status=NULL, error_code=NULL, error_message=NULL, updated_at=CURRENT_TIMESTAMP WHERE tenant_id=$1 AND request_id=$2 AND lease_token=$5 AND status='started' AND COALESCE(lease_expires_at, updated_at + ($3::double precision * INTERVAL '1 second')) <= CURRENT_TIMESTAMP",
                        &[
                            &tenant_id,
                            &request_id,
                            &(CHECKOUT_ATTEMPT_STALE_AFTER.as_secs() as f64),
                            &new_lease_token,
                            &previous_lease_token,
                        ],
                    )
                    .await
                    .map_err(db_error)?;
                if reclaimed == 1 {
                    // The parent generation is the durable fence. Retire
                    // every in-flight child claim before exposing the new
                    // generation so an old worker can only observe a fence
                    // loss, never complete work for its successor.
                    transaction
                        .execute(
                            "UPDATE checkout_payment_reconciliations SET checkout_lease_token=$3, status=CASE WHEN status='processing' THEN CASE WHEN attempts < $4 THEN 'queued' ELSE 'awaiting_provider' END ELSE status END, available_at=CASE WHEN status='processing' THEN CURRENT_TIMESTAMP ELSE available_at END, claimed_at=NULL, lease_token=NULL, lease_expires_at=NULL, last_error=CASE WHEN status='processing' THEN COALESCE(last_error, 'parent checkout generation reclaimed; worker claim retired') ELSE last_error END, updated_at=CURRENT_TIMESTAMP WHERE tenant_id=$1 AND request_id=$2 AND checkout_lease_token=$5 AND status <> 'completed'",
                            &[
                                &tenant_id,
                                &request_id,
                                &new_lease_token,
                                &PAYMENT_RECONCILIATION_MAX_ATTEMPTS,
                                &previous_lease_token,
                            ],
                        )
                        .await
                        .map_err(db_error)?;
                    transaction
                        .execute(
                            "UPDATE checkout_compensation_tasks SET status='superseded', claimed_at=NULL, claim_token=NULL, claim_expires_at=NULL, completed_at=NULL, last_error=COALESCE(last_error, 'parent checkout generation reclaimed; compensation intent quarantined'), updated_at=CURRENT_TIMESTAMP WHERE tenant_id=$1 AND checkout_request_id=$2 AND checkout_lease_token=$3 AND status NOT IN ('completed','superseded')",
                            &[&tenant_id, &request_id, &previous_lease_token],
                        )
                        .await
                        .map_err(db_error)?;
                    CheckoutAttemptClaim::New {
                        lease_token: new_lease_token,
                    }
                } else {
                    CheckoutAttemptClaim::InProgress
                }
            }
        }
        "pending" | "paid" | "degraded" => {
            let payload = row
                .get::<_, Option<String>>(2)
                .and_then(|value| serde_json::from_str::<Value>(&value).ok())
                .ok_or_else(|| {
                    ApiError::new(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "checkout_idempotency_corrupt",
                        "checkout idempotency response is invalid",
                    )
                })?;
            if status == "pending" {
                CheckoutAttemptClaim::Pending(payload)
            } else {
                CheckoutAttemptClaim::Replay(payload)
            }
        }
        "failed" => {
            let status_code = row
                .get::<_, Option<i16>>(3)
                .and_then(|value| u16::try_from(value).ok())
                .and_then(|value| StatusCode::from_u16(value).ok())
                .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            let code = replay_error_code(row.get::<_, Option<String>>(4).as_deref().unwrap_or(""));
            let message = row
                .get::<_, Option<String>>(5)
                .unwrap_or_else(|| "checkout request previously failed".to_owned());
            CheckoutAttemptClaim::Failed(ApiError::new(status_code, code, message))
        }
        _ => {
            return Err(ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "checkout_idempotency_corrupt",
                "checkout idempotency state is invalid",
            ));
        }
    };
    transaction.commit().await.map_err(db_error)?;
    Ok(claim)
}

pub(crate) async fn wake_pending_payment_reconciliation(
    state: &AppState,
    tenant_id: &str,
    request_id: &str,
) -> ApiResult<()> {
    wake_payment_reconciliation(state, tenant_id, request_id).await
}

pub(crate) async fn register_pending_payment_reconciliation(
    state: &AppState,
    order: &PendingOrder,
    lease: &CheckoutLease,
    payment_id: Option<&str>,
    method_type: &str,
    feature_variant: &str,
    context: &Context,
) -> ApiResult<()> {
    register_pending_payment_reconciliation_inner(
        state,
        order,
        lease,
        payment_id,
        method_type,
        feature_variant,
        context,
        false,
    )
    .await
}

/// Persist recovery even after the checkout lease has expired.  The old
/// generation remains the durable owner until reclaim takes the parent lock;
/// reclaim then transfers this reconciliation row to the successor.  This is
/// the recovery path for remote capture/consume effects whose response lost
/// the parent lease.
pub(crate) async fn queue_checkout_finalization_recovery(
    state: &AppState,
    order: &PendingOrder,
    lease: &CheckoutLease,
    payment_id: Option<&str>,
    method_type: &str,
    feature_variant: &str,
    context: &Context,
) -> ApiResult<()> {
    register_pending_payment_reconciliation_inner(
        state,
        order,
        lease,
        payment_id,
        method_type,
        feature_variant,
        context,
        true,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn register_pending_payment_reconciliation_inner(
    state: &AppState,
    order: &PendingOrder,
    lease: &CheckoutLease,
    payment_id: Option<&str>,
    method_type: &str,
    feature_variant: &str,
    context: &Context,
    allow_expired_parent: bool,
) -> ApiResult<()> {
    if lease.tenant_id != order.tenant_id {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "checkout_lease_lost",
            "payment reconciliation lease tenant does not match the order",
        ));
    }
    let payment_id = payment_id
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned);
    let authorize_request_id = format!("{}:authorize", lease.request_id);
    let method_type = stored_payment_method_type(method_type);
    let (traceparent, tracestate, baggage) = stored_propagation(context)?;
    let mut client = acquire_db(&state.pool).await?;
    let transaction = client.transaction().await.map_err(db_error)?;
    if allow_expired_parent {
        let parent_exists = transaction
            .query_opt(
                "SELECT 1 FROM checkout_attempts WHERE tenant_id=$1 AND request_id=$2 AND lease_token=$3 AND status='started' FOR UPDATE",
                &[&lease.tenant_id, &lease.request_id, &lease.token],
            )
            .await
            .map_err(db_error)?
            .is_some();
        if !parent_exists {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "checkout_lease_lost",
                "checkout finalization recovery lost its parent generation",
            ));
        }
    } else {
        lock_checkout_fence(&transaction, &CheckoutFence::Attempt(lease.clone())).await?;
    }
    let changed = transaction
        .execute(
            "INSERT INTO checkout_payment_reconciliations (tenant_id, request_id, order_id, authorize_request_id, payment_id, merchant_reference, amount_minor, currency, method_type, feature_variant, status, checkout_lease_token, attempts, available_at, traceparent, tracestate, baggage) SELECT $1,$2,$3,$4,$5,$3,$6,$7,$8,$9,'awaiting_provider',$13,0,CURRENT_TIMESTAMP,$10,$11,$12 WHERE EXISTS (SELECT 1 FROM checkout_attempts a WHERE a.tenant_id=$1 AND a.request_id=$2 AND a.lease_token=$13 AND a.status='started') ON CONFLICT (tenant_id, request_id) DO UPDATE SET payment_id=COALESCE(checkout_payment_reconciliations.payment_id, EXCLUDED.payment_id), checkout_lease_token=CASE WHEN checkout_payment_reconciliations.status='completed' THEN checkout_payment_reconciliations.checkout_lease_token ELSE EXCLUDED.checkout_lease_token END, feature_variant=EXCLUDED.feature_variant, traceparent=EXCLUDED.traceparent, tracestate=EXCLUDED.tracestate, baggage=EXCLUDED.baggage, status=CASE WHEN checkout_payment_reconciliations.status IN ('awaiting_provider','processing','completed','failed') THEN checkout_payment_reconciliations.status ELSE 'awaiting_provider' END, attempts=checkout_payment_reconciliations.attempts, available_at=CASE WHEN checkout_payment_reconciliations.status IN ('processing','completed','failed') THEN checkout_payment_reconciliations.available_at ELSE CURRENT_TIMESTAMP END, last_error=CASE WHEN checkout_payment_reconciliations.status IN ('processing','completed','failed') THEN checkout_payment_reconciliations.last_error ELSE NULL END, updated_at=CURRENT_TIMESTAMP WHERE checkout_payment_reconciliations.order_id=EXCLUDED.order_id AND checkout_payment_reconciliations.authorize_request_id=EXCLUDED.authorize_request_id",
            &[
                &order.tenant_id,
                &lease.request_id,
                &order.id,
                &authorize_request_id,
                &payment_id,
                &order.total_minor,
                &order.currency,
                &method_type,
                &feature_variant,
                &traceparent,
                &tracestate,
                &baggage,
                &lease.token,
            ],
        )
        .await
        .map_err(db_error)?;
    if changed != 1 {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "checkout_lease_lost",
            "payment reconciliation intent was not created by the current checkout lease",
        ));
    }
    transaction.commit().await.map_err(db_error)
}

fn stored_payment_method_type(method: &str) -> &'static str {
    match payment_method_type(method) {
        PaymentMethodType::Card => "card",
        PaymentMethodType::BankAccount => "bank_account",
        PaymentMethodType::Wallet => "wallet",
        PaymentMethodType::Unspecified => "unspecified",
    }
}

pub(crate) async fn record_checkout_response(
    state: &AppState,
    tenant_id: &str,
    request_id: &str,
    lease_token: &str,
    status: &str,
    order_id: Option<&str>,
    payload: &Value,
) -> ApiResult<()> {
    let client = acquire_db(&state.pool).await?;
    let updated = client
        .execute(
            "UPDATE checkout_attempts SET status=$4, order_id=$5, response_payload=$6::jsonb, error_status=NULL, error_code=NULL, error_message=NULL, lease_expires_at=NULL, updated_at=CURRENT_TIMESTAMP WHERE tenant_id=$1 AND request_id=$2 AND lease_token=$3 AND status='started' AND lease_expires_at > CURRENT_TIMESTAMP",
            &[&tenant_id, &request_id, &lease_token, &status, &order_id, payload],
        )
        .await
        .map_err(db_error)?;
    if updated != 1 {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "checkout_lease_lost",
            "checkout idempotency response no longer owns an active lease",
        ));
    }
    Ok(())
}

pub(crate) async fn record_checkout_failure(
    state: &AppState,
    tenant_id: &str,
    request_id: &str,
    lease_token: &str,
    error: &ApiError,
) -> ApiResult<()> {
    let client = acquire_db(&state.pool).await?;
    let updated = client
        .execute(
            "UPDATE checkout_attempts SET status='failed', response_payload=NULL, error_status=$4, error_code=$5, error_message=$6, lease_expires_at=NULL, updated_at=CURRENT_TIMESTAMP WHERE tenant_id=$1 AND request_id=$2 AND lease_token=$3 AND status='started' AND lease_expires_at > CURRENT_TIMESTAMP",
            &[
                &tenant_id,
                &request_id,
                &lease_token,
                &(error.status.as_u16() as i16),
                &error.code,
                &error.message,
            ],
        )
        .await
        .map_err(db_error)?;
    if updated != 1 {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "checkout_lease_lost",
            "checkout idempotency failure no longer owns an active lease",
        ));
    }
    Ok(())
}

pub(crate) fn replay_error_code(code: &str) -> &'static str {
    match code {
        "amount_out_of_range" => "amount_out_of_range",
        "catalog_protocol_error" => "catalog_protocol_error",
        "catalog_query_failed" => "catalog_query_failed",
        "catalog_unavailable" => "catalog_unavailable",
        "database_timeout" => "database_timeout",
        "database_unavailable" => "database_unavailable",
        "event_encode_failed" => "event_encode_failed",
        "event_protocol_error" => "event_protocol_error",
        "invalid_cart_item" => "invalid_cart_item",
        "invalid_cart" => "invalid_cart",
        "invalid_currency" => "invalid_currency",
        "invalid_identity" => "invalid_identity",
        "invalid_items" => "invalid_items",
        "invalid_quantity" => "invalid_quantity",
        "invalid_request_id" => "invalid_request_id",
        "invalid_session" => "invalid_session",
        "inventory_release_failed" => "inventory_release_failed",
        "inventory_release_unavailable" => "inventory_release_unavailable",
        "inventory_protocol_error" => "inventory_protocol_error",
        "inventory_reservation_failed" => "inventory_reservation_failed",
        "inventory_unavailable" => "inventory_unavailable",
        "inventory_commit_failed" => "inventory_commit_failed",
        "inventory_commit_unavailable" => "inventory_commit_unavailable",
        "inventory_commit_protocol_error" => "inventory_commit_protocol_error",
        "checkout_post_consume_reconciliation" => "checkout_post_consume_reconciliation",
        "checkout_reconciliation_required" => "checkout_reconciliation_required",
        "messaging_rejected" => "messaging_rejected",
        "messaging_unavailable" => "messaging_unavailable",
        "order_not_found" => "order_not_found",
        "payment_already_processed" => "payment_already_processed",
        "payment_declined" => "payment_declined",
        "payment_failed" => "payment_failed",
        "payment_insufficient_funds" => "payment_insufficient_funds",
        "payment_internal" => "payment_internal",
        "payment_invalid_method" => "payment_invalid_method",
        "payment_invalid_request" => "payment_invalid_request",
        "payment_invalid_state" => "payment_invalid_state",
        "payment_not_found" => "payment_not_found",
        "payment_pending" => "payment_pending",
        "payment_protocol_error" => "payment_protocol_error",
        "payment_provider_unavailable" => "payment_provider_unavailable",
        "payment_reconciliation_failed" => "payment_reconciliation_failed",
        "payment_reconciliation_pending" => "payment_reconciliation_pending",
        "payment_timeout" => "payment_timeout",
        "pricing_failed" => "pricing_failed",
        "pricing_invalid_request" => "pricing_invalid_request",
        "pricing_protocol_error" => "pricing_protocol_error",
        "pricing_timeout" => "pricing_timeout",
        "pricing_unavailable" => "pricing_unavailable",
        "product_not_found" => "product_not_found",
        "recommendation_protocol_error" => "recommendation_protocol_error",
        "recommendation_unavailable" => "recommendation_unavailable",
        _ => "checkout_request_failed",
    }
}

pub(crate) fn stale_reclaim_order_is_safe(
    order_id: Option<&str>,
    order_status: Option<&str>,
) -> bool {
    order_id.is_none() || order_status == Some("pending")
}

#[cfg(test)]
pub(crate) fn started_checkout_attempt_claim(stale_started: bool) -> CheckoutAttemptClaim {
    if stale_started {
        CheckoutAttemptClaim::New {
            lease_token: "test-lease".to_owned(),
        }
    } else {
        CheckoutAttemptClaim::InProgress
    }
}

pub(crate) fn checkout_evaluation_context(input: &CheckoutInput) -> EvaluationContext {
    EvaluationContext::default()
        .with_targeting_key(format!("{}:{}", input.tenant_id, input.customer_id))
        .with_custom_field("tenant_id", input.tenant_id.clone())
        .with_custom_field("customer_id", input.customer_id.clone())
        .with_custom_field("segment", input.segment.clone())
        .with_custom_field("tier", input.tier.clone())
}

pub(crate) fn checkout_variant_includes_recommendation(variant: &str) -> bool {
    variant != "control"
}

impl CheckoutInput {
    fn unbounded_fault_requested(&self) -> bool {
        self.retry > 3 || self.timeout_ms > 30_000
    }
}

pub(crate) fn validate_input(input: &CheckoutInput) -> ApiResult<()> {
    if !bounded_identifier(&input.tenant_id) || !bounded_identifier(&input.customer_id) {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_identity",
            "tenant_id and customer_id are required",
        ));
    }
    if input
        .session_id
        .as_deref()
        .is_some_and(|value| !bounded_identifier(value))
    {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_session",
            "session_id is invalid",
        ));
    }
    if input
        .cart_id
        .as_deref()
        .is_some_and(|value| !bounded_identifier(value))
    {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_cart",
            "cart_id is invalid",
        ));
    }
    if input
        .request_id
        .as_deref()
        .is_some_and(|value| !bounded_identifier(value))
    {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request_id",
            "request_id is invalid",
        ));
    }
    if input
        .promotion_code
        .as_deref()
        .is_some_and(|value| !bounded_identifier(value))
    {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_promotion",
            "promotion_code is invalid",
        ));
    }
    if input.currency_code.len() != 3
        || input.currency_code != input.currency_code.to_ascii_uppercase()
        || !input
            .currency_code
            .bytes()
            .all(|byte| byte.is_ascii_alphabetic())
    {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_currency",
            "currency_code must be three uppercase letters",
        ));
    }
    if input.items.is_empty() || input.items.len() > MAX_ITEMS {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_items",
            "checkout needs between one and fifty items",
        ));
    }
    let mut skus = HashSet::with_capacity(input.items.len());
    for item in &input.items {
        if !bounded_identifier(&item.sku) {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "invalid_sku",
                "each item needs a valid SKU",
            ));
        }
        if item.quantity == 0 || item.quantity > MAX_QUANTITY {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "invalid_quantity",
                "each item needs a quantity from one to one hundred",
            ));
        }
        if !skus.insert(item.sku.as_str()) {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "duplicate_sku",
                "each SKU may appear only once in a checkout",
            ));
        }
    }
    Ok(())
}

fn bounded_identifier(value: &str) -> bool {
    !value.trim().is_empty()
        && value.len() <= MAX_IDENTIFIER_LENGTH
        && !value.chars().any(char::is_control)
}

pub(crate) async fn validate_catalog(
    state: &AppState,
    context: &Context,
    input: &CheckoutInput,
) -> ApiResult<Vec<Value>> {
    let mut products = Vec::with_capacity(input.items.len());
    for item in &input.items {
        let request = GraphQlRequest {
            query: r#"query CheckoutProduct($sku: String!, $tenantId: ID!, $segment: String!) {
                product(sku: $sku, tenantId: $tenantId, segment: $segment) {
                    id tenantId sku name price { currency amountMinor }
                    variants { id sku name price { currency amountMinor } }
                }
            }"#,
            variables: json!({"sku": item.sku, "tenantId": input.tenant_id, "segment": input.segment}),
            operation_name: "CheckoutProduct",
        };
        let body = post_graphql(state, context, &request).await?;
        let product = body
            .pointer("/data/product")
            .filter(|value| !value.is_null())
            .ok_or_else(|| {
                ApiError::new(
                    StatusCode::NOT_FOUND,
                    "product_not_found",
                    format!("unknown catalog SKU: {}", item.sku),
                )
            })?;
        if product.get("sku").and_then(Value::as_str) != Some(item.sku.as_str()) {
            return Err(ApiError::new(
                StatusCode::NOT_FOUND,
                "product_not_found",
                format!("catalog did not return SKU: {}", item.sku),
            ));
        }
        products.push(product.clone());
    }
    Ok(products)
}

pub(crate) async fn post_graphql(
    state: &AppState,
    context: &Context,
    request: &GraphQlRequest,
) -> ApiResult<Value> {
    let mut headers = HeaderMap::new();
    playground_telemetry::inject_context_headers(context, &mut headers);
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    let response = state
        .http
        .post(&state.catalog_url)
        .headers(headers)
        .json(request)
        .send()
        .await
        .map_err(|error| {
            ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "catalog_unavailable",
                error.to_string(),
            )
        })?;
    let status = response.status();
    let body = response.json::<Value>().await.map_err(|error| {
        ApiError::new(
            StatusCode::BAD_GATEWAY,
            "catalog_protocol_error",
            error.to_string(),
        )
    })?;
    if !status.is_success() {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "catalog_unavailable",
            format!("catalog HTTP status {status}"),
        ));
    }
    let envelope: GraphQlEnvelope = serde_json::from_value(body.clone()).map_err(|_| {
        ApiError::new(
            StatusCode::BAD_GATEWAY,
            "catalog_protocol_error",
            "catalog returned an invalid GraphQL envelope",
        )
    })?;
    if let Some(errors) = envelope.errors.as_ref()
        && !errors.is_empty()
    {
        tracing::warn!(error_count = errors.len(), "catalog GraphQL query failed");
        return Err(ApiError::new(
            StatusCode::BAD_GATEWAY,
            "catalog_query_failed",
            "catalog GraphQL query failed",
        ));
    }
    Ok(body)
}

#[derive(Debug, Deserialize)]
struct GraphQlEnvelope {
    #[allow(dead_code)]
    data: Option<Value>,
    errors: Option<Vec<GraphQlError>>,
}

#[derive(Debug, Deserialize)]
struct GraphQlError {
    #[allow(dead_code)]
    message: String,
}

pub(crate) async fn quote_with_retry(
    state: &AppState,
    context: &Context,
    input: &CheckoutInput,
    request_id: &str,
    checkout_variant: &str,
) -> ApiResult<QuoteResponse> {
    let attempts = input.retry.min(3) + 1;
    let mut last = None;
    for attempt in 1..=attempts {
        match quote_once(state, context, input, request_id, checkout_variant).await {
            Ok(quote) => return Ok(quote),
            Err(error) => {
                tracing::warn!(attempt, error = %error.message, "pricing attempt failed");
                let retryable = matches!(
                    error.status,
                    StatusCode::SERVICE_UNAVAILABLE | StatusCode::GATEWAY_TIMEOUT
                );
                last = Some(error);
                if !retryable || attempt == attempts {
                    break;
                }
            }
        }
    }
    Err(last.unwrap_or_else(|| {
        ApiError::new(
            StatusCode::BAD_GATEWAY,
            "pricing_failed",
            "pricing returned no result",
        )
    }))
}

pub(crate) async fn quote_once(
    state: &AppState,
    context: &Context,
    input: &CheckoutInput,
    request_id: &str,
    checkout_variant: &str,
) -> ApiResult<QuoteResponse> {
    let mut pricing = PricingClient::new(pricing_channel(state).await?);
    let mut pricing_context = HashMap::from([
        ("pricing_strategy".to_owned(), "standard".to_owned()),
        ("checkout_variant".to_owned(), checkout_variant.to_owned()),
        ("customer_segment".to_owned(), input.segment.clone()),
        ("customer_tier".to_owned(), input.tier.clone()),
        ("region".to_owned(), input.region.clone()),
        ("request_priority".to_owned(), input.priority.clone()),
    ]);
    if let Some(code) = input
        .promotion_code
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        pricing_context.insert("promotion_code".to_owned(), code.to_owned());
        pricing_context.insert("pricing_strategy".to_owned(), "promotional".to_owned());
    }
    if input.delay_ms > 0 {
        pricing_context.insert(
            "delay_ms".to_owned(),
            input.delay_ms.min(30_000).to_string(),
        );
    }
    let request = QuoteRequest {
        request_id: request_id.to_owned(),
        tenant_id: input.tenant_id.clone(),
        customer_id: input.customer_id.clone(),
        items: input
            .items
            .iter()
            .map(|item| QuoteItem {
                sku: item.sku.clone(),
                quantity: item.quantity,
            })
            .collect(),
        currency_code: input.currency_code.clone(),
        context: pricing_context,
        payment_method_type: None,
    };
    let mut request = tonic::Request::new(request);
    request.set_timeout(Duration::from_millis(input.timeout_ms.clamp(1, 30_000)));
    playground_telemetry::inject_grpc_metadata_with_context(context, request.metadata_mut());
    let response = pricing
        .quote(request)
        .await
        .map_err(pricing_error)?
        .into_inner();
    validate_quote_response(input, &response)?;
    Ok(response)
}

pub(crate) fn validate_quote_response(
    input: &CheckoutInput,
    quote: &QuoteResponse,
) -> ApiResult<()> {
    if QuoteStatus::try_from(quote.status).ok() != Some(QuoteStatus::Ready) {
        return Err(ApiError::new(
            StatusCode::BAD_GATEWAY,
            "pricing_protocol_error",
            "pricing returned a quote that is not ready",
        ));
    }
    if quote.quote_id.trim().is_empty() || quote.pricing_version.trim().is_empty() {
        return Err(ApiError::new(
            StatusCode::BAD_GATEWAY,
            "pricing_protocol_error",
            "pricing returned an unidentified quote",
        ));
    }
    if quote.valid_for_seconds == 0 {
        return Err(ApiError::new(
            StatusCode::BAD_GATEWAY,
            "pricing_protocol_error",
            "pricing returned a quote without an expiration",
        ));
    }
    if quote.lines.len() != input.items.len() {
        return Err(ApiError::new(
            StatusCode::BAD_GATEWAY,
            "pricing_protocol_error",
            "pricing returned an incomplete quote",
        ));
    }

    let mut expected = HashMap::with_capacity(input.items.len());
    for item in &input.items {
        expected.insert(item.sku.as_str(), item.quantity);
    }
    let mut seen = HashSet::with_capacity(quote.lines.len());
    let mut subtotal_from_lines = 0_i64;
    for line in &quote.lines {
        let Some(expected_quantity) = expected.get(line.sku.as_str()) else {
            return Err(ApiError::new(
                StatusCode::BAD_GATEWAY,
                "pricing_protocol_error",
                "pricing returned an unexpected quote line",
            ));
        };
        if !seen.insert(line.sku.as_str()) || line.quantity != *expected_quantity {
            return Err(ApiError::new(
                StatusCode::BAD_GATEWAY,
                "pricing_protocol_error",
                "pricing returned duplicate or mismatched quote quantities",
            ));
        }
        let unit = quote_money(
            line.unit_price.as_ref(),
            &input.currency_code,
            "line unit price",
        )?;
        let line_total = quote_money(line.line_total.as_ref(), &input.currency_code, "line total")?;
        i32_amount(unit)?;
        i32_amount(line_total)?;
        let expected_total = unit.checked_mul(i64::from(line.quantity)).ok_or_else(|| {
            ApiError::new(
                StatusCode::BAD_GATEWAY,
                "pricing_protocol_error",
                "quote line arithmetic overflowed",
            )
        })?;
        if expected_total != line_total {
            return Err(ApiError::new(
                StatusCode::BAD_GATEWAY,
                "pricing_protocol_error",
                "quote line amount does not match unit price and quantity",
            ));
        }
        subtotal_from_lines = subtotal_from_lines.checked_add(line_total).ok_or_else(|| {
            ApiError::new(
                StatusCode::BAD_GATEWAY,
                "pricing_protocol_error",
                "quote subtotal arithmetic overflowed",
            )
        })?;
    }
    if seen.len() != expected.len() {
        return Err(ApiError::new(
            StatusCode::BAD_GATEWAY,
            "pricing_protocol_error",
            "pricing omitted a requested quote line",
        ));
    }

    let subtotal = quote_money(quote.subtotal.as_ref(), &input.currency_code, "subtotal")?;
    let discount = quote_money(
        quote.discount_total.as_ref(),
        &input.currency_code,
        "discount",
    )?;
    let tax = quote_money(quote.tax_total.as_ref(), &input.currency_code, "tax")?;
    let total = quote_money(
        quote.grand_total.as_ref(),
        &input.currency_code,
        "grand_total",
    )?;
    if subtotal != subtotal_from_lines || discount > subtotal {
        return Err(ApiError::new(
            StatusCode::BAD_GATEWAY,
            "pricing_protocol_error",
            "quote totals do not match its lines",
        ));
    }
    let expected_total = subtotal
        .checked_sub(discount)
        .and_then(|amount| amount.checked_add(tax))
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::BAD_GATEWAY,
                "pricing_protocol_error",
                "quote total arithmetic overflowed",
            )
        })?;
    if total != expected_total {
        return Err(ApiError::new(
            StatusCode::BAD_GATEWAY,
            "pricing_protocol_error",
            "quote totals do not balance",
        ));
    }
    i32_amount(subtotal)?;
    i32_amount(discount)?;
    i32_amount(tax)?;
    i32_amount(total)?;
    Ok(())
}

pub(crate) fn pricing_error(status: tonic::Status) -> ApiError {
    let (http_status, code) = match status.code() {
        Code::InvalidArgument => (StatusCode::BAD_REQUEST, "pricing_invalid_request"),
        Code::NotFound => (StatusCode::NOT_FOUND, "product_not_found"),
        Code::DeadlineExceeded => (StatusCode::GATEWAY_TIMEOUT, "pricing_timeout"),
        Code::Unavailable => (StatusCode::SERVICE_UNAVAILABLE, "pricing_unavailable"),
        _ => (StatusCode::BAD_GATEWAY, "pricing_failed"),
    };
    ApiError::new(http_status, code, status.message())
}

pub(crate) async fn create_pending_order(
    state: &AppState,
    input: &CheckoutInput,
    quote: &QuoteResponse,
    lease: &CheckoutLease,
) -> ApiResult<PendingOrder> {
    validate_input(input)?;
    validate_quote_response(input, quote)?;
    let session_id = input.session_id.clone().ok_or_else(|| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_session",
            "checkout session identity is required",
        )
    })?;
    let mut client = acquire_db(&state.pool).await?;
    let transaction = client.transaction().await.map_err(db_error)?;

    if let Some(order) = transaction
        .query_opt(
            "SELECT o.id, o.order_number, o.cart_id, o.customer_id, o.currency, o.subtotal_minor, o.discount_minor, o.tax_minor, o.total_minor, c.session_id, o.promotion_code FROM orders o LEFT JOIN carts c ON c.tenant_id=o.tenant_id AND c.id=o.cart_id WHERE o.tenant_id = $1 AND o.checkout_request_id = $2 FOR UPDATE OF o",
            &[&input.tenant_id, &lease.request_id],
        )
        .await
        .map_err(db_error)?
    {
        let stored_customer: Option<String> = order.get(3);
        let stored_currency: String = order.get(4);
        let stored_session: Option<String> = order.get(9);
        if stored_customer.as_deref() != Some(input.customer_id.as_str())
            || stored_currency != input.currency_code
            || stored_session.as_deref() != Some(session_id.as_str())
        {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "checkout_request_conflict",
                "checkout request is linked to a different order identity",
            ));
        }
        let item_rows = transaction
            .query(
                "SELECT product_id, variant_id, sku, product_name, quantity, unit_price_minor FROM order_items WHERE tenant_id = $1 AND order_id = $2 ORDER BY created_at",
                &[&input.tenant_id, &order.get::<_, String>(0)],
            )
            .await
            .map_err(db_error)?;
        let mut lines = Vec::with_capacity(item_rows.len());
        for row in item_rows {
            let quantity: i32 = row.get(4);
            let unit_price_minor: i32 = row.get(5);
            lines.push(OrderLine {
                product_id: row.get(0),
                variant_id: row.get(1),
                sku: row.get(2),
                product_name: row.get(3),
                quantity: u32::try_from(quantity).map_err(|_| {
                    ApiError::new(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "checkout_idempotency_corrupt",
                        "stored order item quantity is invalid",
                    )
                })?,
                unit_price_minor: i64::from(unit_price_minor),
            });
        }
        let order_id: String = order.get(0);
        let linked = transaction
            .execute(
                "UPDATE checkout_attempts SET order_id = $3, updated_at = CURRENT_TIMESTAMP WHERE tenant_id = $1 AND request_id = $2 AND lease_token = $4 AND status = 'started' AND (order_id IS NULL OR order_id = $3)",
                &[
                    &input.tenant_id,
                    &lease.request_id,
                    &order_id,
                    &lease.token,
                ],
            )
            .await
            .map_err(db_error)?;
        if linked != 1 {
            return Err(ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "checkout_idempotency_corrupt",
                "checkout attempt is not linked to its durable order",
            ));
        }
        transaction.commit().await.map_err(db_error)?;
        return Ok(PendingOrder {
            id: order_id,
            order_number: order.get(1),
            cart_id: order.get::<_, Option<String>>(2).ok_or_else(|| {
                ApiError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "checkout_idempotency_corrupt",
                    "stored order has no cart identity",
                )
            })?,
            tenant_id: input.tenant_id.clone(),
            customer_id: input.customer_id.clone(),
            session_id,
            currency: stored_currency,
            lines,
            subtotal_minor: i64::from(order.get::<_, i32>(5)),
            discount_minor: i64::from(order.get::<_, i32>(6)),
            tax_minor: i64::from(order.get::<_, i32>(7)),
            total_minor: i64::from(order.get::<_, i32>(8)),
            promotion_code: order.get(10),
        });
    }

    let subtotal = quote_money(quote.subtotal.as_ref(), &input.currency_code, "subtotal")?;
    let discount = quote_money(
        quote.discount_total.as_ref(),
        &input.currency_code,
        "discount",
    )?;
    let total = quote_money(
        quote.grand_total.as_ref(),
        &input.currency_code,
        "grand_total",
    )?;
    let tax = quote_money(quote.tax_total.as_ref(), &input.currency_code, "tax")?;
    let expected_total = subtotal
        .checked_sub(discount)
        .and_then(|amount| amount.checked_add(tax))
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::BAD_GATEWAY,
                "pricing_protocol_error",
                "quote totals do not balance",
            )
        })?;
    if total != expected_total {
        return Err(ApiError::new(
            StatusCode::BAD_GATEWAY,
            "pricing_protocol_error",
            "quote totals do not balance",
        ));
    }
    let cart_id = input
        .cart_id
        .clone()
        .unwrap_or_else(|| format!("cart-{}", uuid::Uuid::new_v4()));
    transaction
        .execute(
            "INSERT INTO carts (id, tenant_id, customer_id, session_id, status, currency, expires_at) VALUES ($1,$2,$3,$4,'active',$5,CURRENT_TIMESTAMP + INTERVAL '7 days') ON CONFLICT DO NOTHING",
            &[&cart_id, &input.tenant_id, &input.customer_id, &session_id, &input.currency_code],
        )
        .await
        .map_err(db_error)?;

    let cart = transaction
        .query_opt(
            "SELECT customer_id, status, currency, session_id FROM carts WHERE tenant_id=$1 AND id=$2 FOR UPDATE",
            &[&input.tenant_id, &cart_id],
        )
        .await
        .map_err(db_error)?
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "cart_persistence_failed",
                "checkout cart was not persisted",
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
            StatusCode::CONFLICT,
            "cart_scope_conflict",
            "cart does not belong to the checkout customer or is not active",
        ));
    }

    let mut lines = Vec::with_capacity(input.items.len());
    for item in &input.items {
        let row = transaction
            .query_opt(
                "SELECT p.id, pv.id, p.name, pv.name FROM product_variants pv JOIN products p ON p.tenant_id = pv.tenant_id AND p.id = pv.product_id WHERE pv.tenant_id = $1 AND pv.sku = $2 AND pv.status = 'active' AND p.status = 'active'",
                &[&input.tenant_id, &item.sku],
            )
            .await
            .map_err(db_error)?
            .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "product_not_found", format!("unknown SKU: {}", item.sku)))?;
        let quote_line = quote
            .lines
            .iter()
            .find(|line| line.sku == item.sku)
            .ok_or_else(|| {
                ApiError::new(
                    StatusCode::BAD_GATEWAY,
                    "pricing_protocol_error",
                    format!("missing quote line: {}", item.sku),
                )
            })?;
        let unit_price_minor = quote_line
            .unit_price
            .as_ref()
            .map(|money| money.amount_minor)
            .ok_or_else(|| {
                ApiError::new(
                    StatusCode::BAD_GATEWAY,
                    "pricing_protocol_error",
                    "quote line has no unit price",
                )
            })?;
        i32_amount(unit_price_minor)?;
        lines.push(OrderLine {
            product_id: row.get(0),
            variant_id: row.get(1),
            sku: item.sku.clone(),
            product_name: format!("{} / {}", row.get::<_, String>(2), row.get::<_, String>(3)),
            quantity: item.quantity,
            unit_price_minor,
        });
    }

    for line in &lines {
        transaction
            .execute(
                "INSERT INTO cart_items (id, tenant_id, cart_id, variant_id, quantity, unit_price_minor, discount_minor) VALUES ($1,$2,$3,$4,$5,$6,0) ON CONFLICT (tenant_id, cart_id, variant_id) DO UPDATE SET quantity=EXCLUDED.quantity, unit_price_minor=EXCLUDED.unit_price_minor",
                &[&format!("cart-item-{}", uuid::Uuid::new_v4()), &input.tenant_id, &cart_id, &line.variant_id, &i32::try_from(line.quantity).map_err(|_| ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, "quantity_out_of_range", "quantity exceeds PostgreSQL integer range"))?, &i32_amount(line.unit_price_minor)?],
            )
            .await
            .map_err(db_error)?;
    }

    let order_id = format!("order-{}", uuid::Uuid::new_v4());
    let order_number = format!(
        "{}-{}",
        input.tenant_id.replace("tenant-", ""),
        &order_id[6..]
    );
    let subtotal_i32 = i32_amount(subtotal)?;
    let discount_i32 = i32_amount(discount)?;
    let tax_i32 = i32_amount(tax)?;
    let total_i32 = i32_amount(total)?;
    transaction
        .execute(
            "INSERT INTO orders (id, tenant_id, customer_id, cart_id, order_number, status, currency, subtotal_minor, discount_minor, tax_minor, shipping_minor, total_minor, shipping_address, billing_address, checkout_request_id, promotion_code) VALUES ($1,$2,$3,$4,$5,'pending',$6,$7,$8,$9,0,$10,'{}'::jsonb,'{}'::jsonb,$11,$12)",
            &[
                &order_id,
                &input.tenant_id,
                &input.customer_id,
                &cart_id,
                &order_number,
                &input.currency_code,
                &subtotal_i32,
                &discount_i32,
                &tax_i32,
                &total_i32,
                &lease.request_id,
                &input.promotion_code,
            ],
        )
        .await
        .map_err(db_error)?;
    for line in &lines {
        transaction
            .execute(
                "INSERT INTO order_items (id, tenant_id, order_id, product_id, variant_id, sku, product_name, quantity, unit_price_minor, discount_minor) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,0)",
                &[&format!("order-item-{}", uuid::Uuid::new_v4()), &input.tenant_id, &order_id, &line.product_id, &line.variant_id, &line.sku, &line.product_name, &i32::try_from(line.quantity).map_err(|_| ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, "quantity_out_of_range", "quantity exceeds PostgreSQL integer range"))?, &i32_amount(line.unit_price_minor)?],
            )
            .await
            .map_err(db_error)?;
    }
    let linked = transaction
        .execute(
            "UPDATE checkout_attempts SET order_id = $3, updated_at = CURRENT_TIMESTAMP WHERE tenant_id = $1 AND request_id = $2 AND lease_token = $4 AND status = 'started' AND (order_id IS NULL OR order_id = $3)",
            &[
                &input.tenant_id,
                &lease.request_id,
                &order_id,
                &lease.token,
            ],
        )
        .await
        .map_err(db_error)?;
    if linked != 1 {
        return Err(ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "checkout_idempotency_corrupt",
            "checkout attempt could not be linked to its durable order",
        ));
    }
    transaction.commit().await.map_err(db_error)?;
    Ok(PendingOrder {
        id: order_id,
        order_number,
        cart_id,
        tenant_id: input.tenant_id.clone(),
        customer_id: input.customer_id.clone(),
        session_id,
        currency: input.currency_code.clone(),
        lines,
        subtotal_minor: subtotal,
        discount_minor: discount,
        tax_minor: tax,
        total_minor: total,
        promotion_code: input.promotion_code.clone(),
    })
}

pub(crate) fn quote_money(
    money: Option<&QuoteMoney>,
    currency: &str,
    field: &str,
) -> ApiResult<i64> {
    let money = money.ok_or_else(|| {
        ApiError::new(
            StatusCode::BAD_GATEWAY,
            "pricing_protocol_error",
            format!("quote has no {field}"),
        )
    })?;
    if money.currency_code != currency {
        return Err(ApiError::new(
            StatusCode::BAD_GATEWAY,
            "pricing_protocol_error",
            format!("quote {field} currency mismatch"),
        ));
    }
    if money.amount_minor < 0 {
        return Err(ApiError::new(
            StatusCode::BAD_GATEWAY,
            "pricing_protocol_error",
            format!("quote {field} is negative"),
        ));
    }
    Ok(money.amount_minor)
}

pub(crate) fn i32_amount(amount: i64) -> ApiResult<i32> {
    i32::try_from(amount).map_err(|_| {
        ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "amount_out_of_range",
            "amount exceeds PostgreSQL integer range",
        )
    })
}

pub(crate) async fn authorize_payment(
    state: &AppState,
    context: &Context,
    order: &PendingOrder,
    request_id: &str,
    token: &str,
    method: &str,
    timeout_ms: u64,
) -> ApiResult<PaymentAuthorization> {
    let mut client = PaymentClient::new(payment_channel(state).await?);
    let request = AuthorizeRequest {
        request_id: format!("{request_id}:authorize"),
        merchant_reference: order.id.clone(),
        amount: Some(PaymentMoney {
            currency_code: order.currency.clone(),
            amount_minor: order.total_minor,
        }),
        payment_method: Some(PaymentMethod {
            r#type: payment_method_type(method) as i32,
            token: token.to_owned(),
        }),
        tenant_id: order.tenant_id.clone(),
    };
    let mut request = tonic::Request::new(request);
    request.set_timeout(Duration::from_millis(timeout_ms.clamp(1, 30_000)));
    playground_telemetry::inject_grpc_metadata_with_context(context, request.metadata_mut());
    let response = client
        .authorize(request)
        .await
        .map_err(|status| payment_error(status, "payment authorization"))?
        .into_inner();
    let operation_status = payment_operation_status(response.operation_status);
    let failure_reason = payment_failure_reason(response.failure_reason);
    let payment_id = response
        .payment
        .map(|payment| payment.payment_id)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::BAD_GATEWAY,
                "payment_protocol_error",
                format!(
                    "payment authorization returned {} without payment_id",
                    operation_status.as_str_name()
                ),
            )
        })?;
    match operation_status {
        PaymentOperationStatus::Succeeded | PaymentOperationStatus::Pending => {
            Ok(PaymentAuthorization {
                payment_id,
                operation_status,
                failure_reason,
            })
        }
        PaymentOperationStatus::Declined | PaymentOperationStatus::Failed => Err(
            payment_operation_error("payment authorization", operation_status, failure_reason),
        ),
        PaymentOperationStatus::Unspecified => Err(ApiError::new(
            StatusCode::BAD_GATEWAY,
            "payment_protocol_error",
            "payment authorization returned an unspecified operation status",
        )),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn recover_authorization(
    state: &AppState,
    context: &Context,
    order: &PendingOrder,
    request_id: &str,
    token: &str,
    method: &str,
    timeout_ms: u64,
    fence: &CheckoutFence,
) -> ApiResult<AuthorizationRecovery> {
    // The payment service persists the authorize response and its request
    // ledger in one transaction. Reusing this exact operation id either
    // replays that durable response or safely retries an operation that did
    // not commit.
    let retry_error =
        match authorize_payment(state, context, order, request_id, token, method, timeout_ms).await
        {
            Ok(authorization) => return Ok(AuthorizationRecovery::Recovered(authorization)),
            Err(error) => error,
        };
    if !retry_error.is_ambiguous_payment_failure() {
        return Ok(AuthorizationRecovery::Failed(retry_error));
    }
    tracing::warn!(
        order_id = %order.id,
        error = %retry_error.message,
        "payment authorization retry remains ambiguous; checking durable payment state"
    );

    let Some(payment_id) = lookup_authorization_payment_id(state, order, request_id).await? else {
        // Absence is not proof of non-creation while the original payment
        // transaction may still be committing. Do not cancel this order.
        return Ok(AuthorizationRecovery::Pending);
    };
    let snapshot = get_payment_snapshot(state, context, order, &payment_id, timeout_ms).await?;
    if snapshot.authorized_minor != order.total_minor || snapshot.currency != order.currency {
        return Err(payment_reconciliation_error(
            "payment authorization does not match the order",
        ));
    }

    if snapshot.captured_minor > snapshot.refunded_minor {
        enqueue_payment_compensation_task(state, order, &payment_id, request_id, context, fence)
            .await?;
        if compensate_payment_with_fence(
            state,
            context,
            order,
            &payment_id,
            request_id,
            timeout_ms,
            fence,
        )
        .await?
        {
            resolve_compensation_intent(
                state,
                order,
                &payment_compensation_task_key(&payment_id),
                fence,
            )
            .await?;
            return Ok(AuthorizationRecovery::Absent);
        }
        return Err(payment_reconciliation_error(
            "captured funds remain after ambiguous authorization",
        ));
    }

    match snapshot.status {
        PaymentStatus::Pending => Ok(AuthorizationRecovery::Recovered(PaymentAuthorization {
            payment_id,
            operation_status: PaymentOperationStatus::Pending,
            failure_reason: snapshot.failure_reason,
        })),
        PaymentStatus::Authorized => Ok(AuthorizationRecovery::Recovered(PaymentAuthorization {
            payment_id,
            operation_status: PaymentOperationStatus::Succeeded,
            failure_reason: snapshot.failure_reason,
        })),
        PaymentStatus::Voided | PaymentStatus::Refunded => Ok(AuthorizationRecovery::Absent),
        PaymentStatus::Failed => Ok(AuthorizationRecovery::Failed(payment_operation_error(
            "payment authorization",
            authorization_failure_status(snapshot.failure_reason),
            snapshot.failure_reason,
        ))),
        PaymentStatus::Captured | PaymentStatus::PartiallyRefunded => Err(
            payment_reconciliation_error("payment state has no compensable captured balance"),
        ),
        PaymentStatus::Unspecified => {
            Err(payment_reconciliation_error("payment state is unspecified"))
        }
    }
}

pub(crate) async fn lookup_authorization_payment_id(
    state: &AppState,
    order: &PendingOrder,
    request_id: &str,
) -> ApiResult<Option<String>> {
    let client = acquire_db(&state.pool).await?;
    let authorize_request_id = format!("{request_id}:authorize");
    let payment_id = client
        .query_opt(
            "SELECT p.id FROM payment_operation_requests r JOIN payments p ON p.tenant_id=r.tenant_id AND p.id=r.payment_id WHERE r.tenant_id=$1 AND r.request_id=$2 AND r.operation='authorize' AND p.order_id=$3 LIMIT 1",
            &[&order.tenant_id, &authorize_request_id, &order.id],
        )
        .await
        .map_err(db_error)?
        .map(|row| row.get::<_, String>(0));
    if payment_id.is_some() {
        return Ok(payment_id);
    }

    client
        .query_opt(
            "SELECT id FROM payments WHERE tenant_id=$1 AND order_id=$2 ORDER BY created_at DESC LIMIT 1",
            &[&order.tenant_id, &order.id],
        )
        .await
        .map_err(db_error)
        .map(|row| row.map(|value| value.get::<_, String>(0)))
}

pub(crate) async fn get_payment_snapshot(
    state: &AppState,
    context: &Context,
    order: &PendingOrder,
    payment_id: &str,
    timeout_ms: u64,
) -> ApiResult<PaymentSnapshot> {
    let mut client = PaymentClient::new(payment_channel(state).await?);
    let request = GetPaymentRequest {
        payment_id: payment_id.to_owned(),
        tenant_id: order.tenant_id.clone(),
    };
    let mut request = tonic::Request::new(request);
    request.set_timeout(Duration::from_millis(timeout_ms.clamp(1, 30_000)));
    playground_telemetry::inject_grpc_metadata_with_context(context, request.metadata_mut());
    let response = client
        .get_payment(request)
        .await
        .map_err(|status| payment_error(status, "payment reconciliation lookup"))?
        .into_inner();
    let response_status = payment_status(response.status);
    let payment = response.payment.ok_or_else(|| {
        payment_reconciliation_error("payment reconciliation returned no payment record")
    })?;
    let status = payment_status(payment.status);
    if status == PaymentStatus::Unspecified || response_status != status {
        return Err(payment_reconciliation_error(
            "payment reconciliation returned inconsistent status",
        ));
    }
    if payment.payment_id != payment_id || payment.merchant_reference != order.id {
        return Err(payment_reconciliation_error(
            "payment reconciliation returned the wrong payment resource",
        ));
    }
    let authorized = payment.authorized_amount.as_ref().ok_or_else(|| {
        payment_reconciliation_error("payment reconciliation returned no authorized amount")
    })?;
    let captured = payment.captured_amount.as_ref().ok_or_else(|| {
        payment_reconciliation_error("payment reconciliation returned no captured amount")
    })?;
    let refunded = payment.refunded_amount.as_ref().ok_or_else(|| {
        payment_reconciliation_error("payment reconciliation returned no refunded amount")
    })?;
    if authorized.currency_code != order.currency
        || authorized.amount_minor != order.total_minor
        || captured.currency_code != authorized.currency_code
        || refunded.currency_code != authorized.currency_code
        || authorized.amount_minor < 0
        || captured.amount_minor < 0
        || refunded.amount_minor < 0
        || captured.amount_minor > authorized.amount_minor
        || refunded.amount_minor > captured.amount_minor
    {
        return Err(payment_reconciliation_error(
            "payment reconciliation returned invalid amounts",
        ));
    }
    Ok(PaymentSnapshot {
        authorized_minor: authorized.amount_minor,
        currency: authorized.currency_code.clone(),
        status,
        failure_reason: payment_failure_reason(payment.failure_reason),
        captured_minor: captured.amount_minor,
        refunded_minor: refunded.amount_minor,
    })
}

pub(crate) async fn reconcile_pending_payment(
    state: &AppState,
    job: &PaymentReconciliationJob,
) -> ApiResult<PendingPaymentOutcome> {
    if job.status != "processing"
        || job.authorize_request_id != format!("{}:authorize", job.request_id)
        || !matches!(
            job.method_type.as_str(),
            "card" | "bank_account" | "wallet" | "unspecified"
        )
    {
        return Ok(PendingPaymentOutcome::Failed(payment_reconciliation_error(
            "payment reconciliation job identity is invalid",
        )));
    }
    let fence = CheckoutFence::PaymentReconciliation {
        tenant_id: job.tenant_id.clone(),
        request_id: job.request_id.clone(),
        parent_token: job.parent_lease_token.clone(),
        worker_token: job.lease_token.clone(),
    };
    assert_checkout_fence(state, &fence).await?;
    let order = load_pending_order(state, &job.tenant_id, &job.order_id).await?;
    if order.currency != job.currency
        || order.total_minor != job.amount_minor
        || job.merchant_reference != order.id
    {
        return Ok(PendingPaymentOutcome::Failed(payment_reconciliation_error(
            "payment reconciliation job does not match its order",
        )));
    }
    let context = stored_reconciliation_context(job)?;
    let timeout_ms = DB_TIMEOUT.as_millis() as u64;
    let payment_id = match job.payment_id.clone() {
        Some(payment_id) => Some(payment_id),
        None => lookup_authorization_payment_id(state, &order, &job.request_id).await?,
    };
    let Some(payment_id) = payment_id else {
        // The provider operation ledger is the source of truth for a payment
        // whose authorize response was lost. Keep this job retryable until the
        // payment worker exposes the payment id or terminal operation state.
        return Ok(PendingPaymentOutcome::Retry);
    };
    if job.payment_id.as_deref() != Some(payment_id.as_str()) {
        attach_payment_to_reconciliation(state, job, &payment_id).await?;
    }

    let mut snapshot = with_checkout_fence(state, &fence, || async {
        get_payment_snapshot(state, &context, &order, &payment_id, timeout_ms).await
    })
    .await?;
    record_payment_reconciliation_observation(
        state,
        job,
        snapshot.status.as_str_name(),
        reconciliation_operation_status(snapshot.status).as_str_name(),
        snapshot.failure_reason.as_str_name(),
    )
    .await?;
    match snapshot.status {
        PaymentStatus::Pending => return Ok(PendingPaymentOutcome::Retry),
        PaymentStatus::Failed => {
            return Ok(PendingPaymentOutcome::Failed(payment_operation_error(
                "payment authorization",
                authorization_failure_status(snapshot.failure_reason),
                snapshot.failure_reason,
            )));
        }
        PaymentStatus::Voided | PaymentStatus::Refunded => {
            return Ok(PendingPaymentOutcome::Failed(payment_reconciliation_error(
                "payment authorization was voided or refunded before checkout completed",
            )));
        }
        PaymentStatus::PartiallyRefunded => {
            return Ok(PendingPaymentOutcome::Failed(payment_reconciliation_error(
                "payment authorization has a partial refund and cannot complete checkout",
            )));
        }
        PaymentStatus::Authorized | PaymentStatus::Captured => {}
        PaymentStatus::Unspecified => {
            return Err(payment_reconciliation_error(
                "payment reconciliation returned an unspecified state",
            ));
        }
    }

    if snapshot.status == PaymentStatus::Authorized && snapshot.captured_minor < order.total_minor {
        with_checkout_fence(state, &fence, || async {
            capture_payment(
                state,
                &context,
                &order,
                &payment_id,
                &job.request_id,
                timeout_ms,
            )
            .await
        })
        .await?;
        snapshot = with_checkout_fence(state, &fence, || async {
            get_payment_snapshot(state, &context, &order, &payment_id, timeout_ms).await
        })
        .await?;
        record_payment_reconciliation_observation(
            state,
            job,
            snapshot.status.as_str_name(),
            reconciliation_operation_status(snapshot.status).as_str_name(),
            snapshot.failure_reason.as_str_name(),
        )
        .await?;
        if snapshot.status == PaymentStatus::Pending || snapshot.status == PaymentStatus::Authorized
        {
            return Ok(PendingPaymentOutcome::Retry);
        }
    }

    if snapshot.status != PaymentStatus::Captured
        || snapshot.captured_minor != order.total_minor
        || snapshot.refunded_minor != 0
    {
        return Ok(PendingPaymentOutcome::Failed(payment_reconciliation_error(
            "payment capture did not settle the exact checkout amount",
        )));
    }

    let reservations = match reserve_inventory(state, &context, &order, &fence, timeout_ms).await {
        Ok(reservations) => reservations,
        Err(error) => {
            if error.code == "checkout_lease_lost" {
                return Err(error);
            }
            return reconcile_captured_payment_failure(
                state,
                &context,
                &order,
                &payment_id,
                &job.request_id,
                &fence,
                timeout_ms,
                error,
            )
            .await;
        }
    };

    if let Err(error) =
        consume_inventory(state, &context, &order, &reservations, &fence, timeout_ms).await
    {
        if error.code == "checkout_lease_lost" {
            return Err(error);
        }
        let _ = release_inventory_with_recovery(
            state,
            &context,
            &order,
            &reservations,
            &fence,
            timeout_ms,
        )
        .await;
        return reconcile_captured_payment_failure(
            state,
            &context,
            &order,
            &payment_id,
            &job.request_id,
            &fence,
            timeout_ms,
            error,
        )
        .await;
    }

    if let Err(error) = finalize_order(
        state,
        &context,
        &order,
        &payment_id,
        &job.request_id,
        &job.feature_variant,
        &fence,
    )
    .await
    {
        if error.code == "checkout_lease_lost" {
            return Err(error);
        }
        let committed = match finalization_committed(state, &order).await {
            Ok(committed) => committed,
            Err(reconciliation_error)
                if reconciliation_error.code == "finalization_reconciliation_pending" =>
            {
                false
            }
            Err(reconciliation_error) => return Err(reconciliation_error),
        };
        if committed {
            return Ok(PendingPaymentOutcome::Completed(
                paid_reconciliation_payload(state, job, &order, &payment_id).await?,
            ));
        }
        // Inventory is already consumed. A finalization retry is safe and
        // must happen before compensation; do not try to release consumed
        // reservations here.
        return Err(error);
    }

    Ok(PendingPaymentOutcome::Completed(
        paid_reconciliation_payload(state, job, &order, &payment_id).await?,
    ))
}

#[allow(clippy::too_many_arguments)]
async fn reconcile_captured_payment_failure(
    state: &AppState,
    context: &Context,
    order: &PendingOrder,
    payment_id: &str,
    request_id: &str,
    fence: &CheckoutFence,
    timeout_ms: u64,
    error: ApiError,
) -> ApiResult<PendingPaymentOutcome> {
    enqueue_payment_compensation_task(state, order, payment_id, request_id, context, fence).await?;
    if !compensate_payment_with_fence(
        state, context, order, payment_id, request_id, timeout_ms, fence,
    )
    .await?
    {
        return Err(payment_reconciliation_error(
            "captured payment compensation is not yet confirmed",
        ));
    }
    resolve_compensation_intent(
        state,
        order,
        &payment_compensation_task_key(payment_id),
        fence,
    )
    .await?;
    cancel_order_if_compensated(state, order, fence).await?;
    Ok(PendingPaymentOutcome::Failed(error))
}

fn reconciliation_operation_status(status: PaymentStatus) -> PaymentOperationStatus {
    match status {
        PaymentStatus::Pending => PaymentOperationStatus::Pending,
        PaymentStatus::Authorized | PaymentStatus::Captured => PaymentOperationStatus::Succeeded,
        PaymentStatus::Failed
        | PaymentStatus::Voided
        | PaymentStatus::PartiallyRefunded
        | PaymentStatus::Refunded => PaymentOperationStatus::Failed,
        PaymentStatus::Unspecified => PaymentOperationStatus::Unspecified,
    }
}

async fn paid_reconciliation_payload(
    state: &AppState,
    job: &PaymentReconciliationJob,
    order: &PendingOrder,
    payment_id: &str,
) -> ApiResult<Value> {
    let client = acquire_db(&state.pool).await?;
    let mut payload = client
        .query_opt(
            "SELECT response_payload::text FROM checkout_attempts WHERE tenant_id=$1 AND request_id=$2 AND order_id=$3",
            &[&job.tenant_id, &job.request_id, &order.id],
        )
        .await
        .map_err(db_error)?
        .and_then(|row| row.get::<_, Option<String>>(0))
        .and_then(|value| serde_json::from_str::<Value>(&value).ok())
        .unwrap_or_else(|| {
            json!({
                "order_id": order.id,
                "order_number": order.order_number,
                "tenant_id": order.tenant_id,
                "customer_id": order.customer_id,
                "session_id": order.session_id,
                "currency": order.currency,
                "total_minor": order.total_minor,
                "items": order.lines.iter().map(|line| json!({"sku": line.sku, "quantity": line.quantity, "unit_price_minor": line.unit_price_minor})).collect::<Vec<_>>()
            })
        });
    if let Value::Object(fields) = &mut payload {
        fields.insert("status".to_owned(), json!("paid"));
        fields.insert("order_status".to_owned(), json!("paid"));
        fields.insert("payment_id".to_owned(), json!(payment_id));
        fields.insert("payment_status".to_owned(), json!("captured"));
        fields.insert(
            "payment_operation_status".to_owned(),
            json!(PaymentOperationStatus::Succeeded.as_str_name()),
        );
        fields.insert(
            "payment_failure_reason".to_owned(),
            json!(PaymentFailureReason::Unspecified.as_str_name()),
        );
        fields.insert("feature_variant".to_owned(), json!(job.feature_variant));
        fields.insert("event_key".to_owned(), json!(format!("{}:paid", order.id)));
    }
    Ok(payload)
}

pub(crate) fn payment_reconciliation_error(message: impl Into<String>) -> ApiError {
    ApiError::new(
        StatusCode::BAD_GATEWAY,
        "payment_reconciliation_failed",
        message,
    )
}

pub(crate) fn authorization_failure_status(reason: PaymentFailureReason) -> PaymentOperationStatus {
    match reason {
        PaymentFailureReason::Declined
        | PaymentFailureReason::InsufficientFunds
        | PaymentFailureReason::InvalidPaymentMethod => PaymentOperationStatus::Declined,
        _ => PaymentOperationStatus::Failed,
    }
}

pub(crate) async fn capture_payment(
    state: &AppState,
    context: &Context,
    order: &PendingOrder,
    payment_id: &str,
    request_id: &str,
    timeout_ms: u64,
) -> ApiResult<()> {
    let mut client = PaymentClient::new(payment_channel(state).await?);
    let request = CaptureRequest {
        request_id: format!("{request_id}:capture"),
        payment_id: payment_id.to_owned(),
        amount: Some(PaymentMoney {
            currency_code: order.currency.clone(),
            amount_minor: order.total_minor,
        }),
        tenant_id: order.tenant_id.clone(),
    };
    let mut request = tonic::Request::new(request);
    request.set_timeout(Duration::from_millis(timeout_ms.clamp(1, 30_000)));
    playground_telemetry::inject_grpc_metadata_with_context(context, request.metadata_mut());
    let response = client
        .capture(request)
        .await
        .map_err(|status| payment_error(status, "payment capture"))?
        .into_inner();
    let operation_status = payment_operation_status(response.operation_status);
    if operation_status != PaymentOperationStatus::Succeeded {
        return Err(payment_operation_error(
            "payment capture",
            operation_status,
            payment_failure_reason(response.failure_reason),
        ));
    }
    Ok(())
}

pub(crate) async fn void_payment(
    state: &AppState,
    context: &Context,
    order: &PendingOrder,
    payment_id: &str,
    request_id: &str,
    timeout_ms: u64,
) -> ApiResult<()> {
    let mut client = PaymentClient::new(payment_channel(state).await?);
    let request = VoidRequest {
        request_id: format!("{request_id}:void"),
        payment_id: payment_id.to_owned(),
        reason: VoidReason::OrderCancelled as i32,
        tenant_id: order.tenant_id.clone(),
    };
    let mut request = tonic::Request::new(request);
    request.set_timeout(Duration::from_millis(timeout_ms.clamp(1, 30_000)));
    playground_telemetry::inject_grpc_metadata_with_context(context, request.metadata_mut());
    let response = client
        .void(request)
        .await
        .map_err(|status| payment_error(status, "payment void"))?
        .into_inner();
    let operation_status = payment_operation_status(response.operation_status);
    if operation_status != PaymentOperationStatus::Succeeded {
        return Err(payment_operation_error(
            "payment void",
            operation_status,
            payment_failure_reason(response.failure_reason),
        ));
    }
    Ok(())
}

pub(crate) async fn refund_payment(
    state: &AppState,
    context: &Context,
    order: &PendingOrder,
    payment_id: &str,
    request_id: &str,
    amount_minor: i64,
    timeout_ms: u64,
) -> ApiResult<()> {
    let mut client = PaymentClient::new(payment_channel(state).await?);
    let request = RefundRequest {
        request_id: format!("{request_id}:refund:{amount_minor}"),
        payment_id: payment_id.to_owned(),
        amount: Some(PaymentMoney {
            currency_code: order.currency.clone(),
            amount_minor,
        }),
        reason: RefundReason::OrderCancelled as i32,
        tenant_id: order.tenant_id.clone(),
    };
    let mut request = tonic::Request::new(request);
    request.set_timeout(Duration::from_millis(timeout_ms.clamp(1, 30_000)));
    playground_telemetry::inject_grpc_metadata_with_context(context, request.metadata_mut());
    let response = client
        .refund(request)
        .await
        .map_err(|status| payment_error(status, "payment refund"))?
        .into_inner();
    let operation_status = payment_operation_status(response.operation_status);
    if operation_status != PaymentOperationStatus::Succeeded {
        return Err(payment_operation_error(
            "payment refund",
            operation_status,
            payment_failure_reason(response.failure_reason),
        ));
    }
    Ok(())
}

pub(crate) fn payment_compensation_confirmed(snapshot: &PaymentSnapshot) -> bool {
    match snapshot.status {
        PaymentStatus::Voided | PaymentStatus::Failed => {
            snapshot.captured_minor == 0 && snapshot.refunded_minor == 0
        }
        PaymentStatus::PartiallyRefunded | PaymentStatus::Refunded => {
            snapshot.captured_minor == snapshot.refunded_minor
        }
        _ => false,
    }
}

pub(crate) async fn compensate_payment(
    state: &AppState,
    context: &Context,
    order: &PendingOrder,
    payment_id: &str,
    request_id: &str,
    timeout_ms: u64,
) -> bool {
    // Re-read before every financial action. A capture may have committed even
    // when its response was lost, and a void/refund response may be lost too.
    // The bounded retries reuse deterministic operation ids, so replay is safe.
    for attempt in 0..3 {
        let snapshot =
            match get_payment_snapshot(state, context, order, payment_id, timeout_ms).await {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    tracing::error!(
                        order_id = %order.id,
                        attempt,
                        error = %error.message,
                        "payment compensation state lookup failed"
                    );
                    return false;
                }
            };
        if payment_compensation_confirmed(&snapshot) {
            return true;
        }

        let remaining = snapshot.captured_minor - snapshot.refunded_minor;
        let action = if remaining > 0 {
            refund_payment(
                state, context, order, payment_id, request_id, remaining, timeout_ms,
            )
            .await
        } else if snapshot.status == PaymentStatus::Authorized {
            void_payment(state, context, order, payment_id, request_id, timeout_ms).await
        } else {
            return false;
        };
        if let Err(error) = action {
            tracing::warn!(
                order_id = %order.id,
                attempt,
                error = %error.message,
                "payment compensation operation returned an error; reconciling"
            );
        }

        let after = match get_payment_snapshot(state, context, order, payment_id, timeout_ms).await
        {
            Ok(snapshot) => snapshot,
            Err(error) => {
                tracing::error!(
                    order_id = %order.id,
                    attempt,
                    error = %error.message,
                    "payment compensation result could not be reconciled"
                );
                return false;
            }
        };
        if payment_compensation_confirmed(&after) {
            return true;
        }
    }
    false
}

pub(crate) async fn compensate_payment_with_fence(
    state: &AppState,
    context: &Context,
    order: &PendingOrder,
    payment_id: &str,
    request_id: &str,
    timeout_ms: u64,
    fence: &CheckoutFence,
) -> ApiResult<bool> {
    let task_key =
        generation_compensation_task_key(&payment_compensation_task_key(payment_id), fence.token());
    begin_compensation_operation(state, order, &task_key, fence).await?;
    let compensated = with_checkout_fence(state, fence, || async {
        Ok(compensate_payment(state, context, order, payment_id, request_id, timeout_ms).await)
    })
    .await?;
    if compensated {
        complete_compensation_operation(state, order, &task_key, fence).await?;
    }
    Ok(compensated)
}

pub(crate) fn payment_method_type(method: &str) -> PaymentMethodType {
    match method.trim().to_ascii_lowercase().as_str() {
        "bank_account" | "bank-account" => PaymentMethodType::BankAccount,
        "wallet" => PaymentMethodType::Wallet,
        _ => PaymentMethodType::Card,
    }
}

pub(crate) fn payment_operation_status(value: i32) -> PaymentOperationStatus {
    PaymentOperationStatus::try_from(value).unwrap_or(PaymentOperationStatus::Unspecified)
}

pub(crate) fn payment_status(value: i32) -> PaymentStatus {
    PaymentStatus::try_from(value).unwrap_or(PaymentStatus::Unspecified)
}

pub(crate) fn payment_failure_reason(value: i32) -> PaymentFailureReason {
    PaymentFailureReason::try_from(value).unwrap_or(PaymentFailureReason::Unspecified)
}

pub(crate) fn payment_operation_error(
    operation: &str,
    status: PaymentOperationStatus,
    reason: PaymentFailureReason,
) -> ApiError {
    let (http_status, code) = match reason {
        PaymentFailureReason::InsufficientFunds => {
            (StatusCode::PAYMENT_REQUIRED, "payment_insufficient_funds")
        }
        PaymentFailureReason::InvalidPaymentMethod => {
            (StatusCode::BAD_REQUEST, "payment_invalid_method")
        }
        PaymentFailureReason::ProviderUnavailable => (
            StatusCode::SERVICE_UNAVAILABLE,
            "payment_provider_unavailable",
        ),
        PaymentFailureReason::AlreadyProcessed => {
            (StatusCode::CONFLICT, "payment_already_processed")
        }
        PaymentFailureReason::InvalidState => (StatusCode::CONFLICT, "payment_invalid_state"),
        PaymentFailureReason::Declined => (StatusCode::PAYMENT_REQUIRED, "payment_declined"),
        PaymentFailureReason::Unspecified => match status {
            PaymentOperationStatus::Declined => (StatusCode::PAYMENT_REQUIRED, "payment_declined"),
            PaymentOperationStatus::Pending => (StatusCode::ACCEPTED, "payment_pending"),
            PaymentOperationStatus::Failed => (StatusCode::BAD_GATEWAY, "payment_failed"),
            PaymentOperationStatus::Succeeded | PaymentOperationStatus::Unspecified => {
                (StatusCode::BAD_GATEWAY, "payment_protocol_error")
            }
        },
    };
    ApiError::new(
        http_status,
        code,
        format!(
            "{operation}: {} ({})",
            status.as_str_name(),
            reason.as_str_name()
        ),
    )
}

pub(crate) fn payment_error(status: tonic::Status, operation: &str) -> ApiError {
    let reason = status
        .metadata()
        .get("payment-failure-reason")
        .and_then(|value| value.to_str().ok())
        .and_then(PaymentFailureReason::from_str_name)
        .unwrap_or_else(|| match status.code() {
            Code::Unavailable | Code::DeadlineExceeded => PaymentFailureReason::ProviderUnavailable,
            _ => PaymentFailureReason::Unspecified,
        });
    let code = match status.code() {
        Code::DeadlineExceeded => "payment_timeout",
        Code::Unavailable => "payment_provider_unavailable",
        Code::Internal => "payment_internal",
        Code::NotFound => "payment_not_found",
        Code::AlreadyExists => "payment_already_processed",
        Code::FailedPrecondition => "payment_invalid_state",
        Code::InvalidArgument => "payment_invalid_request",
        _ => "payment_failed",
    };
    let http_status = match status.code() {
        Code::InvalidArgument => StatusCode::BAD_REQUEST,
        Code::AlreadyExists | Code::FailedPrecondition => StatusCode::CONFLICT,
        Code::NotFound => StatusCode::NOT_FOUND,
        Code::DeadlineExceeded => StatusCode::GATEWAY_TIMEOUT,
        Code::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::BAD_GATEWAY,
    };
    ApiError::new(
        http_status,
        code,
        format!(
            "{operation}: {} ({})",
            status.message(),
            reason.as_str_name()
        ),
    )
}

pub(crate) async fn reserve_inventory(
    state: &AppState,
    context: &Context,
    order: &PendingOrder,
    fence: &CheckoutFence,
    timeout_ms: u64,
) -> ApiResult<Vec<InventoryReservation>> {
    let mut reservations = Vec::with_capacity(order.lines.len());
    for line in &order.lines {
        let reservation_id = format!("reservation-{}-{}", order.id, line.variant_id);
        let mut headers = HeaderMap::new();
        playground_telemetry::inject_context_headers(context, &mut headers);
        let response = match with_checkout_fence(state, fence, || async {
            state
                .http
                .post(format!(
                    "{}/reserve",
                    state.inventory_url.trim_end_matches('/')
                ))
                .headers(headers)
                .timeout(Duration::from_millis(timeout_ms.clamp(1, 30_000)))
                .json(&json!({
                    "tenant_id": order.tenant_id,
                    "reservation_id": reservation_id.clone(),
                    "sku": line.sku,
                    "quantity": line.quantity,
                    "checkout_request_id": fence.request_id(),
                    "checkout_lease_token": fence.token()
                }))
                .send()
                .await
                .map_err(|error| {
                    ApiError::new(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "inventory_unavailable",
                        error.to_string(),
                    )
                })
        })
        .await
        {
            Ok(response) => response,
            Err(error) => {
                reservations.push(InventoryReservation {
                    reservation_id,
                    sku: line.sku.clone(),
                    quantity: line.quantity,
                    location_id: None,
                });
                if let Err(compensation_error) = release_inventory_with_recovery(
                    state,
                    context,
                    order,
                    &reservations,
                    fence,
                    timeout_ms,
                )
                .await
                {
                    tracing::error!(
                        order_id = %order.id,
                        error = %compensation_error.message,
                        "inventory compensation failed after reservation transport error"
                    );
                    return Err(compensation_error);
                }
                return Err(ApiError::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "inventory_unavailable",
                    error.message,
                ));
            }
        };
        assert_checkout_fence(state, fence).await?;
        let status = response.status();
        if !status.is_success() {
            reservations.push(InventoryReservation {
                reservation_id,
                sku: line.sku.clone(),
                quantity: line.quantity,
                location_id: None,
            });
            if let Err(compensation_error) = release_inventory_with_recovery(
                state,
                context,
                order,
                &reservations,
                fence,
                timeout_ms,
            )
            .await
            {
                tracing::error!(
                    order_id = %order.id,
                    error = %compensation_error.message,
                    "inventory compensation failed after reservation rejection"
                );
                return Err(compensation_error);
            }
            return Err(ApiError::new(
                if status == StatusCode::CONFLICT {
                    StatusCode::CONFLICT
                } else {
                    StatusCode::SERVICE_UNAVAILABLE
                },
                "inventory_reservation_failed",
                format!("inventory returned {status} for {}", line.sku),
            ));
        }
        let body = match response.json::<Value>().await {
            Ok(body) => body,
            Err(error) => {
                reservations.push(InventoryReservation {
                    reservation_id,
                    sku: line.sku.clone(),
                    quantity: line.quantity,
                    location_id: None,
                });
                if let Err(compensation_error) = release_inventory_with_recovery(
                    state,
                    context,
                    order,
                    &reservations,
                    fence,
                    timeout_ms,
                )
                .await
                {
                    tracing::error!(
                        order_id = %order.id,
                        error = %compensation_error.message,
                        "inventory compensation failed after reservation protocol error"
                    );
                    return Err(compensation_error);
                }
                return Err(ApiError::new(
                    StatusCode::BAD_GATEWAY,
                    "inventory_protocol_error",
                    error.to_string(),
                ));
            }
        };
        let reservation = match parse_inventory_reservation(
            &body,
            order,
            &line.sku,
            line.quantity,
            &reservation_id,
        ) {
            Ok(reservation) => reservation,
            Err(error) => {
                // A successful provider response may have committed the hold
                // before returning malformed data. Keep the reservation key
                // so recovery can release it by tenant and reservation id.
                reservations.push(InventoryReservation {
                    reservation_id,
                    sku: line.sku.clone(),
                    quantity: line.quantity,
                    location_id: None,
                });
                if let Err(compensation_error) = release_inventory_with_recovery(
                    state,
                    context,
                    order,
                    &reservations,
                    fence,
                    timeout_ms,
                )
                .await
                {
                    tracing::error!(
                        order_id = %order.id,
                        error = %compensation_error.message,
                        "inventory compensation failed after inventory protocol error"
                    );
                    return Err(compensation_error);
                }
                return Err(error);
            }
        };
        reservations.push(reservation);
    }
    Ok(reservations)
}

#[derive(Debug, Deserialize)]
struct InventoryReserveResponse {
    tenant_id: String,
    reservation_id: String,
    sku: String,
    reserved: u32,
    location_id: String,
    remaining: i64,
    status: String,
}

#[derive(Debug, Deserialize)]
struct InventoryReleaseResponse {
    tenant_id: String,
    reservation_id: String,
    sku: String,
    location_id: String,
    requested: u32,
    released: i64,
    reserved_remaining: i64,
    available: i64,
    status: String,
}

fn validate_inventory_release_response(
    body: &Value,
    order: &PendingOrder,
    reservation: &InventoryReservation,
) -> ApiResult<()> {
    let response: InventoryReleaseResponse =
        serde_json::from_value(body.clone()).map_err(|_| {
            ApiError::new(
                StatusCode::BAD_GATEWAY,
                "inventory_protocol_error",
                "inventory returned an incomplete release payload",
            )
        })?;
    let release_matches = match response.status.as_str() {
        "released" => response.released == i64::from(reservation.quantity),
        "already_released" => response.released == 0,
        _ => false,
    };
    if response.tenant_id != order.tenant_id
        || response.reservation_id != reservation.reservation_id
        || response.sku != reservation.sku
        || response.location_id.trim().is_empty()
        || response.requested != reservation.quantity
        || !release_matches
        || response.reserved_remaining < 0
        || response.available < 0
    {
        return Err(ApiError::new(
            StatusCode::BAD_GATEWAY,
            "inventory_protocol_error",
            "inventory release payload does not match the request",
        ));
    }
    if let Some(expected_location) = reservation.location_id.as_deref()
        && expected_location != response.location_id
    {
        return Err(ApiError::new(
            StatusCode::BAD_GATEWAY,
            "inventory_protocol_error",
            "inventory release location does not match the reservation",
        ));
    }
    Ok(())
}

fn parse_inventory_reservation(
    body: &Value,
    order: &PendingOrder,
    sku: &str,
    quantity: u32,
    expected_reservation_id: &str,
) -> ApiResult<InventoryReservation> {
    let response: InventoryReserveResponse =
        serde_json::from_value(body.clone()).map_err(|_| {
            ApiError::new(
                StatusCode::BAD_GATEWAY,
                "inventory_protocol_error",
                "inventory returned an incomplete reservation payload",
            )
        })?;
    if response.tenant_id != order.tenant_id
        || response.reservation_id != expected_reservation_id
        || response.sku != sku
        || response.reserved != quantity
        || response.location_id.trim().is_empty()
        || response.remaining < 0
        || !matches!(
            response.status.as_str(),
            "reserved" | "already_reserved" | "already_consumed"
        )
    {
        return Err(ApiError::new(
            StatusCode::BAD_GATEWAY,
            "inventory_protocol_error",
            "inventory reservation payload does not match the request",
        ));
    }
    Ok(InventoryReservation {
        reservation_id: response.reservation_id,
        sku: response.sku,
        quantity: response.reserved,
        location_id: Some(response.location_id),
    })
}

#[derive(Debug, Deserialize)]
struct InventoryConsumeResponse {
    tenant_id: String,
    consumed: u32,
    reservation_count: usize,
    status: String,
}

const INVENTORY_CONSUME_ATTEMPTS: u8 = 3;

pub(crate) async fn consume_inventory(
    state: &AppState,
    context: &Context,
    order: &PendingOrder,
    reservations: &[InventoryReservation],
    fence: &CheckoutFence,
    timeout_ms: u64,
) -> ApiResult<()> {
    let payload = json!({
        "tenant_id": order.tenant_id,
        "checkout_request_id": fence.request_id(),
        "checkout_lease_token": fence.token(),
        "reservations": reservations.iter().map(|reservation| json!({
            "reservation_id": reservation.reservation_id,
            "sku": reservation.sku,
            "quantity": reservation.quantity,
            "location_id": reservation.location_id,
        })).collect::<Vec<_>>()
    });
    for attempt in 1..=INVENTORY_CONSUME_ATTEMPTS {
        match consume_inventory_attempt(
            state,
            context,
            order,
            reservations,
            fence,
            timeout_ms,
            &payload,
        )
        .await
        {
            Ok(()) => return Ok(()),
            Err(error)
                if attempt < INVENTORY_CONSUME_ATTEMPTS
                    && is_retryable_inventory_consume_error(&error) =>
            {
                tracing::warn!(
                    order_id = %order.id,
                    attempt,
                    error = %error.message,
                    "inventory consume outcome is ambiguous; retrying the idempotent operation"
                );
                tokio::time::sleep(Duration::from_millis(25 * u64::from(attempt))).await;
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("inventory consume attempts always return")
}

fn is_retryable_inventory_consume_error(error: &ApiError) -> bool {
    matches!(
        error.code,
        "inventory_commit_unavailable" | "inventory_commit_protocol_error"
    )
}

async fn consume_inventory_attempt(
    state: &AppState,
    context: &Context,
    order: &PendingOrder,
    reservations: &[InventoryReservation],
    fence: &CheckoutFence,
    timeout_ms: u64,
    payload: &Value,
) -> ApiResult<()> {
    let mut headers = HeaderMap::new();
    playground_telemetry::inject_context_headers(context, &mut headers);
    let response = with_checkout_fence(state, fence, || async {
        state
            .http
            .post(format!(
                "{}/consume",
                state.inventory_url.trim_end_matches('/')
            ))
            .headers(headers)
            .timeout(Duration::from_millis(timeout_ms.clamp(1, 30_000)))
            .json(&payload)
            .send()
            .await
            .map_err(|error| {
                ApiError::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "inventory_commit_unavailable",
                    error.to_string(),
                )
            })
    })
    .await?;
    assert_checkout_fence(state, fence).await?;
    let status = response.status();
    if !status.is_success() {
        return Err(ApiError::new(
            if status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS {
                StatusCode::SERVICE_UNAVAILABLE
            } else {
                StatusCode::CONFLICT
            },
            if status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS {
                "inventory_commit_unavailable"
            } else {
                "inventory_commit_failed"
            },
            format!("inventory consume returned {status}"),
        ));
    }
    let body = response.json::<Value>().await.map_err(|error| {
        ApiError::new(
            StatusCode::BAD_GATEWAY,
            "inventory_commit_protocol_error",
            error.to_string(),
        )
    })?;
    let response: InventoryConsumeResponse = serde_json::from_value(body).map_err(|_| {
        ApiError::new(
            StatusCode::BAD_GATEWAY,
            "inventory_commit_protocol_error",
            "inventory returned an incomplete consume payload",
        )
    })?;
    let expected_quantity: u32 = reservations
        .iter()
        .try_fold(0_u32, |total, reservation| {
            total.checked_add(reservation.quantity)
        })
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::BAD_GATEWAY,
                "inventory_commit_protocol_error",
                "inventory consume quantity overflowed",
            )
        })?;
    let quantity_matches = match response.status.as_str() {
        "consumed" => response.consumed == expected_quantity,
        "already_consumed" => response.consumed == 0,
        _ => false,
    };
    if response.tenant_id != order.tenant_id
        || response.reservation_count != reservations.len()
        || !quantity_matches
    {
        return Err(ApiError::new(
            StatusCode::BAD_GATEWAY,
            "inventory_commit_protocol_error",
            "inventory consume payload does not match the request",
        ));
    }
    Ok(())
}

pub(crate) async fn release_inventory(
    state: &AppState,
    context: &Context,
    order: &PendingOrder,
    reservations: &[InventoryReservation],
    fence: &CheckoutFence,
    timeout_ms: u64,
) -> ApiResult<()> {
    let mut first_error = None;
    for reservation in reservations {
        let mut headers = HeaderMap::new();
        playground_telemetry::inject_context_headers(context, &mut headers);
        let mut release_payload = json!({
            "tenant_id": order.tenant_id,
            "reservation_id": reservation.reservation_id,
            "sku": reservation.sku,
            "quantity": reservation.quantity,
            "checkout_request_id": fence.request_id(),
            "checkout_lease_token": fence.token()
        });
        if let Some(location_id) = reservation.location_id.as_deref() {
            release_payload["location_id"] = json!(location_id);
        } else {
            tracing::warn!(
                order_id = %order.id,
                sku = %reservation.sku,
                "releasing inventory without location_id"
            );
        }
        let response = match with_checkout_fence(state, fence, || async {
            state
                .http
                .post(format!(
                    "{}/release",
                    state.inventory_url.trim_end_matches('/')
                ))
                .headers(headers)
                .timeout(Duration::from_millis(timeout_ms.clamp(1, 30_000)))
                .json(&release_payload)
                .send()
                .await
                .map_err(|error| {
                    ApiError::new(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "inventory_release_unavailable",
                        error.to_string(),
                    )
                })
        })
        .await
        {
            Ok(response) => response,
            Err(error) if error.code == "checkout_lease_lost" => return Err(error),
            Err(error) => {
                first_error.get_or_insert(error);
                continue;
            }
        };
        assert_checkout_fence(state, fence).await?;
        if response.status() == StatusCode::NOT_FOUND {
            tracing::info!(
                order_id = %order.id,
                reservation_id = %reservation.reservation_id,
                "inventory reservation was already absent"
            );
        } else if response.status().is_success() {
            match response.json::<Value>().await {
                Ok(body) => {
                    if let Err(error) =
                        validate_inventory_release_response(&body, order, reservation)
                    {
                        first_error.get_or_insert(error);
                    }
                }
                Err(error) => {
                    first_error.get_or_insert_with(|| {
                        ApiError::new(
                            StatusCode::BAD_GATEWAY,
                            "inventory_protocol_error",
                            format!("inventory release payload could not be decoded: {error}"),
                        )
                    });
                }
            }
        } else if !response.status().is_success() {
            first_error.get_or_insert_with(|| {
                ApiError::new(
                    if response.status() == StatusCode::CONFLICT {
                        StatusCode::CONFLICT
                    } else {
                        StatusCode::SERVICE_UNAVAILABLE
                    },
                    "inventory_release_failed",
                    format!(
                        "inventory release returned {} for {}",
                        response.status(),
                        reservation.sku
                    ),
                )
            });
        }
    }
    first_error.map_or(Ok(()), Err)
}

pub(crate) async fn release_inventory_with_recovery(
    state: &AppState,
    context: &Context,
    order: &PendingOrder,
    reservations: &[InventoryReservation],
    fence: &CheckoutFence,
    timeout_ms: u64,
) -> ApiResult<()> {
    // Persist the recovery intent before attempting the remote release. A
    // process crash after the remote side effect must leave a replayable task.
    enqueue_inventory_compensation_tasks(state, order, reservations, context, fence).await?;
    let task_keys = reservations
        .iter()
        .map(|reservation| {
            generation_compensation_task_key(
                &inventory_compensation_task_key(&reservation.reservation_id),
                fence.token(),
            )
        })
        .collect::<Vec<_>>();
    for task_key in &task_keys {
        begin_compensation_operation(state, order, task_key, fence).await?;
    }
    match release_inventory(state, context, order, reservations, fence, timeout_ms).await {
        Ok(()) => {
            for (reservation, task_key) in reservations.iter().zip(task_keys) {
                complete_compensation_operation(state, order, &task_key, fence).await?;
                resolve_compensation_intent(
                    state,
                    order,
                    &inventory_compensation_task_key(&reservation.reservation_id),
                    fence,
                )
                .await?;
            }
            Ok(())
        }
        Err(error) => Err(error),
    }
}

pub(crate) async fn recommendation(
    state: &AppState,
    context: &Context,
    tenant_id: &str,
    segment: &str,
    sku: &str,
) -> ApiResult<Value> {
    let mut headers = HeaderMap::new();
    playground_telemetry::inject_context_headers(context, &mut headers);
    let response = state
        .http
        .get(format!(
            "{}/recommend?sku={}&tenant_id={}&segment={}",
            state.recommendation_url.trim_end_matches('/'),
            recommendation_query_component(sku),
            recommendation_query_component(tenant_id),
            recommendation_query_component(segment),
        ))
        .headers(headers)
        .send()
        .await
        .map_err(|error| {
            ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "recommendation_unavailable",
                error.to_string(),
            )
        })?;
    if !response.status().is_success() {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "recommendation_unavailable",
            format!("recommendation returned {}", response.status()),
        ));
    }
    let body = response.json::<Value>().await.map_err(|error| {
        ApiError::new(
            StatusCode::BAD_GATEWAY,
            "recommendation_protocol_error",
            error.to_string(),
        )
    })?;
    validate_recommendation_tenant(&body, tenant_id)?;
    Ok(body)
}

pub(crate) fn validate_recommendation_tenant(body: &Value, tenant_id: &str) -> ApiResult<()> {
    if body.get("tenant_id").and_then(Value::as_str) != Some(tenant_id) {
        return Err(ApiError::new(
            StatusCode::BAD_GATEWAY,
            "recommendation_protocol_error",
            "recommendation response tenant does not match the order",
        ));
    }
    Ok(())
}

pub(crate) fn recommendation_query_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

pub(crate) async fn enqueue_inventory_compensation_tasks(
    state: &AppState,
    order: &PendingOrder,
    reservations: &[InventoryReservation],
    context: &Context,
    fence: &CheckoutFence,
) -> ApiResult<()> {
    if reservations.is_empty() {
        return Ok(());
    }
    let (traceparent, tracestate, baggage) = stored_propagation(context)?;
    let mut client = acquire_db(&state.pool).await?;
    let transaction = client.transaction().await.map_err(db_error)?;
    lock_checkout_fence(&transaction, fence).await?;
    for reservation in reservations {
        let quantity = i32::try_from(reservation.quantity).map_err(|_| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "compensation_persistence_failed",
                "inventory compensation quantity is out of range",
            )
        })?;
        let task_key = generation_compensation_task_key(
            &inventory_compensation_task_key(&reservation.reservation_id),
            fence.token(),
        );
        let changed = transaction
            .execute(
                "INSERT INTO checkout_compensation_tasks (id, tenant_id, order_id, task_key, kind, reservation_id, sku, quantity, status, available_at, checkout_request_id, checkout_lease_token, traceparent, tracestate, baggage) SELECT $1,$2,$3,$4,'inventory_release',$5,$6,$7,'prepared',CURRENT_TIMESTAMP,$8,$9,$10,$11,$12 WHERE ($13='attempt' AND EXISTS (SELECT 1 FROM checkout_attempts a WHERE a.tenant_id=$2 AND a.request_id=$8 AND a.lease_token=$9 AND a.status IN ('started','failed'))) OR ($13='payment_reconciliation' AND EXISTS (SELECT 1 FROM checkout_payment_reconciliations r WHERE r.tenant_id=$2 AND r.request_id=$8 AND r.checkout_lease_token=$9 AND r.status='processing')) ON CONFLICT (tenant_id, task_key) DO UPDATE SET status=CASE WHEN checkout_compensation_tasks.status IN ('completed','processing','prepared','failed','superseded') THEN checkout_compensation_tasks.status ELSE 'queued' END, available_at=CASE WHEN checkout_compensation_tasks.status IN ('completed','processing','prepared','failed','superseded') THEN checkout_compensation_tasks.available_at ELSE CURRENT_TIMESTAMP END, claimed_at=CASE WHEN checkout_compensation_tasks.status IN ('completed','processing','prepared','failed','superseded') THEN checkout_compensation_tasks.claimed_at ELSE NULL END, completed_at=CASE WHEN checkout_compensation_tasks.status='completed' THEN checkout_compensation_tasks.completed_at ELSE NULL END, last_error=CASE WHEN checkout_compensation_tasks.status IN ('completed','processing','prepared','failed','superseded') THEN checkout_compensation_tasks.last_error ELSE NULL END, checkout_request_id=CASE WHEN checkout_compensation_tasks.status IN ('completed','processing','prepared','failed','superseded') THEN checkout_compensation_tasks.checkout_request_id ELSE EXCLUDED.checkout_request_id END, checkout_lease_token=CASE WHEN checkout_compensation_tasks.status IN ('completed','processing','prepared','failed','superseded') THEN checkout_compensation_tasks.checkout_lease_token ELSE EXCLUDED.checkout_lease_token END, traceparent=CASE WHEN checkout_compensation_tasks.status IN ('completed','processing','prepared','failed','superseded') THEN checkout_compensation_tasks.traceparent ELSE EXCLUDED.traceparent END, tracestate=CASE WHEN checkout_compensation_tasks.status IN ('completed','processing','prepared','failed','superseded') THEN checkout_compensation_tasks.tracestate ELSE EXCLUDED.tracestate END, baggage=CASE WHEN checkout_compensation_tasks.status IN ('completed','processing','prepared','failed','superseded') THEN checkout_compensation_tasks.baggage ELSE EXCLUDED.baggage END, updated_at=CURRENT_TIMESTAMP",
                &[
                    &format!("compensation-{}", uuid::Uuid::new_v4()),
                    &order.tenant_id,
                    &order.id,
                    &task_key,
                    &reservation.reservation_id,
                    &reservation.sku,
                    &quantity,
                    &fence.request_id(),
                    &fence.token(),
                    &traceparent,
                    &tracestate,
                    &baggage,
                    &fence.kind(),
                ],
            )
            .await
            .map_err(|error| {
                ApiError::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "compensation_persistence_failed",
                    format!("inventory compensation task: {error}"),
                )
            })?;
        if changed == 0 {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "checkout_lease_lost",
                "inventory compensation intent was not created by the current checkout fence",
            ));
        }
    }
    transaction.commit().await.map_err(|error| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "compensation_persistence_failed",
            format!("inventory compensation commit: {error}"),
        )
    })?;
    drop(client);
    for reservation in reservations {
        activate_compensation_intent(
            state,
            order,
            &generation_compensation_task_key(
                &inventory_compensation_task_key(&reservation.reservation_id),
                fence.token(),
            ),
            fence,
        )
        .await?;
    }
    Ok(())
}

pub(crate) async fn enqueue_payment_compensation_task(
    state: &AppState,
    order: &PendingOrder,
    payment_id: &str,
    request_id: &str,
    context: &Context,
    fence: &CheckoutFence,
) -> ApiResult<()> {
    let (traceparent, tracestate, baggage) = stored_propagation(context)?;
    let mut client = acquire_db(&state.pool).await?;
    let transaction = client.transaction().await.map_err(db_error)?;
    lock_checkout_fence(&transaction, fence).await?;
    let task_key =
        generation_compensation_task_key(&payment_compensation_task_key(payment_id), fence.token());
    let changed = transaction
        .execute(
            "INSERT INTO checkout_compensation_tasks (id, tenant_id, order_id, task_key, kind, payment_id, request_id, currency, status, available_at, checkout_request_id, checkout_lease_token, traceparent, tracestate, baggage) SELECT $1,$2,$3,$4,'payment',$5,$6,$7,'prepared',CURRENT_TIMESTAMP,$8,$9,$10,$11,$12 WHERE ($13='attempt' AND EXISTS (SELECT 1 FROM checkout_attempts a WHERE a.tenant_id=$2 AND a.request_id=$8 AND a.lease_token=$9 AND a.status IN ('started','failed'))) OR ($13='payment_reconciliation' AND EXISTS (SELECT 1 FROM checkout_payment_reconciliations r WHERE r.tenant_id=$2 AND r.request_id=$8 AND r.checkout_lease_token=$9 AND r.status='processing')) ON CONFLICT (tenant_id, task_key) DO UPDATE SET status=CASE WHEN checkout_compensation_tasks.status IN ('completed','processing','prepared','failed','superseded') THEN checkout_compensation_tasks.status ELSE 'queued' END, available_at=CASE WHEN checkout_compensation_tasks.status IN ('completed','processing','prepared','failed','superseded') THEN checkout_compensation_tasks.available_at ELSE CURRENT_TIMESTAMP END, claimed_at=CASE WHEN checkout_compensation_tasks.status IN ('completed','processing','prepared','failed','superseded') THEN checkout_compensation_tasks.claimed_at ELSE NULL END, completed_at=CASE WHEN checkout_compensation_tasks.status='completed' THEN checkout_compensation_tasks.completed_at ELSE NULL END, last_error=CASE WHEN checkout_compensation_tasks.status IN ('completed','processing','prepared','failed','superseded') THEN checkout_compensation_tasks.last_error ELSE NULL END, checkout_request_id=CASE WHEN checkout_compensation_tasks.status IN ('completed','processing','prepared','failed','superseded') THEN checkout_compensation_tasks.checkout_request_id ELSE EXCLUDED.checkout_request_id END, checkout_lease_token=CASE WHEN checkout_compensation_tasks.status IN ('completed','processing','prepared','failed','superseded') THEN checkout_compensation_tasks.checkout_lease_token ELSE EXCLUDED.checkout_lease_token END, traceparent=CASE WHEN checkout_compensation_tasks.status IN ('completed','processing','prepared','failed','superseded') THEN checkout_compensation_tasks.traceparent ELSE EXCLUDED.traceparent END, tracestate=CASE WHEN checkout_compensation_tasks.status IN ('completed','processing','prepared','failed','superseded') THEN checkout_compensation_tasks.tracestate ELSE EXCLUDED.tracestate END, baggage=CASE WHEN checkout_compensation_tasks.status IN ('completed','processing','prepared','failed','superseded') THEN checkout_compensation_tasks.baggage ELSE EXCLUDED.baggage END, updated_at=CURRENT_TIMESTAMP",
            &[
                &format!("compensation-{}", uuid::Uuid::new_v4()),
                &order.tenant_id,
                &order.id,
                &task_key,
                &payment_id,
                &request_id,
                &order.currency,
                &fence.request_id(),
                &fence.token(),
                &traceparent,
                &tracestate,
                &baggage,
                &fence.kind(),
            ],
        )
        .await
        .map_err(|error| {
            ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "compensation_persistence_failed",
                format!("payment compensation task: {error}"),
            )
        })?;
    if changed == 0 {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "checkout_lease_lost",
            "payment compensation intent was not created by the current checkout fence",
        ));
    }
    transaction.commit().await.map_err(db_error)?;
    drop(client);
    activate_compensation_intent(state, order, &task_key, fence).await?;
    Ok(())
}

pub(crate) fn payment_compensation_task_key(payment_id: &str) -> String {
    format!("payment-compensation:{payment_id}")
}

pub(crate) fn inventory_compensation_task_key(reservation_id: &str) -> String {
    format!("inventory-release:{reservation_id}")
}

pub(crate) fn generation_compensation_task_key(base_key: &str, parent_token: &str) -> String {
    let suffix = format!(":{parent_token}");
    if base_key.ends_with(&suffix) {
        base_key.to_owned()
    } else {
        format!("{base_key}{suffix}")
    }
}

pub(crate) fn compensation_operation_id(task_key: &str) -> String {
    format!("checkout-compensation:{task_key}")
}

async fn activate_compensation_intent(
    state: &AppState,
    order: &PendingOrder,
    task_key: &str,
    fence: &CheckoutFence,
) -> ApiResult<()> {
    let mut client = acquire_db(&state.pool).await?;
    let transaction = client.transaction().await.map_err(db_error)?;
    lock_checkout_fence(&transaction, fence).await?;
    let changed = transaction
        .execute(
            "UPDATE checkout_compensation_tasks SET status='queued', available_at=LEAST(available_at, CURRENT_TIMESTAMP), updated_at=CURRENT_TIMESTAMP WHERE tenant_id=$1 AND order_id=$2 AND task_key=$3 AND status='prepared' AND checkout_request_id=$4 AND checkout_lease_token=$5 AND (($6='attempt' AND EXISTS (SELECT 1 FROM checkout_attempts a WHERE a.tenant_id=$1 AND a.request_id=$4 AND a.lease_token=$5 AND a.status IN ('started','failed'))) OR ($6='payment_reconciliation' AND EXISTS (SELECT 1 FROM checkout_payment_reconciliations r WHERE r.tenant_id=$1 AND r.request_id=$4 AND r.checkout_lease_token=$5 AND r.status='processing'))) ",
            &[
                &order.tenant_id,
                &order.id,
                &task_key,
                &fence.request_id(),
                &fence.token(),
                &fence.kind(),
            ],
        )
        .await
        .map_err(db_error)?;
    if changed == 1 {
        transaction.commit().await.map_err(db_error)?;
        return Ok(());
    }
    let status = transaction
        .query_opt(
            "SELECT status FROM checkout_compensation_tasks WHERE tenant_id=$1 AND order_id=$2 AND task_key=$3",
            &[&order.tenant_id, &order.id, &task_key],
        )
        .await
        .map_err(db_error)?
        .map(|row| row.get::<_, String>(0));
    transaction.commit().await.map_err(db_error)?;
    if matches!(
        status.as_deref(),
        Some("queued" | "processing" | "completed")
    ) {
        return Ok(());
    }
    Err(ApiError::new(
        StatusCode::CONFLICT,
        "checkout_lease_lost",
        "compensation intent could not be activated by the current checkout fence",
    ))
}

pub(crate) async fn resolve_compensation_intent(
    state: &AppState,
    order: &PendingOrder,
    task_key: &str,
    fence: &CheckoutFence,
) -> ApiResult<()> {
    let task_key = generation_compensation_task_key(task_key, fence.token());
    let mut client = acquire_db(&state.pool).await?;
    let transaction = client.transaction().await.map_err(db_error)?;
    lock_checkout_fence(&transaction, fence).await?;
    let changed = transaction
        .execute(
            "UPDATE checkout_compensation_tasks SET status='completed', completed_at=CURRENT_TIMESTAMP, claimed_at=NULL, claim_token=NULL, claim_expires_at=NULL, updated_at=CURRENT_TIMESTAMP WHERE tenant_id=$1 AND order_id=$2 AND task_key=$3 AND status='queued' AND checkout_request_id=$4 AND checkout_lease_token=$5 AND (($6='attempt' AND EXISTS (SELECT 1 FROM checkout_attempts a WHERE a.tenant_id=$1 AND a.request_id=$4 AND a.lease_token=$5 AND a.status IN ('started','failed'))) OR ($6='payment_reconciliation' AND EXISTS (SELECT 1 FROM checkout_payment_reconciliations r WHERE r.tenant_id=$1 AND r.request_id=$4 AND r.checkout_lease_token=$5 AND r.status='processing'))) ",
            &[
                &order.tenant_id,
                &order.id,
                &task_key,
                &fence.request_id(),
                &fence.token(),
                &fence.kind(),
            ],
        )
        .await
        .map_err(db_error)?;
    if changed > 1 {
        transaction.rollback().await.map_err(db_error)?;
        return Err(ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "compensation_persistence_failed",
            "duplicate compensation intents exist",
        ));
    }
    if changed == 0 {
        let status = transaction
            .query_opt(
                "SELECT status FROM checkout_compensation_tasks WHERE tenant_id=$1 AND order_id=$2 AND task_key=$3",
                &[&order.tenant_id, &order.id, &task_key],
            )
            .await
            .map_err(db_error)?
            .map(|row| row.get::<_, String>(0));
        transaction.commit().await.map_err(db_error)?;
        if matches!(status.as_deref(), Some("processing" | "completed")) {
            return Ok(());
        }
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "checkout_lease_lost",
            "compensation intent ownership was lost before resolution",
        ));
    }
    transaction.commit().await.map_err(db_error)
}

pub(crate) async fn load_pending_order(
    state: &AppState,
    tenant_id: &str,
    order_id: &str,
) -> ApiResult<PendingOrder> {
    let client = acquire_db(&state.pool).await?;
    let row = client
        .query_opt(
            "SELECT o.id, o.order_number, o.cart_id, o.customer_id, o.currency, o.subtotal_minor, o.discount_minor, o.tax_minor, o.total_minor, c.session_id, o.promotion_code FROM orders o LEFT JOIN carts c ON c.tenant_id=o.tenant_id AND c.id=o.cart_id WHERE o.tenant_id=$1 AND o.id=$2",
            &[&tenant_id, &order_id],
        )
        .await
        .map_err(db_error)?
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                "order_not_found",
                "compensation order was not found",
            )
        })?;
    let cart_id = row.get::<_, Option<String>>(2).ok_or_else(|| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "compensation_persistence_failed",
            "compensation order has no cart identity",
        )
    })?;
    let session_id = row.get::<_, Option<String>>(9).ok_or_else(|| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "compensation_persistence_failed",
            "compensation order has no session identity",
        )
    })?;
    let item_rows = client
        .query(
            "SELECT product_id, variant_id, sku, product_name, quantity, unit_price_minor FROM order_items WHERE tenant_id=$1 AND order_id=$2 ORDER BY created_at",
            &[&tenant_id, &order_id],
        )
        .await
        .map_err(db_error)?;
    let mut lines = Vec::with_capacity(item_rows.len());
    for row in item_rows {
        let quantity: i32 = row.get(4);
        let unit_price_minor: i32 = row.get(5);
        lines.push(OrderLine {
            product_id: row.get(0),
            variant_id: row.get(1),
            sku: row.get(2),
            product_name: row.get(3),
            quantity: u32::try_from(quantity).map_err(|_| {
                ApiError::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "compensation_persistence_failed",
                    "compensation order item quantity is invalid",
                )
            })?,
            unit_price_minor: i64::from(unit_price_minor),
        });
    }
    Ok(PendingOrder {
        id: row.get(0),
        order_number: row.get(1),
        cart_id,
        tenant_id: tenant_id.to_owned(),
        customer_id: row.get::<_, Option<String>>(3).ok_or_else(|| {
            ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "compensation_persistence_failed",
                "compensation order has no customer identity",
            )
        })?,
        session_id,
        currency: row.get(4),
        lines,
        subtotal_minor: i64::from(row.get::<_, i32>(5)),
        discount_minor: i64::from(row.get::<_, i32>(6)),
        tax_minor: i64::from(row.get::<_, i32>(7)),
        total_minor: i64::from(row.get::<_, i32>(8)),
        promotion_code: row.get(10),
    })
}

pub(crate) async fn cancel_order_fenced(
    state: &AppState,
    order: &PendingOrder,
    fence: &CheckoutFence,
) -> ApiResult<()> {
    let mut client = acquire_db(&state.pool).await?;
    let transaction = client.transaction().await.map_err(db_error)?;
    lock_checkout_fence(&transaction, fence).await?;
    let changed = transaction
        .execute(
            "UPDATE orders SET status='cancelled', updated_at=CURRENT_TIMESTAMP WHERE tenant_id=$1 AND id=$2 AND status='pending' AND (($3='attempt' AND EXISTS (SELECT 1 FROM checkout_attempts a WHERE a.tenant_id=$1 AND a.request_id=$4 AND a.lease_token=$5 AND a.status='started')) OR ($3='payment_reconciliation' AND EXISTS (SELECT 1 FROM checkout_payment_reconciliations r WHERE r.tenant_id=$1 AND r.request_id=$4 AND r.checkout_lease_token=$5 AND r.status='processing'))) ",
            &[
                &order.tenant_id,
                &order.id,
                &fence.kind(),
                &fence.request_id(),
                &fence.token(),
            ],
        )
        .await
        .map_err(db_error)?;
    if changed != 1 {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "order_state_conflict",
            "pending order could not be cancelled",
        ));
    }
    transaction.commit().await.map_err(db_error)
}

pub(crate) async fn cancel_order_if_compensated(
    state: &AppState,
    order: &PendingOrder,
    fence: &CheckoutFence,
) -> ApiResult<()> {
    let mut client = acquire_db(&state.pool).await?;
    let transaction = client.transaction().await.map_err(db_error)?;
    lock_checkout_fence(&transaction, fence).await?;
    let changed = transaction
        .execute(
            "UPDATE orders o SET status='cancelled', updated_at=CURRENT_TIMESTAMP WHERE o.tenant_id=$1 AND o.id=$2 AND o.status='pending' AND NOT EXISTS (SELECT 1 FROM checkout_compensation_tasks t WHERE t.tenant_id=o.tenant_id AND t.order_id=o.id AND t.status NOT IN ('completed','superseded')) AND (($3='attempt' AND EXISTS (SELECT 1 FROM checkout_attempts a WHERE a.tenant_id=$1 AND a.request_id=$4 AND a.lease_token=$5 AND a.status='started')) OR ($3='payment_reconciliation' AND EXISTS (SELECT 1 FROM checkout_payment_reconciliations r WHERE r.tenant_id=$1 AND r.request_id=$4 AND r.checkout_lease_token=$5 AND r.status='processing'))) ",
            &[
                &order.tenant_id,
                &order.id,
                &fence.kind(),
                &fence.request_id(),
                &fence.token(),
            ],
        )
        .await
        .map_err(db_error)?;
    if changed != 1 {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "order_state_conflict",
            "order was not cancelled because compensation is incomplete or state changed",
        ));
    }
    transaction.commit().await.map_err(db_error)
}

pub(crate) async fn finalization_committed(
    state: &AppState,
    order: &PendingOrder,
) -> ApiResult<bool> {
    let client = acquire_db(&state.pool).await?;
    let event_key = format!("{}:paid", order.id);
    let row = client
        .query_opt(
            "SELECT o.status, EXISTS (SELECT 1 FROM outbox_events e WHERE e.tenant_id=o.tenant_id AND e.event_key=$3) FROM orders o WHERE o.tenant_id=$1 AND o.id=$2",
            &[&order.tenant_id, &order.id, &event_key],
        )
        .await
        .map_err(db_error)?
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "finalization_reconciliation_pending",
                "finalized order is not observable yet",
            )
        })?;
    let status: String = row.get(0);
    let outbox_present: bool = row.get(1);
    if status == "paid" && outbox_present {
        return Ok(true);
    }
    if status == "paid" {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "finalization_reconciliation_pending",
            "paid order is missing its transactional outbox event",
        ));
    }
    if status == "pending" {
        return Ok(false);
    }
    Err(ApiError::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "finalization_reconciliation_pending",
        format!("order is in unexpected state: {status}"),
    ))
}

pub(crate) async fn finalize_order(
    state: &AppState,
    context: &Context,
    order: &PendingOrder,
    payment_id: &str,
    _request_id: &str,
    feature_variant: &str,
    fence: &CheckoutFence,
) -> ApiResult<()> {
    let payload = json!({
        "order_id": order.id,
        "order_number": order.order_number,
        "tenant_id": order.tenant_id,
        "customer_id": order.customer_id,
        "session_id": order.session_id,
        "currency": order.currency,
        "subtotal_minor": order.subtotal_minor,
        "discount_minor": order.discount_minor,
        "tax_minor": order.tax_minor,
        "shipping_minor": 0,
        "total_minor": order.total_minor,
        "payment_id": payment_id,
        "payment_status": "captured",
        "items": order.lines.iter().map(|line| json!({"sku": line.sku, "quantity": line.quantity, "unit_price_minor": line.unit_price_minor})).collect::<Vec<_>>()
    });
    let context_value = json!({
        "feature_variant": feature_variant,
        "customer_segment": context.baggage().get("customer.segment").map(ToString::to_string),
        "region": context.baggage().get("region").map(ToString::to_string),
        "request_priority": context.baggage().get("request.priority").map(ToString::to_string)
    });
    let (traceparent, tracestate, baggage) = stored_propagation(context)?;
    let trace_id = context.span().span_context().trace_id().to_string();
    let span_id = context.span().span_context().span_id().to_string();
    let event_key = format!("{}:paid", order.id);
    let exposure_id = format!("exposure-{}", uuid::Uuid::new_v4());
    let session_id = &order.session_id;
    // The paid outbox event is the cross-service identity. Persist the same
    // exposure key that the fulfillment analytics consumer derives from that
    // event so PostgreSQL and ClickHouse cannot describe different exposures.
    let exposure_key = format!("{event_key}:checkoutFlow");
    let mut client = acquire_db(&state.pool).await?;
    let transaction = client.transaction().await.map_err(db_error)?;
    lock_checkout_fence(&transaction, fence).await?;
    let order_changed = transaction
        .execute("UPDATE orders SET status='paid', placed_at=COALESCE(placed_at,CURRENT_TIMESTAMP), updated_at=CURRENT_TIMESTAMP WHERE tenant_id=$1 AND id=$2 AND status='pending' AND (($3='attempt' AND EXISTS (SELECT 1 FROM checkout_attempts a WHERE a.tenant_id=$1 AND a.request_id=$4 AND a.lease_token=$5 AND a.status='started')) OR ($3='payment_reconciliation' AND EXISTS (SELECT 1 FROM checkout_payment_reconciliations r WHERE r.tenant_id=$1 AND r.request_id=$4 AND r.checkout_lease_token=$5 AND r.status='processing'))) ", &[
            &order.tenant_id,
            &order.id,
            &fence.kind(),
            &fence.request_id(),
            &fence.token(),
        ])
        .await
        .map_err(|error| db_error_at("finalize orders", error))?;
    if order_changed != 1 {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "finalization_state_conflict",
            "order is no longer pending",
        ));
    }
    let cart_changed = transaction
        .execute("UPDATE carts SET status='checked_out', updated_at=CURRENT_TIMESTAMP WHERE tenant_id=$1 AND id=$2 AND status='active'", &[&order.tenant_id, &order.cart_id])
        .await
        .map_err(|error| db_error_at("finalize cart", error))?;
    if cart_changed != 1 {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "finalization_state_conflict",
            "order cart is no longer active",
        ));
    }
    if let Some(promotion_code) = order
        .promotion_code
        .as_deref()
        .filter(|code| !code.is_empty())
    {
        let redeemed = transaction
            .execute(
                "UPDATE promotions SET redemption_count=redemption_count+1 WHERE tenant_id=$1 AND code=$2 AND active AND starts_at <= CURRENT_TIMESTAMP AND (ends_at IS NULL OR ends_at > CURRENT_TIMESTAMP) AND (max_redemptions IS NULL OR redemption_count < max_redemptions)",
                &[&order.tenant_id, &promotion_code],
            )
            .await
            .map_err(|error| db_error_at("redeem promotion", error))?;
        if redeemed != 1 {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "promotion_redemption_unavailable",
                "promotion is no longer redeemable",
            ));
        }
    }
    transaction
        .execute(
            "INSERT INTO feature_exposures (id, tenant_id, customer_id, session_id, feature_key, variant, exposure_key, context, exposed_at) VALUES ($1,$2,$3,$4,'checkoutFlow',$5,$6,$7::jsonb,CURRENT_TIMESTAMP) ON CONFLICT (tenant_id, exposure_key) DO NOTHING",
            &[&exposure_id, &order.tenant_id, &order.customer_id, &session_id, &feature_variant, &exposure_key, &context_value],
        )
        .await
        .map_err(|error| db_error_at("finalize exposure", error))?;
    transaction
        .execute(
            "INSERT INTO analytics_events (id, tenant_id, event_key, customer_id, session_id, event_name, event_version, source, entity_type, entity_id, occurred_at, trace_id, span_id, properties, context) VALUES ($1,$2,$3,$4,$5,'order.paid',1,'checkout','order',$6,CURRENT_TIMESTAMP,NULLIF($7,''),NULLIF($8,''),$9::jsonb,$10::jsonb) ON CONFLICT (tenant_id, event_key) DO NOTHING",
            &[&format!("analytics-{}", uuid::Uuid::new_v4()), &order.tenant_id, &event_key, &order.customer_id, &session_id, &order.id, &trace_id, &span_id, &payload, &context_value],
        )
        .await
        .map_err(|error| db_error_at("finalize analytics", error))?;
    transaction
        .execute(
            "INSERT INTO outbox_events (id, tenant_id, event_key, aggregate_type, aggregate_id, event_type, schema_version, payload, traceparent, tracestate, baggage, status) VALUES ($1,$2,$3,'order',$4,'order.paid',1,$5::jsonb,$6,$7,$8,'queued') ON CONFLICT (tenant_id, event_key) DO NOTHING",
            &[&format!("outbox-{}", order.id), &order.tenant_id, &event_key, &order.id, &payload, &traceparent, &tracestate, &baggage],
        )
        .await
        .map_err(|error| db_error_at("finalize outbox", error))?;
    transaction.commit().await.map_err(|error| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "finalization_commit_ambiguous",
            format!("finalization commit outcome is unknown: {error}"),
        )
    })?;
    Ok(())
}

pub(crate) fn stored_propagation(context: &Context) -> ApiResult<(String, String, String)> {
    let headers =
        playground_telemetry::inject_durable_context_headers(context).map_err(|error| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "propagation_persistence_invalid",
                format!("generated durable W3C carrier is invalid: {error}"),
            )
        })?;
    let header = |name: &'static str| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
            .ok_or_else(|| {
                ApiError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "propagation_persistence_invalid",
                    format!("generated durable W3C carrier is missing {name}"),
                )
            })
    };
    Ok((
        header("traceparent")?,
        header("tracestate")?,
        header("baggage")?,
    ))
}

pub(crate) fn context_with_checkout_variant(context: &Context, variant: &str) -> Context {
    playground_telemetry::extend_baggage(
        context,
        [KeyValue::new("feature.variant", variant.to_owned())],
    )
}

pub(crate) fn context_with_session_id(context: &Context, session_id: Option<&str>) -> Context {
    match session_id.filter(|value| !value.trim().is_empty()) {
        Some(session_id) => playground_telemetry::extend_baggage(
            context,
            [KeyValue::new("session.id", session_id.to_owned())],
        ),
        None => context.clone(),
    }
}

pub(crate) fn context_with_identity(
    context: &Context,
    tenant_id: &str,
    session_id: &str,
) -> Context {
    playground_telemetry::extend_baggage(
        context,
        [
            KeyValue::new("tenant.id", tenant_id.to_owned()),
            KeyValue::new("session.id", session_id.to_owned()),
        ],
    )
}

pub(crate) fn quote_json(quote: &QuoteResponse) -> Value {
    json!({
        "quote_id": quote.quote_id,
        "status": quote.status,
        "lines": quote.lines.iter().map(|line| json!({
            "sku": line.sku,
            "quantity": line.quantity,
            "unit_price": money_json(line.unit_price.as_ref()),
            "line_total": money_json(line.line_total.as_ref())
        })).collect::<Vec<_>>(),
        "subtotal": money_json(quote.subtotal.as_ref()),
        "discount_total": money_json(quote.discount_total.as_ref()),
        "tax_total": money_json(quote.tax_total.as_ref()),
        "grand_total": money_json(quote.grand_total.as_ref()),
        "pricing_version": quote.pricing_version
    })
}

pub(crate) fn money_json(money: Option<&QuoteMoney>) -> Value {
    money.map_or(Value::Null, |money| {
        json!({
            "currency_code": money.currency_code,
            "amount_minor": money.amount_minor
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry::propagation::text_map_propagator::FieldIter;
    use opentelemetry::propagation::{Extractor, Injector, TextMapPropagator};
    use opentelemetry::trace::{SpanContext, SpanId, TraceFlags, TraceId, TraceState};
    use opentelemetry::{KeyValue, global};

    #[derive(Debug)]
    struct CompleteDurablePropagator;

    impl TextMapPropagator for CompleteDurablePropagator {
        fn inject_context(&self, context: &Context, injector: &mut dyn Injector) {
            if context.span().span_context().is_valid() {
                injector.set(
                    "traceparent",
                    "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01".to_owned(),
                );
            }
            if context.baggage().get("tenant.id").is_some() {
                injector.set("baggage", "tenant.id=tenant-acme".to_owned());
            }
        }

        fn extract_with_context(&self, context: &Context, _extractor: &dyn Extractor) -> Context {
            context.clone()
        }

        fn fields(&self) -> FieldIter<'_> {
            FieldIter::new(&[])
        }
    }

    #[test]
    fn payment_method_mapping_is_typed() {
        assert_eq!(payment_method_type("card") as i32, 1);
        assert_eq!(payment_method_type("bank_account") as i32, 2);
        assert_eq!(payment_method_type("wallet") as i32, 3);
    }

    #[test]
    fn checkout_flag_context_contains_request_identity() {
        let input = CheckoutInput {
            tenant_id: "tenant-nova".into(),
            customer_id: "customer-nova-mia".into(),
            session_id: None,
            cart_id: None,
            items: vec![CheckoutItemInput {
                sku: "NOVA-PACK-20".into(),
                quantity: 1,
            }],
            currency_code: "USD".into(),
            promotion_code: None,
            segment: "outdoor".into(),
            tier: "premium".into(),
            region: "us-west-2".into(),
            priority: "normal".into(),
            payment_method_token: Some("tok_visa".into()),
            payment_method_type: Some("card".into()),
            request_id: Some("checkout-context".into()),
            delay_ms: 0,
            slow: 0,
            retry: 0,
            timeout_ms: 1000,
            degrade: false,
        };
        let context = checkout_evaluation_context(&input);
        assert_eq!(
            context.targeting_key.as_deref(),
            Some("tenant-nova:customer-nova-mia")
        );
        assert_eq!(
            context
                .custom_fields
                .get("tenant_id")
                .and_then(|value| value.as_str()),
            Some("tenant-nova")
        );
        assert_eq!(
            context
                .custom_fields
                .get("customer_id")
                .and_then(|value| value.as_str()),
            Some("customer-nova-mia")
        );
        assert_eq!(
            context
                .custom_fields
                .get("segment")
                .and_then(|value| value.as_str()),
            Some("outdoor")
        );
        assert_eq!(
            context
                .custom_fields
                .get("tier")
                .and_then(|value| value.as_str()),
            Some("premium")
        );
    }

    #[test]
    fn checkout_flow_variants_change_recommendation_topology() {
        assert!(!checkout_variant_includes_recommendation("control"));
        assert!(checkout_variant_includes_recommendation("orchestrated"));
    }

    #[test]
    fn compensation_attempts_have_a_terminal_bound() {
        assert!(!compensation_attempts_exhausted(
            COMPENSATION_MAX_ATTEMPTS - 1
        ));
        assert!(compensation_attempts_exhausted(COMPENSATION_MAX_ATTEMPTS));
        assert!(compensation_attempts_exhausted(i32::MAX));
    }

    #[test]
    fn compensation_identity_is_generation_scoped_and_idempotent() {
        let first = generation_compensation_task_key("payment-compensation:pay-1", "lease-a");
        let successor = generation_compensation_task_key("payment-compensation:pay-1", "lease-b");

        assert_ne!(first, successor);
        assert_eq!(generation_compensation_task_key(&first, "lease-a"), first);
        assert_eq!(
            compensation_operation_id(&first),
            "checkout-compensation:payment-compensation:pay-1:lease-a"
        );
    }

    #[test]
    fn checkout_context_keeps_selected_feature_variant() {
        let context = context_with_checkout_variant(
            &playground_telemetry::extend_baggage(
                &Context::new(),
                [
                    KeyValue::new("tenant.id", "kept"),
                    KeyValue::new("upstream.value", "dropped"),
                ],
            ),
            "control",
        );
        assert_eq!(
            context
                .baggage()
                .get("feature.variant")
                .map(ToString::to_string),
            Some("control".to_owned())
        );
        assert_eq!(
            context.baggage().get("tenant.id").map(ToString::to_string),
            Some("kept".to_owned())
        );
        assert_eq!(context.baggage().get("upstream.value"), None);
    }

    #[test]
    fn stored_propagation_rejects_partial_generated_carriers() {
        let error = stored_propagation(&Context::new()).expect_err("partial carrier");
        assert_eq!(error.code, "propagation_persistence_invalid");
    }

    #[test]
    fn stored_propagation_returns_complete_valid_generated_carrier() {
        global::set_text_map_propagator(CompleteDurablePropagator);
        let context = Context::new()
            .with_remote_span_context(SpanContext::new(
                TraceId::from_hex("4bf92f3577b34da6a3ce929d0e0e4736").expect("trace id"),
                SpanId::from_hex("00f067aa0ba902b7").expect("span id"),
                TraceFlags::SAMPLED,
                true,
                TraceState::default(),
            ))
            .with_baggage([KeyValue::new("tenant.id", "tenant-acme")]);

        let (traceparent, tracestate, baggage) =
            stored_propagation(&context).expect("complete generated carrier");
        assert_eq!(
            traceparent,
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"
        );
        assert_eq!(tracestate, "playground=commerce");
        assert_eq!(baggage, "tenant.id=tenant-acme");
    }

    #[test]
    fn recommendation_response_must_match_order_tenant() {
        assert!(
            validate_recommendation_tenant(&json!({"tenant_id": "tenant-nova"}), "tenant-nova")
                .is_ok()
        );
        assert!(
            validate_recommendation_tenant(&json!({"tenant_id": "tenant-acme"}), "tenant-nova")
                .is_err()
        );
        assert_eq!(
            recommendation_query_component("tenant nova"),
            "tenant%20nova"
        );
    }

    #[test]
    fn checkout_attempt_reclaims_only_stale_started_rows() {
        assert!(matches!(
            started_checkout_attempt_claim(false),
            CheckoutAttemptClaim::InProgress
        ));
        assert!(matches!(
            started_checkout_attempt_claim(true),
            CheckoutAttemptClaim::New { .. }
        ));
        assert!(CHECKOUT_ATTEMPT_STALE_AFTER >= Duration::from_secs(60));
        assert!(CHECKOUT_ATTEMPT_STALE_AFTER <= Duration::from_secs(15 * 60));
    }

    #[test]
    fn stale_reclaim_keeps_durable_order_linkage_safe() {
        assert!(stale_reclaim_order_is_safe(None, None));
        assert!(stale_reclaim_order_is_safe(
            Some("order-1"),
            Some("pending")
        ));
        assert!(!stale_reclaim_order_is_safe(
            Some("order-1"),
            Some("cancelled")
        ));
        assert!(!stale_reclaim_order_is_safe(Some("order-1"), None));
    }

    #[test]
    fn checkout_fingerprint_changes_with_tenant() {
        let acme = CheckoutInput {
            tenant_id: "tenant-acme".into(),
            customer_id: "customer-acme-ava".into(),
            session_id: None,
            cart_id: None,
            items: vec![CheckoutItemInput {
                sku: "WIDGET-1".into(),
                quantity: 1,
            }],
            currency_code: "USD".into(),
            promotion_code: None,
            segment: "standard".into(),
            tier: "free".into(),
            region: "us-east-1".into(),
            priority: "normal".into(),
            payment_method_token: Some("tok_visa".into()),
            payment_method_type: Some("card".into()),
            request_id: Some("same-request".into()),
            delay_ms: 0,
            slow: 0,
            retry: 0,
            timeout_ms: 1000,
            degrade: false,
        };
        let mut nova = acme.clone();
        nova.tenant_id = "tenant-nova".into();
        assert_ne!(
            checkout_request_fingerprint(&acme),
            checkout_request_fingerprint(&nova)
        );
    }

    #[test]
    fn payment_outcomes_keep_decline_and_provider_distinctions() {
        let decline = payment_operation_error(
            "payment authorization",
            PaymentOperationStatus::Declined,
            PaymentFailureReason::InsufficientFunds,
        );
        assert_eq!(decline.status, StatusCode::PAYMENT_REQUIRED);
        assert_eq!(decline.code, "payment_insufficient_funds");

        let provider = payment_operation_error(
            "payment authorization",
            PaymentOperationStatus::Failed,
            PaymentFailureReason::ProviderUnavailable,
        );
        assert_eq!(provider.status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(provider.code, "payment_provider_unavailable");
    }

    #[test]
    fn validation_rejects_empty_or_oversized_orders() {
        let mut input = CheckoutInput {
            tenant_id: default_tenant(),
            customer_id: default_customer(),
            session_id: None,
            cart_id: None,
            items: Vec::new(),
            currency_code: default_currency(),
            promotion_code: None,
            segment: default_segment(),
            tier: default_tier(),
            region: default_region(),
            priority: default_priority(),
            payment_method_token: Some("tok_test".into()),
            payment_method_type: Some("card".into()),
            request_id: None,
            delay_ms: 0,
            slow: 0,
            retry: 0,
            timeout_ms: default_timeout_ms(),
            degrade: false,
        };
        assert!(validate_input(&input).is_err());
        input.items = vec![CheckoutItemInput {
            sku: "WIDGET-1".into(),
            quantity: MAX_QUANTITY + 1,
        }];
        assert!(validate_input(&input).is_err());
    }
}
