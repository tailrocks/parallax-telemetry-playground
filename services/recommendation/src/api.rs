use crate::catalog::{fetch_with_chaos, product_sku};
use crate::chaos::retain_bounded_memory;
use crate::config::{
    AppState, CatalogHealthResponse, CatalogReadinessResponse, READINESS_QUERY,
    READINESS_TENANT_ID, Recommend, bounded_delay_ms, bounded_stampede, catalog_readiness_url,
};
use crate::error::RecommendationError;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::{Json, Router, routing::get};
use opentelemetry::Context;
use playground_telemetry::semconv;
use serde_json::{Value, json};
use std::time::Duration;
use tracing::Instrument;

pub(crate) fn app_with_state(state: AppState) -> Router {
    Router::new()
        .route("/recommend", get(recommend))
        .route("/healthz", get(|| async { "ok" }))
        .route("/readyz", get(readiness))
        .with_state(state)
        .layer(axum::middleware::from_fn(
            playground_telemetry::http_server_observability,
        ))
}

async fn recommend(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(mut params): Query<Recommend>,
) -> Result<Json<Value>, RecommendationError> {
    let parent = playground_telemetry::extract_context(&headers);
    let tenant_id =
        playground_telemetry::resolve_http_tenant_identity(&headers, params.tenant_id.as_deref())
            .map_err(tenant_identity_error)?;
    params.tenant_id = Some(tenant_id.clone());
    let span = tracing::info_span!(
        "recommend",
        otel.kind = semconv::SPAN_KIND_SERVER,
        recommendation.sku = %params.sku,
        tenant.id = %tenant_id,
        customer.segment = %params.segment,
    );
    playground_telemetry::set_parent_from_headers(&span, &headers);
    playground_telemetry::stamp_business_baggage(&span, &parent);
    recommend_inner(state, params, parent)
        .instrument(span)
        .await
}

async fn recommend_inner(
    state: AppState,
    params: Recommend,
    context: Context,
) -> Result<Json<Value>, RecommendationError> {
    let tenant_id = params.tenant_id.as_deref().ok_or_else(|| {
        RecommendationError::new(
            StatusCode::BAD_REQUEST,
            "tenant_required",
            "tenant identity is required",
        )
    })?;
    if params.sku.trim().is_empty() {
        return Err(RecommendationError::new(
            StatusCode::BAD_REQUEST,
            "invalid_sku",
            "sku must not be empty",
        ));
    }
    if params.limit == 0 {
        return Err(RecommendationError::new(
            StatusCode::BAD_REQUEST,
            "invalid_limit",
            "limit must be greater than zero",
        ));
    }

    let slow_ms = bounded_delay_ms(params.slow);
    if slow_ms > 0 {
        tokio::time::sleep(Duration::from_millis(slow_ms)).await;
    }

    let leak_flag = if params.leak > 0 {
        playground_telemetry::feature_flag("cacheLeak", "CACHE_LEAK").await
    } else {
        false
    };
    let retained_leak_kb = retain_bounded_memory(params.leak, leak_flag);
    let stampede_workers = bounded_stampede(params.stampede);
    let lookup = fetch_with_chaos(&state, &context, &params, stampede_workers).await?;

    let recommended = lookup
        .products
        .iter()
        .filter_map(product_sku)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    tracing::info!(
        sku = %params.sku,
        count = recommended.len(),
        catalog_products = lookup.products.len(),
        catalog_variants = lookup.variants.len(),
        stampede_workers,
        "catalog-backed recommendations returned"
    );

    Ok(Json(json!({
        "sku": params.sku,
        "tenant_id": tenant_id,
        "segment": params.segment,
        "source": "catalog-graphql",
        "experience": lookup.experience,
        "product": lookup.product,
        "products": lookup.products,
        "variants": lookup.variants,
        "recommended": recommended,
        "chaos": {
            "slow_ms": slow_ms,
            "leak_kb": retained_leak_kb,
            "stampede_workers": stampede_workers,
        }
    })))
}

fn tenant_identity_error(error: playground_telemetry::TenantIdentityError) -> RecommendationError {
    let (status, code) = match error {
        playground_telemetry::TenantIdentityError::Missing => {
            (StatusCode::BAD_REQUEST, "tenant_required")
        }
        playground_telemetry::TenantIdentityError::Invalid => {
            (StatusCode::BAD_REQUEST, "invalid_tenant")
        }
        playground_telemetry::TenantIdentityError::Conflicting => {
            (StatusCode::CONFLICT, "identity_conflict")
        }
    };
    RecommendationError::new(status, code, error.to_string())
}

async fn readiness(State(state): State<AppState>) -> impl IntoResponse {
    match check_catalog_readiness(&state).await {
        Ok(()) => (
            StatusCode::OK,
            Json(json!({"status": "UP", "catalog": "UP"})),
        ),
        Err(error) => {
            let mut body = json!({
                "status": "DOWN",
                "catalog": "DOWN",
                "error": error.message,
            });
            if let Some(http_status) = error.http_status {
                body["http_status"] = json!(http_status);
            }
            (StatusCode::SERVICE_UNAVAILABLE, Json(body))
        }
    }
}

#[derive(Debug)]
struct ReadinessFailure {
    message: String,
    http_status: Option<u16>,
}

impl ReadinessFailure {
    fn message(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            http_status: None,
        }
    }

    fn http(status: reqwest::StatusCode, message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            http_status: Some(status.as_u16()),
        }
    }
}

async fn check_catalog_readiness(state: &AppState) -> Result<(), ReadinessFailure> {
    let health_url = catalog_readiness_url(&state.catalog_url)
        .map_err(|error| ReadinessFailure::message(error.to_string()))?;
    let health_response = state.http.get(health_url).send().await.map_err(|error| {
        ReadinessFailure::message(format!("Catalog readiness request failed: {error}"))
    })?;
    let health_status = health_response.status();
    let health_body = health_response.text().await.map_err(|error| {
        ReadinessFailure::message(format!("read Catalog readiness response: {error}"))
    })?;
    if !health_status.is_success() {
        return Err(ReadinessFailure::http(
            health_status,
            format!("Catalog readiness returned HTTP {health_status}"),
        ));
    }
    let health: CatalogHealthResponse = serde_json::from_str(&health_body).map_err(|error| {
        ReadinessFailure::message(format!("parse Catalog readiness response: {error}"))
    })?;
    if health.status != "UP" {
        return Err(ReadinessFailure::message(format!(
            "Catalog readiness status is {}",
            health.status
        )));
    }
    // Catalog's readiness group includes `db`; Spring may hide component details
    // unless explicitly enabled, so the group status is authoritative when absent.
    // When details are exposed, require the database component explicitly.
    if let Some(components) = health.components.as_ref() {
        let Some(db) = components.db.as_ref() else {
            return Err(ReadinessFailure::message(
                "Catalog readiness omitted the database component",
            ));
        };
        if db.status != "UP" {
            return Err(ReadinessFailure::message(
                "Catalog database readiness is not UP",
            ));
        }
    }

    let graphql_response = state
        .http
        .post(&state.catalog_url)
        // Catalog treats the GraphQL argument as a requested tenant and requires
        // an independent caller identity header before executing the resolver.
        .header("x-tenant-id", READINESS_TENANT_ID)
        .header("Cache-Control", "no-store")
        .json(&json!({
            "query": READINESS_QUERY,
            "operationName": "Readiness",
            "variables": {"tenantId": READINESS_TENANT_ID},
        }))
        .send()
        .await
        .map_err(|error| {
            ReadinessFailure::message(format!("Catalog GraphQL readiness request failed: {error}"))
        })?;
    let graphql_status = graphql_response.status();
    let graphql_body = graphql_response.text().await.map_err(|error| {
        ReadinessFailure::message(format!("read Catalog GraphQL readiness response: {error}"))
    })?;
    if !graphql_status.is_success() {
        return Err(ReadinessFailure::http(
            graphql_status,
            format!("Catalog GraphQL readiness returned HTTP {graphql_status}"),
        ));
    }
    let envelope: CatalogReadinessResponse =
        serde_json::from_str(&graphql_body).map_err(|error| {
            ReadinessFailure::message(format!("parse Catalog GraphQL response: {error}"))
        })?;
    if let Some(errors) = envelope.errors.filter(|errors| !errors.is_empty()) {
        let message = errors
            .into_iter()
            .map(|error| error.message)
            .collect::<Vec<_>>()
            .join("; ");
        return Err(ReadinessFailure::message(format!(
            "Catalog GraphQL readiness returned errors: {message}"
        )));
    }
    let data = envelope
        .data
        .ok_or_else(|| ReadinessFailure::message("Catalog GraphQL readiness returned null data"))?;
    let Some(product) = data.product else {
        return Err(ReadinessFailure::message(
            "Catalog GraphQL readiness returned no WIDGET-1 product",
        ));
    };
    if product.id.trim().is_empty()
        || product.tenant_id != READINESS_TENANT_ID
        || product.sku != "WIDGET-1"
    {
        return Err(ReadinessFailure::message(
            "Catalog GraphQL readiness returned an invalid Product contract",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Json,
        body::{Body, to_bytes},
        http::{Request, StatusCode},
        routing::{get, post},
    };
    use opentelemetry::global;
    use opentelemetry::propagation::TextMapCompositePropagator;
    use opentelemetry::trace::{
        SpanContext, SpanId, TraceContextExt, TraceFlags, TraceId, TraceState,
    };
    use opentelemetry_sdk::propagation::{BaggagePropagator, TraceContextPropagator};
    use serde_json::json;
    use std::sync::{Arc, Mutex};
    use tower::ServiceExt;

    fn test_state(url: String) -> AppState {
        AppState::new(url).expect("build test state")
    }

    async fn spawn_catalog_mock(
        graphql_response: Value,
    ) -> (
        String,
        tokio::task::JoinHandle<()>,
        Arc<Mutex<Option<Value>>>,
        Arc<Mutex<Option<HeaderMap>>>,
    ) {
        spawn_catalog_mock_with_health(
            graphql_response,
            json!({
                "status": "UP",
                "components": {"db": {"status": "UP"}}
            }),
        )
        .await
    }

    async fn spawn_catalog_mock_with_health(
        graphql_response: Value,
        health_response: Value,
    ) -> (
        String,
        tokio::task::JoinHandle<()>,
        Arc<Mutex<Option<Value>>>,
        Arc<Mutex<Option<HeaderMap>>>,
    ) {
        let captured_request = Arc::new(Mutex::new(None));
        let captured_for_handler = Arc::clone(&captured_request);
        let captured_headers = Arc::new(Mutex::new(None));
        let captured_headers_for_handler = Arc::clone(&captured_headers);
        let response_for_handler = graphql_response.clone();
        let health_for_handler = health_response.clone();
        let mock = Router::new()
            .route(
                "/actuator/health/readiness",
                get(move || {
                    let health = health_for_handler.clone();
                    async move { Json(health) }
                }),
            )
            .route(
                "/graphql",
                post(move |headers: HeaderMap, Json(body): Json<Value>| {
                    let captured = Arc::clone(&captured_for_handler);
                    let captured_headers = Arc::clone(&captured_headers_for_handler);
                    let response = response_for_handler.clone();
                    async move {
                        *captured.lock().expect("capture request lock") = Some(body);
                        *captured_headers.lock().expect("capture headers lock") =
                            Some(headers.clone());
                        if headers
                            .get("x-tenant-id")
                            .and_then(|value| value.to_str().ok())
                            != Some(READINESS_TENANT_ID)
                        {
                            return (
                                StatusCode::BAD_REQUEST,
                                Json(json!({"error": "missing readiness tenant identity"})),
                            );
                        }
                        (StatusCode::OK, Json(response))
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind Catalog mock");
        let address = listener.local_addr().expect("Catalog mock address");
        let server = tokio::spawn(async move {
            axum::serve(listener, mock)
                .await
                .expect("serve Catalog mock");
        });
        (
            format!("http://{address}"),
            server,
            captured_request,
            captured_headers,
        )
    }

    async fn request_readiness(state: AppState) -> (StatusCode, Value) {
        let response = app_with_state(state)
            .oneshot(
                Request::builder()
                    .uri("/readyz")
                    .body(Body::empty())
                    .expect("readiness request"),
            )
            .await
            .expect("readiness response");
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("readiness body");
        (
            status,
            serde_json::from_slice(&body).expect("readiness JSON body"),
        )
    }

    #[test]
    fn chaos_controls_are_bounded() {
        assert_eq!(crate::config::bounded_limit(0), 1);
        assert_eq!(
            crate::config::bounded_limit(crate::config::MAX_LIMIT + 1),
            crate::config::MAX_LIMIT
        );
        assert_eq!(
            bounded_delay_ms(crate::config::MAX_CHAOS_DELAY_MS + 1),
            crate::config::MAX_CHAOS_DELAY_MS
        );
        assert_eq!(
            bounded_stampede(crate::config::MAX_STAMPEDE + 1),
            crate::config::MAX_STAMPEDE
        );
        assert_eq!(retain_bounded_memory(0, false), 0);
        assert!(
            retain_bounded_memory(crate::config::MAX_CHAOS_LEAK_KB_PER_REQUEST + 1, false)
                <= crate::config::MAX_CHAOS_LEAK_KB_PER_REQUEST
        );
    }

    #[tokio::test]
    async fn exposes_health_and_rejects_missing_sku() {
        let state = test_state("http://127.0.0.1:9/graphql".to_owned());
        let health = app_with_state(state.clone())
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .expect("health request"),
            )
            .await
            .expect("health response");
        assert_eq!(health.status(), StatusCode::OK);

        let readiness = app_with_state(state.clone())
            .oneshot(
                Request::builder()
                    .uri("/readyz")
                    .body(Body::empty())
                    .expect("readiness request"),
            )
            .await
            .expect("readiness response");
        assert_eq!(readiness.status(), StatusCode::SERVICE_UNAVAILABLE);

        let missing_sku = app_with_state(state)
            .oneshot(
                Request::builder()
                    .uri("/recommend")
                    .body(Body::empty())
                    .expect("recommend request"),
            )
            .await
            .expect("recommend response");
        assert_eq!(missing_sku.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn readiness_executes_catalog_health_and_graphql_contract() {
        let (base_url, server, captured_request, captured_headers) = spawn_catalog_mock(json!({
            "data": {"product": {"id": "product-1", "tenantId": "tenant-acme", "sku": "WIDGET-1"}}
        }))
        .await;

        let (status, body) = request_readiness(test_state(format!("{base_url}/graphql"))).await;
        server.abort();

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "UP");
        let request = captured_request
            .lock()
            .expect("captured request lock")
            .clone()
            .expect("readiness GraphQL request");
        let query = request["query"].as_str().expect("readiness query");
        assert!(query.contains("query Readiness($tenantId: ID!)"));
        assert!(
            query
                .contains("product(sku: \"WIDGET-1\", tenantId: $tenantId, segment: \"standard\")")
        );
        assert!(!query.contains("__typename"));
        assert_eq!(request["operationName"], "Readiness");
        assert_eq!(request["variables"]["tenantId"], READINESS_TENANT_ID);
        let headers = captured_headers
            .lock()
            .expect("captured headers lock")
            .clone()
            .expect("readiness GraphQL headers");
        assert_eq!(
            headers
                .get("cache-control")
                .and_then(|value| value.to_str().ok()),
            Some("no-store")
        );
    }

    #[tokio::test]
    async fn readiness_rejects_http_success_with_graphql_errors() {
        let (base_url, server, _, _) = spawn_catalog_mock(json!({
            "data": null,
            "errors": [{"message": "database connection refused"}]
        }))
        .await;

        let (status, body) = request_readiness(test_state(format!("{base_url}/graphql"))).await;
        server.abort();

        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["status"], "DOWN");
        assert_eq!(body["catalog"], "DOWN");
        assert!(
            body["error"]
                .as_str()
                .expect("readiness error")
                .contains("database connection refused")
        );
    }

    #[tokio::test]
    async fn readiness_rejects_null_graphql_data() {
        let (base_url, server, _, _) = spawn_catalog_mock(json!({"data": null})).await;

        let (status, body) = request_readiness(test_state(format!("{base_url}/graphql"))).await;
        server.abort();

        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["status"], "DOWN");
        assert!(
            body["error"]
                .as_str()
                .expect("readiness error")
                .contains("null data")
        );
    }

    #[tokio::test]
    async fn readiness_rejects_catalog_with_database_not_ready() {
        let (base_url, server, _, _) = spawn_catalog_mock_with_health(
            json!({
                "data": {"product": {"id": "product-1", "tenantId": "tenant-acme", "sku": "WIDGET-1"}}
            }),
            json!({
                "status": "UP",
                "components": {"db": {"status": "DOWN"}}
            }),
        )
        .await;

        let (status, body) = request_readiness(test_state(format!("{base_url}/graphql"))).await;
        server.abort();

        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["status"], "DOWN");
        assert!(
            body["error"]
                .as_str()
                .expect("readiness error")
                .contains("database readiness")
        );
    }

    #[tokio::test]
    async fn readiness_reports_unavailable_catalog() {
        let (status, body) =
            request_readiness(test_state("http://127.0.0.1:9/graphql".to_owned())).await;

        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["status"], "DOWN");
        assert_eq!(body["catalog"], "DOWN");
    }

    #[tokio::test]
    async fn rejects_missing_and_conflicting_tenant_identity() {
        let missing = app_with_state(test_state("http://127.0.0.1:9/graphql".to_owned()))
            .oneshot(
                Request::builder()
                    .uri("/recommend?sku=WIDGET-1")
                    .body(Body::empty())
                    .expect("missing-tenant request"),
            )
            .await
            .expect("missing-tenant response");
        assert_eq!(missing.status(), StatusCode::BAD_REQUEST);

        let conflicting = app_with_state(test_state("http://127.0.0.1:9/graphql".to_owned()))
            .oneshot(
                Request::builder()
                    .uri("/recommend?sku=WIDGET-1&tenant_id=tenant-acme")
                    .header("x-tenant-id", "tenant-nova")
                    .body(Body::empty())
                    .expect("conflicting-tenant request"),
            )
            .await
            .expect("conflicting-tenant response");
        assert_eq!(conflicting.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn direct_query_tenant_identity_reaches_catalog_headers_and_baggage() {
        let captured_headers = Arc::new(Mutex::new(HeaderMap::new()));
        let captured_for_server = Arc::clone(&captured_headers);
        let mock = Router::new().route(
            "/graphql",
            post(move |headers: HeaderMap, Json(_body): Json<Value>| {
                let captured = Arc::clone(&captured_for_server);
                async move {
                    *captured.lock().expect("capture headers lock") = headers.clone();
                    let has_identity = headers
                        .get("x-tenant-id")
                        .and_then(|value| value.to_str().ok())
                        == Some("tenant-acme")
                        && headers
                            .get("baggage")
                            .and_then(|value| value.to_str().ok())
                            .is_some_and(|value| value.contains("tenant.id=tenant-acme"));
                    if has_identity {
                        (
                            StatusCode::OK,
                            Json(json!({
                                "data": {
                                    "product": {"sku": "WIDGET-1"},
                                    "products": {"items": [], "experience": "featured"}
                                }
                            })),
                        )
                    } else {
                        (
                            StatusCode::BAD_REQUEST,
                            Json(json!({"error": "tenant identity was not forwarded"})),
                        )
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind Catalog mock");
        let address = listener.local_addr().expect("Catalog mock address");
        let server = tokio::spawn(async move {
            axum::serve(listener, mock)
                .await
                .expect("serve Catalog mock");
        });

        let response = app_with_state(test_state(format!("http://{address}/graphql")))
            .oneshot(
                Request::builder()
                    .uri("/recommend?sku=WIDGET-1&tenant_id=tenant-acme&segment=returning")
                    .body(Body::empty())
                    .expect("direct recommendation request"),
            )
            .await
            .expect("direct recommendation response");
        server.abort();

        assert_eq!(response.status(), StatusCode::OK);
        let headers = captured_headers.lock().expect("capture headers lock");
        assert_eq!(
            headers
                .get("x-tenant-id")
                .and_then(|value| value.to_str().ok()),
            Some("tenant-acme")
        );
        assert!(
            headers
                .get("baggage")
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value.contains("tenant.id=tenant-acme"))
        );
    }

    #[tokio::test]
    async fn queries_catalog_returns_variants_and_forwards_w3c_context() {
        global::set_text_map_propagator(TextMapCompositePropagator::new(vec![
            Box::new(TraceContextPropagator::new()),
            Box::new(BaggagePropagator::new()),
        ]));
        let captured_headers = Arc::new(Mutex::new(HeaderMap::new()));
        let captured_for_server = Arc::clone(&captured_headers);
        let mock = Router::new().route(
            "/graphql",
            post(move |headers: HeaderMap, Json(_body): Json<Value>| {
                let captured = Arc::clone(&captured_for_server);
                async move {
                    *captured.lock().expect("capture headers lock") = headers;
                    Json(json!({
                        "data": {
                            "product": {
                                "sku": "WIDGET-1",
                                "variants": [{"sku": "WIDGET-1-BLUE"}]
                            },
                            "products": {
                                "items": [
                                    {"sku": "WIDGET-1", "variants": [{"sku": "WIDGET-1-BLUE"}]},
                                    {"sku": "GADGET-1", "variants": [{"sku": "GADGET-1-RED"}]}
                                ],
                                "experience": "featured"
                            }
                        }
                    }))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind Catalog mock");
        let address = listener.local_addr().expect("Catalog mock address");
        let server = tokio::spawn(async move {
            axum::serve(listener, mock)
                .await
                .expect("serve Catalog mock");
        });

        let trace_id = TraceId::from_hex("4bf92f3577b34da6a3ce929d0e0e4736").expect("trace id");
        let span_context = SpanContext::new(
            trace_id,
            SpanId::from_hex("00f067aa0ba902b7").expect("span id"),
            TraceFlags::SAMPLED,
            true,
            TraceState::default(),
        );
        let context = playground_telemetry::with_business_context(
            &Context::new().with_remote_span_context(span_context),
            "tenant-acme",
            "pro",
            "returning",
            "us-east-1",
            "normal",
        );
        let result = recommend_inner(
            test_state(format!("http://{address}/graphql")),
            Recommend {
                sku: "WIDGET-1".to_owned(),
                tenant_id: Some("tenant-acme".to_owned()),
                segment: "returning".to_owned(),
                limit: 8,
                slow: 0,
                leak: 0,
                stampede: 0,
            },
            context,
        )
        .await
        .expect("catalog-backed recommendation");
        server.abort();

        let body = result.0;
        assert_eq!(body["source"], "catalog-graphql");
        assert_eq!(body["recommended"], json!(["GADGET-1"]));
        assert_eq!(body["variants"][0]["sku"], "GADGET-1-RED");
        assert_eq!(body["chaos"]["stampede_workers"], 0);

        let headers = captured_headers.lock().expect("capture headers lock");
        assert_eq!(
            headers
                .get("x-tenant-id")
                .and_then(|value| value.to_str().ok()),
            Some("tenant-acme")
        );
        assert!(
            headers.get("traceparent").is_none(),
            "the stale inbound traceparent must not be forwarded without an active client span"
        );
        let baggage = headers
            .get("baggage")
            .and_then(|value| value.to_str().ok())
            .expect("baggage header");
        assert!(baggage.contains("tenant.id=tenant-acme"));
        assert!(baggage.contains("user.tier=pro"));
        assert!(baggage.contains("customer.segment=returning"));
    }
}
