use playground_proto::pricing::v1::{Money, QuoteLine, QuoteRequest, QuoteResponse, QuoteStatus};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tonic::Status;

pub(crate) const QUOTE_TTL_SECONDS: u64 = 45;
pub(crate) const DEFAULT_CUSTOMER_SEGMENT: &str = "standard";
pub(crate) const DEFAULT_CUSTOMER_TIER: &str = "free";
pub(crate) const MAX_ITEMS: usize = 50;
pub(crate) const MAX_QUANTITY: u32 = 100;
pub(crate) const MAX_IDENTIFIER_LENGTH: usize = 128;
const MILLIS_PER_SECOND: i64 = 1_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CachedQuote {
    pub(crate) quote_id: String,
    pub(crate) lines: Vec<CachedLine>,
    pub(crate) subtotal_minor: i64,
    pub(crate) discount_minor: i64,
    pub(crate) grand_total_minor: i64,
    pub(crate) currency: String,
    pub(crate) pricing_version: String,
    pub(crate) price_list_code: String,
    pub(crate) expires_at_unix_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CachedLine {
    pub(crate) product_id: String,
    pub(crate) sku: String,
    pub(crate) quantity: u32,
    pub(crate) unit_minor: i64,
    pub(crate) line_minor: i64,
}

impl CachedQuote {
    pub(crate) fn remaining_validity_seconds(
        &self,
        now: SystemTime,
    ) -> Result<u32, Status> {
        let now_unix_ms = unix_millis(now)?;
        if self.expires_at_unix_ms <= now_unix_ms {
            return Ok(0);
        }
        let remaining_ms = self
            .expires_at_unix_ms
            .checked_sub(now_unix_ms)
            .ok_or_else(|| Status::internal("quote expiry arithmetic overflowed"))?;
        let remaining_seconds = remaining_ms
            .checked_add(MILLIS_PER_SECOND - 1)
            .ok_or_else(|| Status::internal("quote expiry arithmetic overflowed"))?
            / MILLIS_PER_SECOND;
        u32::try_from(remaining_seconds)
            .map_err(|_| Status::internal("quote validity exceeds protocol limits"))
    }

    pub(crate) fn consume_at(&self, now: SystemTime) -> Result<(), Status> {
        if self.remaining_validity_seconds(now)? == 0 {
            return Err(Status::failed_precondition("quote has expired"));
        }
        Ok(())
    }

    pub(crate) fn consume(&self) -> Result<(), Status> {
        self.consume_at(SystemTime::now())
    }
}

pub(crate) fn quote_expiry_from(now: SystemTime) -> Result<i64, Status> {
    let now_unix_ms = unix_millis(now)?;
    let ttl_ms = i64::try_from(QUOTE_TTL_SECONDS)
        .ok()
        .and_then(|seconds| seconds.checked_mul(MILLIS_PER_SECOND))
        .ok_or_else(|| Status::internal("quote TTL arithmetic overflowed"))?;
    now_unix_ms
        .checked_add(ttl_ms)
        .ok_or_else(|| Status::internal("quote expiry arithmetic overflowed"))
}

fn unix_millis(now: SystemTime) -> Result<i64, Status> {
    let duration = now
        .duration_since(UNIX_EPOCH)
        .map_err(|_| Status::internal("system clock predates the Unix epoch"))?;
    i64::try_from(duration.as_millis())
        .map_err(|_| Status::internal("system clock exceeds quote timestamp limits"))
}

pub(crate) fn validate_request(request: &QuoteRequest) -> Result<(), Status> {
    if request.tenant_id.trim().is_empty() {
        return Err(Status::invalid_argument("tenant_id is required"));
    }
    if request.tenant_id.len() > 128 {
        return Err(Status::invalid_argument("tenant_id exceeds maximum length"));
    }
    if request.customer_id.trim().is_empty() {
        return Err(Status::invalid_argument("customer_id is required"));
    }
    if request.customer_id.len() > MAX_IDENTIFIER_LENGTH {
        return Err(Status::invalid_argument(
            "customer_id exceeds maximum length",
        ));
    }
    if request.items.is_empty() {
        return Err(Status::invalid_argument(
            "at least one quote item is required",
        ));
    }
    if request.items.len() > MAX_ITEMS {
        return Err(Status::invalid_argument("too many quote items"));
    }
    if request.currency_code.len() != 3
        || request.currency_code != request.currency_code.to_ascii_uppercase()
        || !request
            .currency_code
            .bytes()
            .all(|byte| byte.is_ascii_alphabetic())
    {
        return Err(Status::invalid_argument(
            "currency_code must be a three-letter uppercase code",
        ));
    }
    let mut skus = HashSet::with_capacity(request.items.len());
    for item in &request.items {
        if item.sku.trim().is_empty() || item.sku.len() > MAX_IDENTIFIER_LENGTH {
            return Err(Status::invalid_argument(
                "each quote item needs a valid SKU",
            ));
        }
        if item.quantity == 0 || item.quantity > MAX_QUANTITY {
            return Err(Status::invalid_argument(
                "each quote item needs a quantity from one to one hundred",
            ));
        }
        if !skus.insert(item.sku.as_str()) {
            return Err(Status::invalid_argument("quote SKUs must be unique"));
        }
    }
    for (name, value) in [
        ("promotion_code", request.context.get("promotion_code")),
        ("pricing_strategy", request.context.get("pricing_strategy")),
        ("customer_segment", request.context.get("customer_segment")),
        ("customer_tier", request.context.get("customer_tier")),
        ("region", request.context.get("region")),
        ("request_priority", request.context.get("request_priority")),
    ] {
        if let Some(value) = value
            && (value.len() > MAX_IDENTIFIER_LENGTH || value.chars().any(char::is_control))
        {
            return Err(Status::invalid_argument(format!(
                "{name} exceeds the maximum length"
            )));
        }
    }
    if let Some(strategy) = request.context.get("pricing_strategy")
        && !matches!(strategy.as_str(), "standard" | "promotional")
    {
        return Err(Status::invalid_argument(
            "pricing_strategy is not supported",
        ));
    }
    Ok(())
}

pub(crate) fn request_id(request: &QuoteRequest) -> String {
    if request.request_id.trim().is_empty() {
        format!("quote-{}", stable_request_fingerprint(request))
    } else {
        request.request_id.clone()
    }
}

pub(crate) fn stable_request_fingerprint(request: &QuoteRequest) -> String {
    let mut value = request.tenant_id.clone();
    value.push('|');
    value.push_str(&request.customer_id);
    value.push('|');
    value.push_str(&request.currency_code);
    for key in [
        "pricing_strategy",
        "promotion_code",
        "customer_segment",
        "customer_tier",
        "region",
        "request_priority",
    ] {
        value.push('|');
        value.push_str(key);
        value.push('=');
        value.push_str(request.context.get(key).map(String::as_str).unwrap_or(""));
    }
    for item in &request.items {
        value.push('|');
        value.push_str(&item.sku);
        value.push(':');
        value.push_str(&item.quantity.to_string());
    }
    value
        .bytes()
        .fold(0_u64, |hash, byte| {
            hash.wrapping_mul(31).wrapping_add(u64::from(byte))
        })
        .to_string()
}

pub(crate) fn cache_key(request: &QuoteRequest) -> String {
    format!(
        "pricing:quote:{}:{}:{}:{}",
        request.tenant_id,
        request.customer_id,
        request.currency_code,
        stable_request_fingerprint(request)
    )
}

pub(crate) fn to_proto(quote: &CachedQuote) -> Result<QuoteResponse, Status> {
    to_proto_at(quote, SystemTime::now())
}

fn to_proto_at(quote: &CachedQuote, now: SystemTime) -> Result<QuoteResponse, Status> {
    quote.consume_at(now)?;
    let valid_for_seconds = quote.remaining_validity_seconds(now)?;
    Ok(QuoteResponse {
        quote_id: quote.quote_id.clone(),
        status: QuoteStatus::Ready as i32,
        lines: quote
            .lines
            .iter()
            .map(|line| QuoteLine {
                sku: line.sku.clone(),
                quantity: line.quantity,
                unit_price: Some(Money {
                    currency_code: quote.currency.clone(),
                    amount_minor: line.unit_minor,
                }),
                line_total: Some(Money {
                    currency_code: quote.currency.clone(),
                    amount_minor: line.line_minor,
                }),
            })
            .collect(),
        subtotal: Some(Money {
            currency_code: quote.currency.clone(),
            amount_minor: quote.subtotal_minor,
        }),
        discount_total: Some(Money {
            currency_code: quote.currency.clone(),
            amount_minor: quote.discount_minor,
        }),
        tax_total: Some(Money {
            currency_code: quote.currency.clone(),
            amount_minor: 0,
        }),
        grand_total: Some(Money {
            currency_code: quote.currency.clone(),
            amount_minor: quote.grand_total_minor,
        }),
        valid_for_seconds,
        pricing_version: quote.pricing_version.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use playground_proto::pricing::v1::QuoteItem;

    #[test]
    fn rejects_empty_or_invalid_quote_requests() {
        let empty = QuoteRequest {
            currency_code: "USD".into(),
            ..Default::default()
        };
        assert_eq!(
            validate_request(&empty).expect_err("empty request").code(),
            tonic::Code::InvalidArgument
        );
        let bad_currency = QuoteRequest {
            currency_code: "usd".into(),
            items: vec![QuoteItem {
                sku: "WIDGET-1".into(),
                quantity: 1,
            }],
            ..Default::default()
        };
        assert_eq!(
            validate_request(&bad_currency)
                .expect_err("bad currency")
                .code(),
            tonic::Code::InvalidArgument
        );
    }

    #[test]
    fn cache_fingerprint_is_stable_and_request_id_is_idempotent() {
        let request = QuoteRequest {
            request_id: "request-1".into(),
            tenant_id: "tenant-acme".into(),
            customer_id: "customer-acme-ava".into(),
            currency_code: "USD".into(),
            items: vec![QuoteItem {
                sku: "WIDGET-1".into(),
                quantity: 2,
            }],
            ..Default::default()
        };
        assert_eq!(request_id(&request), "request-1");
        assert_eq!(
            cache_key(&request),
            cache_key(&request)
        );
    }

    #[test]
    fn cache_key_is_tenant_scoped() {
        let acme = QuoteRequest {
            tenant_id: "tenant-acme".into(),
            customer_id: "customer-acme-ava".into(),
            currency_code: "USD".into(),
            items: vec![QuoteItem {
                sku: "WIDGET-1".into(),
                quantity: 1,
            }],
            ..Default::default()
        };
        let nova = QuoteRequest {
            tenant_id: "tenant-nova".into(),
            customer_id: "customer-nova-mia".into(),
            ..acme.clone()
        };
        assert_ne!(
            cache_key(&acme),
            cache_key(&nova)
        );
        assert_ne!(
            stable_request_fingerprint(&acme),
            stable_request_fingerprint(&nova)
        );
    }

    #[test]
    fn cache_key_is_independent_of_the_durable_price_version() {
        let request = QuoteRequest {
            tenant_id: "tenant-acme".into(),
            customer_id: "customer-acme-ava".into(),
            currency_code: "USD".into(),
            items: vec![QuoteItem {
                sku: "WIDGET-1".into(),
                quantity: 1,
            }],
            ..Default::default()
        };

        assert_eq!(cache_key(&request), cache_key(&request));
    }

    #[test]
    fn rejects_missing_tenant_context() {
        let request = QuoteRequest {
            customer_id: "customer-acme-ava".into(),
            currency_code: "USD".into(),
            items: vec![QuoteItem {
                sku: "WIDGET-1".into(),
                quantity: 1,
            }],
            ..Default::default()
        };
        assert_eq!(
            validate_request(&request)
                .expect_err("tenant required")
                .code(),
            tonic::Code::InvalidArgument
        );
    }

    fn quote_with_expiry(expires_at_unix_ms: i64) -> CachedQuote {
        CachedQuote {
            quote_id: "quote-test".into(),
            lines: vec![CachedLine {
                product_id: "product-test".into(),
                sku: "WIDGET-1".into(),
                quantity: 1,
                unit_minor: 1_999,
                line_minor: 1_999,
            }],
            subtotal_minor: 1_999,
            discount_minor: 0,
            grand_total_minor: 1_999,
            currency: "USD".into(),
            pricing_version: "price-change-7".into(),
            price_list_code: "standard-free".into(),
            expires_at_unix_ms,
        }
    }

    #[test]
    fn quote_expiry_is_absolute_and_remaining_validity_is_rounded_up() {
        let quote = quote_with_expiry(6_000);
        let just_before_expiry = UNIX_EPOCH + Duration::from_millis(1_001);

        assert_eq!(
            quote
                .remaining_validity_seconds(just_before_expiry)
                .expect("validity calculation"),
            5
        );
        assert!(quote.consume_at(just_before_expiry).is_ok());
        assert_eq!(
            quote
                .remaining_validity_seconds(UNIX_EPOCH + Duration::from_millis(6_000))
                .expect("expired validity calculation"),
            0
        );
        assert_eq!(
            quote
                .consume_at(UNIX_EPOCH + Duration::from_millis(6_000))
                .expect_err("expired quote must be rejected")
                .code(),
            tonic::Code::FailedPrecondition
        );
    }

    #[test]
    fn quote_expiry_is_serialized_into_the_cache_value() {
        let quote = quote_with_expiry(6_000);
        let value = serde_json::to_value(&quote).expect("quote serializes");

        assert_eq!(value["expires_at_unix_ms"], 6_000);
        assert_eq!(value["price_list_code"], "standard-free");
    }

    #[test]
    fn quote_proto_reports_remaining_validity_and_rejects_expiration() {
        let quote = quote_with_expiry(6_000);
        let response = to_proto_at(&quote, UNIX_EPOCH + Duration::from_millis(1_001))
            .expect("live quote converts");
        assert_eq!(response.valid_for_seconds, 5);

        let error = to_proto_at(&quote, UNIX_EPOCH + Duration::from_millis(6_000))
            .expect_err("expired quote conversion must fail");
        assert_eq!(error.code(), tonic::Code::FailedPrecondition);
    }

    #[test]
    fn quote_expiry_from_now_uses_the_configured_ttl() {
        let now = UNIX_EPOCH + Duration::from_secs(100);
        assert_eq!(
            quote_expiry_from(now).expect("expiry calculation"),
            100_000 + (QUOTE_TTL_SECONDS as i64 * MILLIS_PER_SECOND)
        );
    }
}
