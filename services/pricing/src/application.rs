use crate::{
    domain::{CachedQuote, cache_key, validate_request},
    infrastructure::AppState,
};
use open_feature::EvaluationContext;
use opentelemetry::KeyValue;
use playground_proto::pricing::v1::QuoteRequest;
use std::{
    collections::HashMap,
    time::{Duration, SystemTime},
};
use tonic::Status;

pub(crate) async fn calculate_quote(
    state: &AppState,
    request: &QuoteRequest,
) -> Result<CachedQuote, Status> {
    validate_request(request)?;
    let pricing_mode = tokio::time::timeout(
        Duration::from_millis(500),
        playground_telemetry::feature_variant(
            "pricingStrategy",
            "cached",
            "PRICING_STRATEGY",
            pricing_evaluation_context(request),
        ),
    )
    .await
    .unwrap_or_else(|_| "cached".to_owned());
    let use_cache = match pricing_mode.as_str() {
        "cached" => true,
        "database" => false,
        _ => {
            tracing::warn!(
                variant = %pricing_mode,
                "unknown pricing strategy variant; using database source"
            );
            false
        }
    };
    tracing::Span::current().record("pricing.strategy", pricing_mode.as_str());
    let key = cache_key(request);
    if use_cache {
        if let Some(result) = state.redis.get(&key).await {
            match result {
                Ok(Some(value)) => match serde_json::from_str::<CachedQuote>(&value) {
                    Ok(quote) => {
                        if quote.consume().is_ok() {
                            let pricing_version =
                                state.postgres.pricing_version(&request.tenant_id).await?;
                            if quote.pricing_version == pricing_version {
                                tracing::Span::current()
                                    .record("pricing.price_list", quote.price_list_code.as_str());
                                tracing::Span::current().record("cache.result", "hit");
                                tracing::info!(
                                    cache = "hit",
                                    price_list = %quote.price_list_code,
                                    "pricing quote cache hit"
                                );
                                record_cache_metric("hit");
                                return Ok(quote);
                            }
                            tracing::warn!(
                                cached_version = %quote.pricing_version,
                                current_version = %pricing_version,
                                "pricing cache entry has an obsolete durable price version"
                            );
                        } else {
                            tracing::info!("pricing quote cache entry expired; refreshing source");
                        }
                    }
                    Err(error) => {
                        tracing::warn!(error = %error, "pricing cache entry was invalid; refreshing source")
                    }
                },
                Ok(None) => tracing::info!(cache = "miss", "pricing quote cache miss"),
                Err(error) => {
                    tracing::warn!(error = %error, "redis unavailable; using postgres pricing source")
                }
            }
            record_cache_metric("miss");
            tracing::Span::current().record("cache.result", "miss");
        }
    } else {
        record_cache_metric("bypass");
        tracing::Span::current().record("cache.result", "bypass");
    }

    let quote = state.postgres.calculate_quote(request).await?;
    tracing::Span::current().record("pricing.price_list", quote.price_list_code.as_str());
    if use_cache && let Some(result) = state.redis.set(&key, &quote).await {
        if let Err(error) = result {
            tracing::warn!(error = %error, "redis quote population failed");
        } else {
            let ttl_seconds = quote
                .remaining_validity_seconds(SystemTime::now())
                .unwrap_or_default();
            tracing::info!(
                ttl_seconds,
                "pricing quote cached"
            );
        }
    }
    Ok(quote)
}

pub(crate) fn pricing_evaluation_context(request: &QuoteRequest) -> EvaluationContext {
    EvaluationContext::default()
        .with_targeting_key(format!("{}:{}", request.tenant_id, request.customer_id))
        .with_custom_field("tenant_id", request.tenant_id.clone())
        .with_custom_field("customer_id", request.customer_id.clone())
        .with_custom_field(
            "customer_segment",
            request
                .context
                .get("customer_segment")
                .cloned()
                .unwrap_or_else(|| "standard".to_owned()),
        )
        .with_custom_field(
            "customer_tier",
            request
                .context
                .get("customer_tier")
                .cloned()
                .unwrap_or_else(|| "free".to_owned()),
        )
        .with_custom_field(
            "region",
            request
                .context
                .get("region")
                .cloned()
                .unwrap_or_else(|| "us-east-1".to_owned()),
        )
        .with_custom_field(
            "request_priority",
            request
                .context
                .get("request_priority")
                .cloned()
                .unwrap_or_else(|| "normal".to_owned()),
        )
}

pub(crate) async fn apply_scenario_delay(
    context: &HashMap<String, String>,
    request_id: &str,
) -> Result<(), Status> {
    if let Some(delay) = context
        .get("delay_ms")
        .and_then(|value| value.parse::<u64>().ok())
    {
        tokio::time::sleep(Duration::from_millis(delay.min(30_000))).await;
    }
    if context
        .get("fault")
        .is_some_and(|fault| fault == "unavailable")
    {
        tracing::warn!(request_id, "pricing deterministic transient fault");
        return Err(Status::unavailable(
            "pricing provider temporarily unavailable",
        ));
    }
    Ok(())
}

fn record_cache_metric(result: &'static str) {
    opentelemetry::global::meter("playground.pricing")
        .u64_counter("pricing.cache.operations")
        .with_description("Pricing Redis cache lookups")
        .build()
        .add(
            1,
            &[
                KeyValue::new("cache.result", result),
                KeyValue::new("cache.system", "redis"),
            ],
        );
}
