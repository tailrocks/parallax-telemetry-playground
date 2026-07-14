//! Juniper GraphQL gateway for the cross-language comparison lane.
//!
//! `catalog_products` forwards GraphQL to the Java catalog (A24), while `quote`
//! calls the pricing gRPC contract (A23). Resolver spans deliberately retain the
//! GraphQL fields Parallax's field-tree view consumes.

use axum::{
    Extension, Router, middleware,
    routing::{get, post},
};
use futures::stream::{BoxStream, StreamExt as _};
use juniper::{
    EmptyMutation, FieldError, FieldResult, RootNode, graphql_object, graphql_subscription,
};
use juniper_axum::{graphiql, graphql, subscriptions};
use juniper_graphql_ws::ConnectionConfig;
use playground_proto::pricing::v1::{QuoteRequest, pricing_client::PricingClient};
use std::sync::Arc;
use std::time::Duration;
use tokio_stream::wrappers::IntervalStream;
use tonic::transport::Endpoint;
use tracing::Instrument;

#[derive(Clone, Debug)]
struct StoreContext {
    catalog_url: String,
    pricing_endpoint: String,
}

impl Default for StoreContext {
    fn default() -> Self {
        Self {
            catalog_url: std::env::var("CATALOG_GRAPHQL_URL")
                .unwrap_or_else(|_| "http://catalog:8080/graphql".to_owned()),
            pricing_endpoint: std::env::var("PRICING_ENDPOINT")
                .unwrap_or_else(|_| "http://pricing:50051".to_owned()),
        }
    }
}

impl juniper::Context for StoreContext {}

#[derive(Clone, Debug)]
struct Product {
    sku: String,
    name: String,
    price_minor: i32,
}

#[graphql_object(context = StoreContext)]
impl Product {
    fn sku(&self) -> &str {
        &self.sku
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn price_minor(&self) -> i32 {
        self.price_minor
    }

    /// Deliberately independent per-product work to preserve the N+1 shape.
    async fn related_sku(&self) -> String {
        resolver_span("Product.relatedSku", "Product.relatedSku", "field")
            .in_scope(|| format!("{}-ACCESSORY", self.sku))
    }

    /// Mirrors catalog's partial-error comparison shape without failing the
    /// complete products operation.
    fn risk_score(&self) -> FieldResult<f64> {
        let _guard = resolver_span("Product.riskScore", "Product.riskScore", "field").entered();
        if self.sku == "PARTIAL-ERROR" {
            return Err(FieldError::new(
                "risk score unavailable",
                juniper::Value::null(),
            ));
        }
        Ok(if self.price_minor > 3000 { 0.72 } else { 0.18 })
    }
}

#[derive(Clone, Debug)]
struct Quote {
    sku: String,
    quantity: i32,
    total_minor: String,
    currency: String,
}

#[graphql_object]
impl Quote {
    fn sku(&self) -> &str {
        &self.sku
    }
    fn quantity(&self) -> i32 {
        self.quantity
    }
    fn total_minor(&self) -> &str {
        &self.total_minor
    }
    fn currency(&self) -> &str {
        &self.currency
    }
}

struct Query;

#[graphql_object(context = StoreContext)]
impl Query {
    async fn catalog_products(context: &StoreContext) -> FieldResult<Vec<Product>> {
        let span = resolver_span("Query.catalogProducts", "Query.catalogProducts", "query");
        async move {
            let mut headers = reqwest::header::HeaderMap::new();
            playground_telemetry::inject_headers(&mut headers);
            let payload = serde_json::json!({"query": "query StorefrontCatalog { products { sku name priceMinor } }", "operationName": "StorefrontCatalog"});
            let response = reqwest::Client::new()
                .post(&context.catalog_url)
                .headers(headers)
                .json(&payload)
                .send()
                .await
                .map_err(field_error)?;
            let value: serde_json::Value = response.json().await.map_err(field_error)?;
            let products = value
                .pointer("/data/products")
                .and_then(serde_json::Value::as_array)
                .ok_or_else(|| FieldError::new("catalog response missing products", juniper::Value::null()))?;
            Ok(products.iter().map(product_from_json).collect())
        }
        .instrument(span)
        .await
    }

    async fn quote(context: &StoreContext, sku: String, quantity: i32) -> FieldResult<Quote> {
        let span = resolver_span("Query.quote", "Query.quote", "query");
        async move {
            let channel = Endpoint::from_shared(context.pricing_endpoint.clone())
                .map_err(field_error)?
                .connect()
                .await
                .map_err(field_error)?;
            let mut client = PricingClient::new(channel);
            let mut request = tonic::Request::new(QuoteRequest {
                sku,
                quantity: quantity.max(1) as u32,
                delay_ms: 0,
                fail_at: 0,
            });
            playground_telemetry::inject_grpc_metadata(request.metadata_mut());
            let quote = client
                .quote(request)
                .await
                .map_err(field_error)?
                .into_inner();
            Ok(Quote {
                sku: quote.sku,
                quantity: quote.quantity as i32,
                total_minor: quote.total_minor.to_string(),
                currency: quote.currency,
            })
        }
        .instrument(span)
        .await
    }
}

struct Subscription;
type QuoteStream = BoxStream<'static, Result<Quote, FieldError>>;

#[graphql_subscription(context = StoreContext)]
impl Subscription {
    async fn price_ticks() -> QuoteStream {
        let mut index = 0_i32;
        Box::pin(
            IntervalStream::new(tokio::time::interval(Duration::from_secs(1))).map(move |_| {
                index += 1;
                Ok(Quote {
                    sku: "WIDGET-1".to_owned(),
                    quantity: index,
                    total_minor: (1999 * index).to_string(),
                    currency: "USD".to_owned(),
                })
            }),
        )
    }
}

type Schema = RootNode<Query, EmptyMutation<StoreContext>, Subscription>;

fn resolver_span(name: &'static str, path: &'static str, operation: &'static str) -> tracing::Span {
    tracing::info_span!(
        "graphql.resolver",
        otel.kind = playground_telemetry::semconv::SPAN_KIND_INTERNAL,
        graphql.operation.type = operation,
        graphql.operation.name = "Storefront",
        graphql.document = "storefront-gateway",
        graphql.field.name = name,
        graphql.field.path = path,
    )
}

fn product_from_json(value: &serde_json::Value) -> Product {
    Product {
        sku: value
            .get("sku")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown")
            .to_owned(),
        name: value
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown")
            .to_owned(),
        price_minor: value
            .get("priceMinor")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or_default() as i32,
    }
}

fn field_error(error: impl std::fmt::Display) -> FieldError {
    playground_telemetry::mark_span_error("storefront_upstream_error");
    FieldError::new(error.to_string(), juniper::Value::null())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let telemetry = playground_telemetry::init("storefront")?;
    let schema = Arc::new(Schema::new(Query, EmptyMutation::new(), Subscription));
    let context = StoreContext::default();
    let app = Router::new()
        .route("/graphql", post(graphql::<Arc<Schema>>))
        .route(
            "/subscriptions",
            get(subscriptions::ws::<Arc<Schema>>(ConnectionConfig::new(
                context,
            ))),
        )
        .route("/graphiql", get(graphiql("/graphql", "/subscriptions")))
        .layer(Extension(schema))
        .layer(middleware::from_fn(
            playground_telemetry::http_server_observability,
        ));
    let addr = std::env::var("STOREFRONT_ADDR").unwrap_or_else(|_| "0.0.0.0:8094".to_owned());
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!(%addr, "storefront GraphQL ready: /graphql, /subscriptions, /graphiql");
    axum::serve(listener, app).await?;
    telemetry.shutdown();
    Ok(())
}
