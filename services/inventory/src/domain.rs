use serde::Deserialize;

pub(crate) const MAX_IDENTIFIER_LENGTH: usize = 128;
pub(crate) const MAX_RESERVE_QUANTITY: u32 = 1_000_000;
pub(crate) const MAX_RELEASE_QUANTITY: u32 = 1_000_000;
pub(crate) const MAX_CONSUME_RESERVATIONS: usize = 50;

#[derive(Debug, Deserialize)]
pub(crate) struct ReserveQuery {
    #[serde(default)]
    pub(crate) tenant_id: String,
    pub(crate) reservation_id: String,
    pub(crate) sku: String,
    #[serde(default = "one")]
    pub(crate) quantity: u32,
    #[serde(default)]
    pub(crate) slow: u64,
    #[serde(default)]
    pub(crate) db_n1: u32,
    #[serde(default)]
    pub(crate) hold_ms: u64,
    /// Isolated deterministic fault injection for the telemetry corpus.
    #[serde(default, deserialize_with = "de_flag")]
    pub(crate) fail: bool,
    #[serde(default)]
    pub(crate) checkout_request_id: Option<String>,
    #[serde(default)]
    pub(crate) checkout_lease_token: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ReserveBody {
    #[serde(default)]
    pub(crate) tenant_id: String,
    pub(crate) reservation_id: String,
    pub(crate) sku: String,
    #[serde(default = "one")]
    pub(crate) quantity: u32,
    #[serde(default)]
    pub(crate) checkout_request_id: Option<String>,
    #[serde(default)]
    pub(crate) checkout_lease_token: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ReleaseBody {
    #[serde(default)]
    pub(crate) tenant_id: String,
    pub(crate) reservation_id: String,
    pub(crate) sku: String,
    pub(crate) quantity: u32,
    #[serde(default)]
    pub(crate) location_id: Option<String>,
    #[serde(default)]
    pub(crate) checkout_request_id: Option<String>,
    #[serde(default)]
    pub(crate) checkout_lease_token: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub(crate) struct ConsumeBody {
    #[serde(default)]
    pub(crate) tenant_id: String,
    #[serde(default)]
    pub(crate) checkout_request_id: Option<String>,
    #[serde(default)]
    pub(crate) checkout_lease_token: Option<String>,
    pub(crate) reservations: Vec<ConsumeReservation>,
}

#[derive(Debug, Deserialize, Clone)]
pub(crate) struct ConsumeReservation {
    pub(crate) reservation_id: String,
    pub(crate) sku: String,
    pub(crate) quantity: u32,
    #[serde(default)]
    pub(crate) location_id: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ValidationError {
    pub(crate) field: &'static str,
    pub(crate) reason: &'static str,
}

#[derive(Debug)]
pub(crate) struct ReservationConflict {
    pub(crate) code: &'static str,
    pub(crate) message: String,
}

impl std::fmt::Display for ReservationConflict {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ReservationConflict {}

#[derive(Debug)]
pub(crate) struct ReserveOutcome {
    pub(crate) location_id: String,
    pub(crate) remaining: i32,
    pub(crate) status: &'static str,
}

#[derive(Debug)]
pub(crate) struct ReleaseOutcome {
    pub(crate) location_id: String,
    pub(crate) released: i32,
    pub(crate) reserved_remaining: i32,
    pub(crate) available: i32,
    pub(crate) status: &'static str,
}

#[derive(Debug)]
pub(crate) struct ConsumeOutcome {
    pub(crate) consumed: u32,
    pub(crate) status: &'static str,
}

fn one() -> u32 {
    1
}

fn de_flag<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    let value = String::deserialize(deserializer)?;
    Ok(matches!(value.as_str(), "1" | "true" | "yes" | "on"))
}

pub(crate) fn validate_reserve(params: &ReserveQuery) -> Result<(), ValidationError> {
    validate_identifier(&params.tenant_id, "tenant_id")?;
    validate_identifier(&params.reservation_id, "reservation_id")?;
    validate_identifier(&params.sku, "sku")?;
    if params.quantity == 0 {
        return Err(ValidationError {
            field: "quantity",
            reason: "must be greater than zero",
        });
    }
    if params.quantity > MAX_RESERVE_QUANTITY {
        return Err(ValidationError {
            field: "quantity",
            reason: "exceeds the maximum reserve quantity",
        });
    }
    validate_fence_fields(
        params.checkout_request_id.as_deref(),
        params.checkout_lease_token.as_deref(),
    )?;
    Ok(())
}

pub(crate) fn validate_release(body: &ReleaseBody) -> Result<(), ValidationError> {
    validate_identifier(&body.tenant_id, "tenant_id")?;
    validate_identifier(&body.reservation_id, "reservation_id")?;
    validate_identifier(&body.sku, "sku")?;
    if body.quantity == 0 {
        return Err(ValidationError {
            field: "quantity",
            reason: "must be greater than zero",
        });
    }
    if body.quantity > MAX_RELEASE_QUANTITY {
        return Err(ValidationError {
            field: "quantity",
            reason: "exceeds the maximum release quantity",
        });
    }
    if let Some(location_id) = &body.location_id {
        validate_identifier(location_id, "location_id")?;
    }
    validate_fence_fields(
        body.checkout_request_id.as_deref(),
        body.checkout_lease_token.as_deref(),
    )?;
    Ok(())
}

pub(crate) fn validate_consume(body: &ConsumeBody) -> Result<(), ValidationError> {
    validate_identifier(&body.tenant_id, "tenant_id")?;
    if body.reservations.is_empty() || body.reservations.len() > MAX_CONSUME_RESERVATIONS {
        return Err(ValidationError {
            field: "reservations",
            reason: "must contain between one and fifty reservations",
        });
    }
    validate_fence_fields(
        body.checkout_request_id.as_deref(),
        body.checkout_lease_token.as_deref(),
    )?;
    let mut reservation_ids = std::collections::HashSet::with_capacity(body.reservations.len());
    for reservation in &body.reservations {
        validate_identifier(&reservation.reservation_id, "reservation_id")?;
        validate_identifier(&reservation.sku, "sku")?;
        if reservation.quantity == 0 || reservation.quantity > MAX_RELEASE_QUANTITY {
            return Err(ValidationError {
                field: "quantity",
                reason: "must be between one and one million",
            });
        }
        if let Some(location_id) = &reservation.location_id {
            validate_identifier(location_id, "location_id")?;
        }
        if !reservation_ids.insert(&reservation.reservation_id) {
            return Err(ValidationError {
                field: "reservation_id",
                reason: "must be unique within a consume request",
            });
        }
    }
    Ok(())
}

fn validate_fence_fields(
    request_id: Option<&str>,
    lease_token: Option<&str>,
) -> Result<(), ValidationError> {
    match (request_id, lease_token) {
        (Some(request_id), Some(lease_token)) => {
            validate_identifier(request_id, "checkout_request_id")?;
            validate_identifier(lease_token, "checkout_lease_token")?;
        }
        (None, None) => {
            return Err(ValidationError {
                field: "checkout_request_id",
                reason: "authoritative checkout fence is required",
            });
        }
        (Some(_), None) => {
            return Err(ValidationError {
                field: "checkout_lease_token",
                reason: "is required with checkout_request_id",
            });
        }
        (None, Some(_)) => {
            return Err(ValidationError {
                field: "checkout_request_id",
                reason: "is required with checkout_lease_token",
            });
        }
    }
    Ok(())
}

fn validate_identifier(value: &str, field: &'static str) -> Result<(), ValidationError> {
    if value.is_empty() {
        return Err(ValidationError {
            field,
            reason: "must not be empty",
        });
    }
    if value.len() > MAX_IDENTIFIER_LENGTH {
        return Err(ValidationError {
            field,
            reason: "exceeds the maximum length",
        });
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        return Err(ValidationError {
            field,
            reason: "contains unsupported characters",
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_validation_accepts_compensation_request() {
        let body = ReleaseBody {
            tenant_id: "tenant-acme".to_owned(),
            reservation_id: "checkout-1:inventory:0".to_owned(),
            sku: "WIDGET-1".to_owned(),
            quantity: 2,
            location_id: Some("loc-acme-east".to_owned()),
            checkout_request_id: Some("checkout-1".to_owned()),
            checkout_lease_token: Some("lease-1".to_owned()),
        };
        assert_eq!(validate_release(&body), Ok(()));
    }

    #[test]
    fn reserve_validation_rejects_zero_and_oversized_quantities() {
        let mut params = ReserveQuery {
            tenant_id: "tenant-acme".to_owned(),
            reservation_id: "checkout-1:inventory:0".to_owned(),
            sku: "WIDGET-1".to_owned(),
            quantity: 0,
            slow: 0,
            db_n1: 0,
            hold_ms: 0,
            fail: false,
            checkout_request_id: Some("checkout-1".to_owned()),
            checkout_lease_token: Some("lease-1".to_owned()),
        };
        assert_eq!(
            validate_reserve(&params),
            Err(ValidationError {
                field: "quantity",
                reason: "must be greater than zero",
            })
        );

        params.quantity = MAX_RESERVE_QUANTITY + 1;
        assert_eq!(
            validate_reserve(&params),
            Err(ValidationError {
                field: "quantity",
                reason: "exceeds the maximum reserve quantity",
            })
        );
    }

    #[test]
    fn reserve_validation_rejects_unbounded_identifiers() {
        let params = ReserveQuery {
            tenant_id: "tenant/acme".to_owned(),
            reservation_id: "checkout-1:inventory:0".to_owned(),
            sku: "WIDGET-1".to_owned(),
            quantity: 1,
            slow: 0,
            db_n1: 0,
            hold_ms: 0,
            fail: false,
            checkout_request_id: Some("checkout-1".to_owned()),
            checkout_lease_token: Some("lease-1".to_owned()),
        };
        assert_eq!(
            validate_reserve(&params),
            Err(ValidationError {
                field: "tenant_id",
                reason: "contains unsupported characters",
            })
        );
    }

    #[test]
    fn release_validation_rejects_zero_and_oversized_quantities() {
        let mut body = ReleaseBody {
            tenant_id: "tenant-acme".to_owned(),
            reservation_id: "checkout-1:inventory:0".to_owned(),
            sku: "WIDGET-1".to_owned(),
            quantity: 0,
            location_id: None,
            checkout_request_id: Some("checkout-1".to_owned()),
            checkout_lease_token: Some("lease-1".to_owned()),
        };
        assert_eq!(
            validate_release(&body),
            Err(ValidationError {
                field: "quantity",
                reason: "must be greater than zero",
            })
        );

        body.quantity = MAX_RELEASE_QUANTITY + 1;
        assert_eq!(
            validate_release(&body),
            Err(ValidationError {
                field: "quantity",
                reason: "exceeds the maximum release quantity",
            })
        );
    }

    #[test]
    fn release_validation_rejects_unbounded_identifiers() {
        let body = ReleaseBody {
            tenant_id: "tenant/acme".to_owned(),
            reservation_id: "checkout-1:inventory:0".to_owned(),
            sku: "WIDGET-1".to_owned(),
            quantity: 1,
            location_id: None,
            checkout_request_id: Some("checkout-1".to_owned()),
            checkout_lease_token: Some("lease-1".to_owned()),
        };
        assert_eq!(
            validate_release(&body),
            Err(ValidationError {
                field: "tenant_id",
                reason: "contains unsupported characters",
            })
        );
    }

    #[test]
    fn reservation_validation_requires_a_stable_identity() {
        let params = ReserveQuery {
            tenant_id: "tenant-acme".to_owned(),
            reservation_id: String::new(),
            sku: "WIDGET-1".to_owned(),
            quantity: 1,
            slow: 0,
            db_n1: 0,
            hold_ms: 0,
            fail: false,
            checkout_request_id: None,
            checkout_lease_token: None,
        };
        assert_eq!(
            validate_reserve(&params),
            Err(ValidationError {
                field: "reservation_id",
                reason: "must not be empty",
            })
        );
    }
}
