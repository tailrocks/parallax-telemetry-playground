use playground_proto::pricing::v1::{Money, QuoteLine, QuoteRequest, QuoteResponse, QuoteStatus};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use tonic::Status;

pub(crate) const QUOTE_TTL_SECONDS: u64 = 45;
pub(crate) const MAX_ITEMS: usize = 50;
pub(crate) const MAX_QUANTITY: u32 = 100;
pub(crate) const MAX_IDENTIFIER_LENGTH: usize = 128;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CachedQuote {
    pub(crate) quote_id: String,
    pub(crate) lines: Vec<CachedLine>,
    pub(crate) subtotal_minor: i64,
    pub(crate) discount_minor: i64,
    pub(crate) grand_total_minor: i64,
    pub(crate) currency: String,
    pub(crate) pricing_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CachedLine {
    pub(crate) product_id: String,
    pub(crate) sku: String,
    pub(crate) quantity: u32,
    pub(crate) unit_minor: i64,
    pub(crate) line_minor: i64,
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

pub(crate) fn cache_key(request: &QuoteRequest, pricing_version: &str) -> String {
    format!(
        "pricing:quote:{}:{}:{}:{}:{}",
        request.tenant_id,
        request.customer_id,
        request.currency_code,
        pricing_version,
        stable_request_fingerprint(request)
    )
}

pub(crate) fn to_proto(quote: &CachedQuote) -> QuoteResponse {
    QuoteResponse {
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
        valid_for_seconds: QUOTE_TTL_SECONDS as u32,
        pricing_version: quote.pricing_version.clone(),
    }
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
            cache_key(&request, "price-change-7"),
            cache_key(&request, "price-change-7")
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
            cache_key(&acme, "price-change-7"),
            cache_key(&nova, "price-change-7")
        );
        assert_ne!(
            stable_request_fingerprint(&acme),
            stable_request_fingerprint(&nova)
        );
    }

    #[test]
    fn cache_key_changes_when_the_durable_price_version_changes() {
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

        assert_ne!(
            cache_key(&request, "price-change-7"),
            cache_key(&request, "price-change-8")
        );
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
}
