use deadpool_postgres::Pool;

use crate::domain::{
    ConsumeBody, ConsumeOutcome, ReleaseBody, ReleaseOutcome, ReserveOutcome, ReserveQuery,
    ValidationError, validate_consume, validate_release, validate_reserve,
};
use crate::infrastructure;

pub(crate) enum ApplicationError {
    Invalid(ValidationError),
    Operation(anyhow::Error),
}

pub(crate) async fn reserve(
    pool: &Pool,
    params: &ReserveQuery,
) -> Result<Option<ReserveOutcome>, ApplicationError> {
    if let Err(error) = validate_reserve(params) {
        playground_telemetry::mark_span_error("invalid_reserve_request");
        return Err(ApplicationError::Invalid(error));
    }

    infrastructure::reserve(pool, params)
        .await
        .map_err(ApplicationError::Operation)
}

pub(crate) async fn release(
    pool: &Pool,
    body: &ReleaseBody,
) -> Result<Option<ReleaseOutcome>, ApplicationError> {
    if let Err(error) = validate_release(body) {
        playground_telemetry::mark_span_error("invalid_release_request");
        return Err(ApplicationError::Invalid(error));
    }

    infrastructure::release(pool, body)
        .await
        .map_err(ApplicationError::Operation)
}

pub(crate) async fn consume(
    pool: &Pool,
    body: &ConsumeBody,
) -> Result<ConsumeOutcome, ApplicationError> {
    if let Err(error) = validate_consume(body) {
        playground_telemetry::mark_span_error("invalid_consume_request");
        return Err(ApplicationError::Invalid(error));
    }

    infrastructure::consume(pool, body)
        .await
        .map_err(ApplicationError::Operation)
}
