//! Checkout domain types, validation values, and boundary errors.

use axum::Json;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use opentelemetry::{Context, baggage::BaggageExt};
use playground_proto::payment::v1::{PaymentFailureReason, PaymentOperationStatus, PaymentStatus};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::Duration;
pub(crate) const DEFAULT_DATABASE_URL: &str =
    "postgres://postgres:playground@postgres:5432/playground";

pub(crate) const DEFAULT_RABBITMQ_URL: &str = "amqp://playground:playground@rabbitmq:5672/%2f";

pub(crate) const DEFAULT_CATALOG_URL: &str = "http://catalog:8080/graphql";

pub(crate) const DEFAULT_PRICING_ENDPOINT: &str = "http://pricing:50051";

pub(crate) const DEFAULT_PAYMENT_ENDPOINT: &str = "http://payment:9090";

pub(crate) const DEFAULT_INVENTORY_URL: &str = "http://inventory:8089";

pub(crate) const DEFAULT_RECOMMENDATION_URL: &str = "http://recommendation:8090";

pub(crate) const EXCHANGE: &str = "commerce.events";

pub(crate) const DB_TIMEOUT: Duration = Duration::from_secs(3);

pub(crate) const CHECKOUT_ATTEMPT_STALE_AFTER: Duration = Duration::from_secs(10 * 60);

pub(crate) const CHECKOUT_LEASE_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);

pub(crate) const PAYMENT_RECONCILIATION_POLL_INTERVAL: Duration = Duration::from_millis(500);

pub(crate) const PAYMENT_RECONCILIATION_STALE_AFTER: Duration = Duration::from_secs(5 * 60);

pub(crate) const PAYMENT_RECONCILIATION_RETRY_BASE: i32 = 2;

pub(crate) const PAYMENT_RECONCILIATION_MAX_ATTEMPTS: i32 = 8;

pub(crate) const COMPENSATION_POLL_INTERVAL: Duration = Duration::from_millis(500);

pub(crate) const COMPENSATION_STALE_AFTER: Duration = Duration::from_secs(5 * 60);

pub(crate) const COMPENSATION_RETRY_BASE: i32 = 2;

pub(crate) const COMPENSATION_MAX_ATTEMPTS: i32 = 3;

pub(crate) const COMPENSATION_ERROR_LIMIT: usize = 1_000;

pub(crate) const MAX_ITEMS: usize = 50;

pub(crate) const MAX_QUANTITY: u32 = 100;

pub(crate) const MAX_IDENTIFIER_LENGTH: usize = 128;

#[derive(Debug)]

pub(crate) struct ApiError {
    pub(crate) status: StatusCode,
    pub(crate) code: &'static str,
    pub(crate) message: String,
}

impl ApiError {
    pub(crate) fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    pub(crate) fn is_provider_failure(&self) -> bool {
        matches!(
            self.code,
            "payment_provider_unavailable" | "payment_timeout" | "payment_internal"
        )
    }

    pub(crate) fn is_ambiguous_payment_failure(&self) -> bool {
        self.is_provider_failure() || self.code == "payment_protocol_error"
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        (
            self.status,
            Json(json!({"error": self.code, "message": self.message})),
        )
            .into_response()
    }
}

pub(crate) type ApiResult<T> = Result<T, ApiError>;

#[derive(Debug, Clone, Deserialize, Serialize)]

pub(crate) struct CheckoutItemInput {
    pub(crate) sku: String,
    pub(crate) quantity: u32,
}

#[derive(Debug, Clone, Deserialize)]

pub(crate) struct CheckoutInput {
    #[serde(default)]
    pub(crate) tenant_id: String,
    #[serde(default)]
    pub(crate) customer_id: String,
    #[serde(default)]
    pub(crate) session_id: Option<String>,
    #[serde(default)]
    pub(crate) cart_id: Option<String>,
    #[serde(default)]
    pub(crate) items: Vec<CheckoutItemInput>,
    #[serde(default = "default_currency")]
    pub(crate) currency_code: String,
    #[serde(default)]
    pub(crate) promotion_code: Option<String>,
    #[serde(default = "default_segment")]
    pub(crate) segment: String,
    #[serde(default = "default_tier")]
    pub(crate) tier: String,
    #[serde(default = "default_region")]
    pub(crate) region: String,
    #[serde(default = "default_priority")]
    pub(crate) priority: String,
    /// Real checkout validates these at the command boundary.  Missing
    /// credentials must never turn into a synthetic payment method.
    pub(crate) payment_method_token: Option<String>,
    pub(crate) payment_method_type: Option<String>,
    #[serde(default)]
    pub(crate) request_id: Option<String>,
    /// Bounded, isolated scenario controls used by the telemetry corpus.
    #[serde(default)]
    pub(crate) delay_ms: u64,
    #[serde(default)]
    pub(crate) slow: u64,
    #[serde(default)]
    pub(crate) retry: u32,
    #[serde(default = "default_timeout_ms")]
    pub(crate) timeout_ms: u64,
    #[serde(default)]
    pub(crate) degrade: bool,
}

#[derive(Debug, Deserialize)]

pub(crate) struct QuoteStreamQuery {
    pub(crate) sku: String,
    #[serde(default = "default_quantity")]
    pub(crate) quantity: u32,
    #[serde(default)]
    pub(crate) tenant_id: String,
    #[serde(default)]
    pub(crate) customer_id: String,
    #[serde(default)]
    pub(crate) session_id: Option<String>,
    #[serde(default)]
    pub(crate) cart_id: Option<String>,
    #[serde(default = "default_currency")]
    pub(crate) currency_code: String,
    #[serde(default)]
    pub(crate) promotion_code: Option<String>,
    #[serde(default = "default_segment")]
    pub(crate) segment: String,
    #[serde(default = "default_tier")]
    pub(crate) tier: String,
    #[serde(default = "default_region")]
    pub(crate) region: String,
    #[serde(default = "default_priority")]
    pub(crate) priority: String,
    #[serde(default)]
    pub(crate) request_id: Option<String>,
    #[serde(default)]
    pub(crate) delay_ms: u64,
    #[serde(default)]
    pub(crate) slow: u64,
    #[serde(default)]
    pub(crate) retry: u32,
    #[serde(default = "default_timeout_ms")]
    pub(crate) timeout_ms: u64,
    #[serde(default, deserialize_with = "de_flag")]
    pub(crate) degrade: bool,
}

impl From<QuoteStreamQuery> for CheckoutInput {
    fn from(query: QuoteStreamQuery) -> Self {
        Self {
            tenant_id: query.tenant_id,
            customer_id: query.customer_id,
            session_id: query.session_id,
            cart_id: query.cart_id,
            items: vec![CheckoutItemInput {
                sku: query.sku,
                quantity: query.quantity,
            }],
            currency_code: query.currency_code,
            promotion_code: query.promotion_code,
            segment: query.segment,
            tier: query.tier,
            region: query.region,
            priority: query.priority,
            payment_method_token: None,
            payment_method_type: None,
            request_id: query.request_id,
            delay_ms: query.delay_ms,
            slow: query.slow,
            retry: query.retry,
            timeout_ms: query.timeout_ms,
            degrade: query.degrade,
        }
    }
}

#[derive(Debug, Deserialize)]

pub(crate) struct CartQuery {
    #[serde(default)]
    pub(crate) tenant_id: String,
    pub(crate) customer_id: Option<String>,
    #[serde(default)]
    pub(crate) session_id: Option<String>,
}

#[derive(Debug, Deserialize)]

pub(crate) struct AddCartItemInput {
    #[serde(default)]
    pub(crate) tenant_id: String,
    #[serde(default)]
    pub(crate) customer_id: String,
    pub(crate) cart_id: Option<String>,
    pub(crate) session_id: Option<String>,
    pub(crate) sku: String,
    pub(crate) quantity: u32,
    #[serde(default = "default_currency")]
    pub(crate) currency_code: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CartItemScope {
    #[serde(default)]
    pub(crate) tenant_id: String,
    #[serde(default)]
    pub(crate) customer_id: String,
    #[serde(default)]
    pub(crate) cart_id: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ReplaceCartItemInput {
    #[serde(flatten)]
    pub(crate) scope: CartItemScope,
    #[serde(default)]
    pub(crate) quantity: i64,
}

impl CartItemScope {
    pub(crate) fn validate(&self) -> ApiResult<()> {
        if !valid_identity_component(&self.tenant_id)
            || !valid_identity_component(&self.customer_id)
            || !valid_identity_component(&self.cart_id)
        {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "invalid_cart_scope",
                "tenant_id, customer_id, and cart_id are required",
            ));
        }
        Ok(())
    }
}

impl ReplaceCartItemInput {
    pub(crate) fn validate(&self, sku: &str) -> ApiResult<u32> {
        self.scope.validate()?;
        validate_cart_sku(sku)?;
        if self.quantity < 1 || self.quantity > i64::from(MAX_QUANTITY) {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "invalid_cart_quantity",
                format!("quantity must be between 1 and {MAX_QUANTITY}"),
            ));
        }
        u32::try_from(self.quantity).map_err(|_| {
            ApiError::new(
                StatusCode::BAD_REQUEST,
                "invalid_cart_quantity",
                format!("quantity must be between 1 and {MAX_QUANTITY}"),
            )
        })
    }
}

pub(crate) fn validate_cart_sku(sku: &str) -> ApiResult<()> {
    if sku.trim().is_empty() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_cart_sku",
            "sku is required",
        ));
    }
    Ok(())
}

#[derive(Debug, Deserialize)]

pub(crate) struct OrderQuery {
    #[serde(default)]
    pub(crate) tenant_id: String,
    pub(crate) customer_id: Option<String>,
    #[serde(default)]
    pub(crate) session_id: Option<String>,
}

#[derive(Debug, Clone)]

pub(crate) struct OrderLine {
    pub(crate) product_id: String,
    pub(crate) variant_id: String,
    pub(crate) sku: String,
    pub(crate) product_name: String,
    pub(crate) quantity: u32,
    pub(crate) unit_price_minor: i64,
}

#[derive(Debug, Clone)]

pub(crate) struct PendingOrder {
    pub(crate) id: String,
    pub(crate) order_number: String,
    pub(crate) cart_id: String,
    pub(crate) tenant_id: String,
    pub(crate) customer_id: String,
    pub(crate) session_id: String,
    pub(crate) currency: String,
    pub(crate) lines: Vec<OrderLine>,
    pub(crate) subtotal_minor: i64,
    pub(crate) discount_minor: i64,
    pub(crate) tax_minor: i64,
    pub(crate) total_minor: i64,
    pub(crate) promotion_code: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct CheckoutLease {
    pub(crate) tenant_id: String,
    pub(crate) request_id: String,
    pub(crate) token: String,
}

#[derive(Debug, Clone)]
pub(crate) enum CheckoutFence {
    Attempt(CheckoutLease),
    PaymentReconciliation {
        tenant_id: String,
        request_id: String,
        parent_token: String,
        worker_token: String,
    },
    Compensation {
        tenant_id: String,
        request_id: String,
        token: String,
        task_id: String,
        claim_token: String,
    },
}

impl CheckoutFence {
    pub(crate) fn request_id(&self) -> &str {
        match self {
            Self::Attempt(lease) => &lease.request_id,
            Self::PaymentReconciliation { request_id, .. } => request_id,
            Self::Compensation { request_id, .. } => request_id,
        }
    }

    pub(crate) fn token(&self) -> &str {
        match self {
            Self::Attempt(lease) => &lease.token,
            Self::PaymentReconciliation { parent_token, .. } => parent_token,
            Self::Compensation { token, .. } => token,
        }
    }

    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::Attempt(_) => "attempt",
            Self::PaymentReconciliation { .. } => "payment_reconciliation",
            Self::Compensation { .. } => "compensation",
        }
    }
}

#[derive(Debug, Clone)]

pub(crate) struct PaymentAuthorization {
    pub(crate) payment_id: String,
    pub(crate) operation_status: PaymentOperationStatus,
    pub(crate) failure_reason: PaymentFailureReason,
}

pub(crate) enum AuthorizationRecovery {
    Recovered(PaymentAuthorization),
    Absent,
    Pending,
    Failed(ApiError),
}

pub(crate) struct PaymentSnapshot {
    pub(crate) authorized_minor: i64,
    pub(crate) currency: String,
    pub(crate) status: PaymentStatus,
    pub(crate) failure_reason: PaymentFailureReason,
    pub(crate) captured_minor: i64,
    pub(crate) refunded_minor: i64,
}

#[derive(Debug)]
pub(crate) struct PaymentReconciliationJob {
    pub(crate) tenant_id: String,
    pub(crate) request_id: String,
    pub(crate) order_id: String,
    pub(crate) authorize_request_id: String,
    pub(crate) payment_id: Option<String>,
    pub(crate) merchant_reference: String,
    pub(crate) amount_minor: i64,
    pub(crate) currency: String,
    pub(crate) method_type: String,
    pub(crate) feature_variant: String,
    pub(crate) status: String,
    pub(crate) attempts: i32,
    pub(crate) lease_token: String,
    pub(crate) parent_lease_token: String,
    pub(crate) traceparent: Option<String>,
    pub(crate) tracestate: Option<String>,
    pub(crate) baggage: Option<String>,
}

pub(crate) enum PendingPaymentOutcome {
    Retry,
    Completed(Value),
    Failed(ApiError),
}

#[derive(Debug, Clone)]

pub(crate) struct InventoryReservation {
    pub(crate) reservation_id: String,
    pub(crate) sku: String,
    pub(crate) quantity: u32,
    pub(crate) location_id: Option<String>,
}

#[derive(Debug)]

pub(crate) struct CompensationTask {
    pub(crate) id: String,
    pub(crate) tenant_id: String,
    pub(crate) order_id: String,
    pub(crate) kind: String,
    pub(crate) reservation_id: Option<String>,
    pub(crate) sku: Option<String>,
    pub(crate) quantity: Option<u32>,
    pub(crate) payment_id: Option<String>,
    pub(crate) request_id: Option<String>,
    pub(crate) currency: Option<String>,
    pub(crate) attempts: i32,
    pub(crate) traceparent: Option<String>,
    pub(crate) tracestate: Option<String>,
    pub(crate) baggage: Option<String>,
    pub(crate) checkout_request_id: Option<String>,
    pub(crate) checkout_lease_token: Option<String>,
    pub(crate) claim_token: String,
}

pub(crate) enum CheckoutAttemptClaim {
    New { lease_token: String },
    Pending(Value),
    Replay(Value),
    InProgress,
    Failed(ApiError),
}

#[derive(Debug, Serialize)]

pub(crate) struct GraphQlRequest {
    pub(crate) query: &'static str,
    pub(crate) variables: Value,
    pub(crate) operation_name: &'static str,
}

#[cfg(test)]
pub(crate) fn default_tenant() -> String {
    "tenant-acme".to_owned()
}

#[cfg(test)]
pub(crate) fn default_customer() -> String {
    "customer-acme-ava".to_owned()
}

fn valid_identity_component(value: &str) -> bool {
    !value.trim().is_empty()
        && value.len() <= MAX_IDENTIFIER_LENGTH
        && !value.chars().any(char::is_control)
}

pub(crate) fn resolve_tenant_id(context: &Context, requested: &str) -> ApiResult<String> {
    let requested = (!requested.trim().is_empty()).then_some(requested.trim());
    let propagated = context
        .baggage()
        .get("tenant.id")
        .map(ToString::to_string)
        .filter(|value| !value.is_empty());
    if let (Some(requested), Some(propagated)) = (requested, propagated.as_deref())
        && requested != propagated
    {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "identity_conflict",
            "tenant identity conflicts with propagated request identity",
        ));
    }
    let tenant_id = requested.map(str::to_owned).or(propagated).ok_or_else(|| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "tenant_required",
            "tenant_id is required in the request or tenant.id baggage",
        )
    })?;
    if !valid_identity_component(&tenant_id) {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_identity",
            "tenant_id is invalid",
        ));
    }
    Ok(tenant_id)
}

pub(crate) fn require_customer_id(requested: Option<&str>) -> ApiResult<String> {
    let customer_id = requested
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::BAD_REQUEST,
                "customer_required",
                "customer_id is required",
            )
        })?;
    if !valid_identity_component(customer_id) {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_identity",
            "customer_id is invalid",
        ));
    }
    Ok(customer_id.to_owned())
}

pub(crate) fn resolve_session_id(
    context: &Context,
    requested: Option<&str>,
) -> ApiResult<Option<String>> {
    let requested = requested
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    if let Some(value) = requested.as_deref()
        && !valid_identity_component(value)
    {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_session",
            "session_id is invalid",
        ));
    }
    let propagated = context
        .baggage()
        .get("session.id")
        .map(ToString::to_string)
        .filter(|value| !value.is_empty());
    if let (Some(requested), Some(propagated)) = (requested.as_deref(), propagated.as_deref())
        && requested != propagated
    {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "identity_conflict",
            "session identity conflicts with propagated request identity",
        ));
    }
    Ok(requested.or(propagated))
}

pub(crate) fn resolve_checkout_identity(
    context: &Context,
    requested_tenant: &str,
    requested_session: Option<&str>,
    fallback_session: &str,
) -> ApiResult<(String, String)> {
    let tenant_id = resolve_tenant_id(context, requested_tenant)?;
    let session_id = resolve_session_id(context, requested_session)?
        .unwrap_or_else(|| fallback_session.to_owned());
    if !valid_identity_component(&session_id) {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_session",
            "session_id is invalid",
        ));
    }
    Ok((tenant_id, session_id))
}

pub(crate) fn default_currency() -> String {
    "USD".to_owned()
}

pub(crate) fn default_segment() -> String {
    "standard".to_owned()
}

pub(crate) fn default_tier() -> String {
    "free".to_owned()
}

pub(crate) fn default_region() -> String {
    "us-east-1".to_owned()
}

pub(crate) fn default_priority() -> String {
    "normal".to_owned()
}

pub(crate) fn default_timeout_ms() -> u64 {
    1_000
}

pub(crate) fn default_quantity() -> u32 {
    1
}

pub(crate) fn de_flag<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    let value = String::deserialize(deserializer)?;
    Ok(matches!(value.as_str(), "1" | "true" | "yes" | "on"))
}

pub(crate) fn require_payment_credentials(input: &CheckoutInput) -> ApiResult<(&str, &str)> {
    let token = input
        .payment_method_token
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::BAD_REQUEST,
                "payment_method_required",
                "payment_method_token is required",
            )
        })?;
    if !valid_identity_component(token) {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_payment_method_token",
            "payment_method_token is invalid",
        ));
    }
    let method = input
        .payment_method_type
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::BAD_REQUEST,
                "payment_method_required",
                "payment_method_type is required",
            )
        })?;
    if !matches!(
        method.trim().to_ascii_lowercase().as_str(),
        "card" | "bank_account" | "bank-account" | "wallet"
    ) {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_payment_method_type",
            "payment_method_type is unsupported",
        ));
    }
    Ok((token, method))
}

#[cfg(test)]
mod cart_item_tests {
    use super::*;

    fn scope() -> CartItemScope {
        CartItemScope {
            tenant_id: "tenant-acme".to_owned(),
            customer_id: "customer-acme-ava".to_owned(),
            cart_id: "cart-acme-liam".to_owned(),
        }
    }

    #[test]
    fn replacement_accepts_bounded_quantity_and_sku() {
        let input = ReplaceCartItemInput {
            scope: scope(),
            quantity: i64::from(MAX_QUANTITY),
        };

        assert_eq!(input.validate("WIDGET-1").unwrap(), MAX_QUANTITY);
    }

    #[test]
    fn replacement_rejects_out_of_range_quantity() {
        for quantity in [0, -1, i64::from(MAX_QUANTITY) + 1] {
            let input = ReplaceCartItemInput {
                scope: scope(),
                quantity,
            };
            let error = input.validate("WIDGET-1").unwrap_err();
            assert_eq!(error.code, "invalid_cart_quantity");
        }
    }

    #[test]
    fn mutations_require_tenant_customer_and_cart_scope() {
        let mut input = scope();
        input.cart_id.clear();
        assert_eq!(input.validate().unwrap_err().code, "invalid_cart_scope");

        assert_eq!(validate_cart_sku(" ").unwrap_err().code, "invalid_cart_sku");
    }
}
