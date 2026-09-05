use crate::config::{AppState, Recommend, bounded_limit};
use crate::error::RecommendationError;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use opentelemetry::baggage::{BaggageExt, KeyValueMetadata};
use opentelemetry::{Context, KeyValue};
use serde::Deserialize;
use serde_json::{Value, json};
use tracing::Instrument;

const CATALOG_QUERY: &str = r#"
query RecommendationCatalog($sku: String!, $tenantId: ID!, $segment: String!, $size: Int!) {
  product(sku: $sku, tenantId: $tenantId, segment: $segment) {
    id
    tenantId
    slug
    sku
    name
    description
    brand
    category { id tenantId slug name }
    price { id currency amountMinor compareAtMinor validFrom }
    variants {
      id
      tenantId
      productId
      sku
      name
      options
      price { id currency amountMinor compareAtMinor validFrom }
    }
  }
  products(tenantId: $tenantId, page: 0, size: $size, segment: $segment) {
    items {
      id
      tenantId
      slug
      sku
      name
      description
      brand
      category { id tenantId slug name }
      price { id currency amountMinor compareAtMinor validFrom }
      variants {
        id
        tenantId
        productId
        sku
        name
        options
        price { id currency amountMinor compareAtMinor validFrom }
      }
    }
    page
    size
    totalElements
    totalPages
    hasNext
    experience
  }
}
"#;

#[derive(Debug, Deserialize)]
struct GraphQlResponse {
    data: Option<GraphQlData>,
    #[serde(default)]
    errors: Option<Vec<GraphQlError>>,
}

#[derive(Debug, Deserialize)]
struct GraphQlError {
    message: String,
}

#[derive(Debug, Deserialize)]
struct GraphQlData {
    product: Option<Value>,
    products: CatalogProductPage,
}

#[derive(Debug, Deserialize)]
struct CatalogProductPage {
    items: Vec<Value>,
    #[serde(default)]
    experience: Option<String>,
}

#[derive(Debug)]
pub(crate) struct CatalogLookup {
    pub(crate) product: Value,
    pub(crate) products: Vec<Value>,
    pub(crate) variants: Vec<Value>,
    pub(crate) experience: Option<String>,
}

pub(crate) fn product_sku(product: &Value) -> Option<&str> {
    product.get("sku").and_then(Value::as_str)
}

fn catalog_outbound_context(active: &Context, inbound: &Context) -> Context {
    let inbound = playground_telemetry::sanitize_context(inbound);
    active.with_baggage(inbound.baggage().iter().map(|(key, (value, metadata))| {
        KeyValueMetadata::new(key.clone(), value.clone(), metadata.as_str())
    }))
}

pub(crate) async fn fetch_with_chaos(
    state: &AppState,
    context: &Context,
    params: &Recommend,
    stampede_workers: usize,
) -> Result<CatalogLookup, RecommendationError> {
    if stampede_workers == 0 {
        return fetch(state, context, params).await;
    }

    tracing::warn!(
        sku = %params.sku,
        workers = stampede_workers,
        "bounded Catalog request stampede requested"
    );
    let mut handles = Vec::with_capacity(stampede_workers);
    for worker in 0..stampede_workers {
        let state = state.clone();
        let context = context.clone();
        let worker_sku = params.sku.clone();
        let params = params.clone();
        handles.push(tokio::spawn(
            async move { fetch(&state, &context, &params).await }.instrument(
                tracing::info_span!("catalog_stampede_worker", worker, sku = %worker_sku),
            ),
        ));
    }

    let mut success = None;
    let mut last_error = None;
    for handle in handles {
        match handle.await {
            Ok(Ok(result)) => {
                if success.is_none() {
                    success = Some(result);
                }
            }
            Ok(Err(error)) => last_error = Some(error),
            Err(error) => {
                last_error = Some(RecommendationError::new(
                    StatusCode::BAD_GATEWAY,
                    "catalog_worker_failed",
                    error.to_string(),
                ));
            }
        }
    }
    if let Some(result) = success {
        Ok(result)
    } else if let Some(error) = last_error {
        Err(error)
    } else {
        Err(RecommendationError::new(
            StatusCode::BAD_GATEWAY,
            "catalog_worker_failed",
            "all Catalog request workers failed",
        ))
    }
}

async fn fetch(
    state: &AppState,
    context: &Context,
    params: &Recommend,
) -> Result<CatalogLookup, RecommendationError> {
    let tenant_id = params.tenant_id.as_deref().ok_or_else(|| {
        RecommendationError::new(
            StatusCode::BAD_REQUEST,
            "tenant_required",
            "tenant identity is required",
        )
    })?;
    let span = tracing::info_span!(
        "catalog.graphql.recommendations",
        otel.kind = playground_telemetry::semconv::SPAN_KIND_CLIENT,
        graphql.operation.name = "RecommendationCatalog",
        server.address = %state.catalog_url,
    );
    async move {
        let mut headers = HeaderMap::new();
        let outbound = playground_telemetry::extend_baggage(
            &catalog_outbound_context(&playground_telemetry::current_context(), context),
            [KeyValue::new(
                playground_telemetry::semconv::TENANT_ID,
                tenant_id.to_owned(),
            )],
        );
        playground_telemetry::inject_context_headers(&outbound, &mut headers);
        headers.insert(
            "x-tenant-id",
            HeaderValue::from_str(tenant_id).map_err(|error| {
                RecommendationError::new(
                    StatusCode::BAD_REQUEST,
                    "invalid_tenant",
                    format!("tenant identity cannot be forwarded: {error}"),
                )
            })?,
        );
        let payload = json!({
            "query": CATALOG_QUERY,
            "variables": {
                "sku": params.sku,
                "tenantId": tenant_id,
                "segment": params.segment,
                "size": bounded_limit(params.limit).saturating_add(1).min(crate::config::MAX_LIMIT),
            }
        });
        let response = state
            .http
            .post(&state.catalog_url)
            .headers(headers)
            .json(&payload)
            .send()
            .await
            .map_err(|error| {
                RecommendationError::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "catalog_unavailable",
                    error.to_string(),
                )
            })?;
        let status = response.status();
        let body = response.text().await.map_err(|error| {
            RecommendationError::new(
                StatusCode::BAD_GATEWAY,
                "catalog_protocol_error",
                error.to_string(),
            )
        })?;
        if !status.is_success() {
            return Err(RecommendationError::new(
                StatusCode::BAD_GATEWAY,
                "catalog_http_error",
                format!("Catalog returned {status}"),
            ));
        }
        let envelope: GraphQlResponse = serde_json::from_str(&body).map_err(|error| {
            RecommendationError::new(
                StatusCode::BAD_GATEWAY,
                "catalog_protocol_error",
                error.to_string(),
            )
        })?;
        if let Some(errors) = envelope.errors.filter(|errors| !errors.is_empty()) {
            let message = errors
                .into_iter()
                .map(|error| error.message)
                .collect::<Vec<_>>()
                .join("; ");
            return Err(RecommendationError::new(
                StatusCode::BAD_GATEWAY,
                "catalog_graphql_error",
                message,
            ));
        }
        let data = envelope.data.ok_or_else(|| {
            RecommendationError::new(
                StatusCode::BAD_GATEWAY,
                "catalog_protocol_error",
                "Catalog GraphQL response had no data",
            )
        })?;
        let product = data.product.ok_or_else(|| {
            RecommendationError::new(
                StatusCode::NOT_FOUND,
                "product_not_found",
                format!("no Catalog product for sku {}", params.sku),
            )
        })?;
        let products = data
            .products
            .items
            .into_iter()
            .filter(|candidate| {
                product_sku(candidate).is_some_and(|sku| !sku.eq_ignore_ascii_case(&params.sku))
            })
            .take(bounded_limit(params.limit))
            .collect::<Vec<_>>();
        let variants = products
            .iter()
            .flat_map(|candidate| {
                candidate
                    .get("variants")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .cloned()
            })
            .collect::<Vec<_>>();
        Ok(CatalogLookup {
            product,
            products,
            variants,
            experience: data.products.experience,
        })
    }
    .instrument(span)
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry::KeyValue;
    use opentelemetry::trace::{
        SpanContext, SpanId, TraceContextExt, TraceFlags, TraceId, TraceState,
    };

    #[test]
    fn outbound_context_uses_active_span_and_sanitized_inbound_baggage() {
        let active_trace_id =
            TraceId::from_hex("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").expect("active trace id");
        let active_span_id = SpanId::from_hex("aaaaaaaaaaaaaaaa").expect("active span id");
        let inbound_trace_id =
            TraceId::from_hex("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb").expect("inbound trace id");
        let active = Context::new().with_remote_span_context(SpanContext::new(
            active_trace_id,
            active_span_id,
            TraceFlags::SAMPLED,
            false,
            TraceState::default(),
        ));
        let inbound = Context::new()
            .with_remote_span_context(SpanContext::new(
                inbound_trace_id,
                SpanId::from_hex("bbbbbbbbbbbbbbbb").expect("inbound span id"),
                TraceFlags::SAMPLED,
                true,
                TraceState::default(),
            ))
            .with_baggage([
                KeyValue::new("tenant.id", "tenant-acme"),
                KeyValue::new("secret.token", "must-not-forward"),
            ]);

        let outbound = catalog_outbound_context(&active, &inbound);

        assert_eq!(outbound.span().span_context().trace_id(), active_trace_id);
        assert_eq!(outbound.span().span_context().span_id(), active_span_id);
        assert_eq!(
            outbound.baggage().get("tenant.id").map(ToString::to_string),
            Some("tenant-acme".to_owned())
        );
        assert!(outbound.baggage().get("secret.token").is_none());
    }
}
