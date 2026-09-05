//! External service clients, durable journal adapters, and wire DTOs.

use anyhow::{Context as _, anyhow};
use axum::{Json, http::StatusCode};
use futures::future::poll_fn;
use opentelemetry::{Context as OtelContext, baggage::BaggageExt};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{future::Future, time::Duration};
use tokio_postgres::{AsyncMessage, Client};

pub(crate) const DEFAULT_CATALOG_URL: &str = "http://catalog:8080/graphql";
pub(crate) const DEFAULT_PRICING_ENDPOINT: &str = "http://pricing:50051";
pub(crate) const DEFAULT_CHECKOUT_ENDPOINT: &str = "http://checkout:8088";
pub(crate) const DEFAULT_CLICKHOUSE_HTTP_URL: &str = "http://clickhouse:8123";
pub(crate) const DEFAULT_DATABASE_URL: &str =
    "postgres://postgres:playground@localhost:5432/playground";
pub(crate) const DEFAULT_WEB_ORIGIN: &str = "http://localhost:5173";
pub(crate) const MAX_CART_ITEM_QUANTITY: i32 = 100;
pub(crate) const PRICE_CHANGE_CHANNEL: &str = "price_changes";
pub(crate) const PRICE_CHANGE_RECONNECT_DELAY: Duration = Duration::from_millis(250);
pub(crate) const DEPENDENCY_READY_TIMEOUT: Duration = Duration::from_secs(2);
pub(crate) const STOREFRONT_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
pub(crate) const DOWNSTREAM_REQUEST_TIMEOUT: Duration = Duration::from_secs(4);
pub(crate) const HTTP_CONNECT_TIMEOUT: Duration = Duration::from_millis(500);
pub(crate) const HTTP_READ_TIMEOUT: Duration = Duration::from_secs(4);
pub(crate) const GRPC_CONNECT_TIMEOUT: Duration = Duration::from_millis(500);
pub(crate) const CORS_ALLOWED_METHODS: &str = "GET, POST, OPTIONS";
pub(crate) const CORS_ALLOWED_HEADERS: &str =
    "content-type, traceparent, tracestate, baggage, sentry-trace";
pub(crate) const CORS_ALLOWED_HEADER_NAMES: [&str; 5] = [
    "content-type",
    "traceparent",
    "tracestate",
    "baggage",
    "sentry-trace",
];
pub(crate) const CORS_MAX_AGE_SECONDS: &str = "600";
pub(crate) const CATALOG_PRODUCT_QUERY: &str = r#"
query StorefrontProduct($sku: String!, $tenantId: ID!, $segment: String!) {
  product(sku: $sku, tenantId: $tenantId, segment: $segment) {
    id tenantId slug sku name description brand priceMinor
    category { id tenantId slug name }
    price { id currency amountMinor compareAtMinor validFrom }
    variants {
      id tenantId productId sku name options
      price { id currency amountMinor compareAtMinor validFrom }
    }
    reviews { id productId title text stars verifiedPurchase createdAt }
    riskScore
  }
}
"#;
pub(crate) const CATALOG_PRODUCTS_QUERY: &str = r#"
query StorefrontProducts(
  $search: String,
  $category: String,
  $sort: ProductSort,
  $tenantId: ID!,
  $page: Int!,
  $size: Int!,
  $segment: String!
) {
  products(
    search: $search,
    category: $category,
    sort: $sort,
    tenantId: $tenantId,
    page: $page,
    size: $size,
    segment: $segment
  ) {
    items {
      id tenantId slug sku name description brand priceMinor
      category { id tenantId slug name }
      price { id currency amountMinor compareAtMinor validFrom }
      variants {
        id tenantId productId sku name options
        price { id currency amountMinor compareAtMinor validFrom }
      }
      reviews { id productId title text stars verifiedPurchase createdAt }
      riskScore
    }
    page size totalElements totalPages hasNext experience
  }
}
"#;
pub(crate) const CATALOG_CATEGORIES_QUERY: &str = r#"
query StorefrontCategories($tenantId: ID!) {
  categories(tenantId: $tenantId) { id tenantId slug name }
}
"#;
pub(crate) const ANALYTICS_INSERT_QUERY: &str = r#"
INSERT INTO analytics.analytics_events
(
  event_id, tenant_id, event_key, customer_id, session_id, event_name, event_version,
  source, entity_type, entity_id, occurred_at, trace_id, span_id, traceparent,
  tracestate, baggage, feature_variant, properties, context
)
SELECT
  {event_id:UUID},
  {tenant_id:String},
  {event_key:String},
  nullIf({customer_id:String}, ''),
  nullIf({session_id:String}, ''),
  {event_name:String},
  {event_version:UInt16},
  {source:String},
  {entity_type:String},
  {entity_id:String},
  parseDateTime64BestEffort({occurred_at:String}, 3, 'UTC'),
  {trace_id:String},
  {span_id:String},
  {traceparent:String},
  {tracestate:String},
  {baggage:String},
  {feature_variant:String},
  {properties:String},
  {context:String}
"#;

#[derive(Debug)]
pub(crate) struct DependencyReadiness {
    pub(crate) catalog: bool,
    pub(crate) pricing: bool,
    pub(crate) checkout: bool,
    pub(crate) postgres: bool,
}

impl DependencyReadiness {
    pub(crate) fn is_ready(&self) -> bool {
        self.catalog && self.pricing && self.checkout && self.postgres
    }

    pub(crate) fn into_response(self) -> (StatusCode, Json<Value>) {
        let ready = self.is_ready();
        let status = if ready { "UP" } else { "DOWN" };
        (
            if ready {
                StatusCode::OK
            } else {
                StatusCode::SERVICE_UNAVAILABLE
            },
            Json(json!({
                "status": status,
                "dependencies": {
                    "catalog": if self.catalog { "UP" } else { "DOWN" },
                    "pricing": if self.pricing { "UP" } else { "DOWN" },
                    "checkout": if self.checkout { "UP" } else { "DOWN" },
                    "postgres": if self.postgres { "UP" } else { "DOWN" },
                },
            })),
        )
    }
}

#[derive(Debug)]
pub(crate) struct AnalyticsReadiness {
    pub(crate) clickhouse: bool,
}

impl AnalyticsReadiness {
    pub(crate) fn into_response(self) -> (StatusCode, Json<Value>) {
        let ready = self.clickhouse;
        let status = if ready { "UP" } else { "DOWN" };
        (
            if ready {
                StatusCode::OK
            } else {
                StatusCode::SERVICE_UNAVAILABLE
            },
            Json(json!({
                "status": status,
                "dependencies": {
                    "clickhouse": status,
                },
            })),
        )
    }
}

pub(crate) type PriceChangeConnection =
    tokio_postgres::Connection<tokio_postgres::Socket, tokio_postgres::tls::NoTlsStream>;

pub(crate) struct PriceChangeListener {
    pub(crate) client: Client,
    connection: PriceChangeConnection,
}

impl PriceChangeListener {
    pub(crate) async fn connect(database_url: &str) -> anyhow::Result<Self> {
        let (client, mut connection) = tokio_postgres::connect(database_url, tokio_postgres::NoTls)
            .await
            .context("price change LISTEN connection failed")?;
        drive_listener_operation(
            &mut connection,
            client.batch_execute("LISTEN price_changes"),
        )
        .await
        .context("price change LISTEN registration failed")?;
        Ok(Self { client, connection })
    }

    pub(crate) async fn latest_sequence(
        &mut self,
        tenant_id: &str,
        sku: Option<&str>,
    ) -> anyhow::Result<i64> {
        let (row, _) = drive_listener_operation(
            &mut self.connection,
            self.client.query_one(
                "SELECT COALESCE(MAX(sequence), 0)::BIGINT FROM price_change_events WHERE tenant_id=$1 AND ($2::text IS NULL OR sku=$2::text)",
                &[&tenant_id, &sku],
            ),
        )
        .await
        .context("price change journal baseline query failed")?;
        Ok(row.get(0))
    }

    pub(crate) async fn healthcheck(&mut self) -> anyhow::Result<()> {
        drive_listener_operation(&mut self.connection, self.client.simple_query("SELECT 1"))
            .await
            .context("price change PostgreSQL healthcheck failed")
            .map(|_| ())
    }

    pub(crate) async fn events_after(
        &mut self,
        tenant_id: &str,
        sku: Option<&str>,
        sequence: i64,
    ) -> anyhow::Result<(Vec<PriceChangeNotification>, bool)> {
        let (rows, notification_received) = drive_listener_operation(
            &mut self.connection,
            self.client.query(
                "SELECT sequence, tenant_id, sku, product_id, variant_id, price_id, currency, amount_minor, compare_at_minor, valid_from::text, observed_at::text FROM price_change_events WHERE tenant_id=$1 AND ($2::text IS NULL OR sku=$2::text) AND sequence>$3 ORDER BY sequence",
                &[&tenant_id, &sku, &sequence],
            ),
        )
        .await
        .context("price change journal replay query failed")?;
        Ok((
            rows.into_iter()
                .map(|row| PriceChangeNotification {
                    sequence: row.get(0),
                    tenant_id: row.get(1),
                    sku: row.get(2),
                    product_id: row.get(3),
                    variant_id: row.get(4),
                    price_id: row.get(5),
                    currency: row.get(6),
                    amount_minor: row.get(7),
                    compare_at_minor: row.get(8),
                    valid_from: row.get(9),
                    observed_at: row.get(10),
                })
                .collect(),
            notification_received,
        ))
    }

    pub(crate) async fn wait_for_price_change(&mut self) -> anyhow::Result<()> {
        loop {
            match poll_fn(|cx| self.connection.poll_message(cx)).await {
                Some(Ok(AsyncMessage::Notification(notification)))
                    if notification.channel() == PRICE_CHANGE_CHANNEL =>
                {
                    return Ok(());
                }
                Some(Ok(_)) => {}
                Some(Err(error)) => {
                    return Err(error).context("price change LISTEN connection failed");
                }
                None => return Err(anyhow!("price change LISTEN connection closed")),
            }
        }
    }
}

async fn drive_listener_operation<T, F>(
    connection: &mut PriceChangeConnection,
    operation: F,
) -> anyhow::Result<(T, bool)>
where
    F: Future<Output = Result<T, tokio_postgres::Error>>,
{
    tokio::pin!(operation);
    let mut price_notification_received = false;
    loop {
        tokio::select! {
            result = &mut operation => return result.map(|value| (value, price_notification_received)).context("price change PostgreSQL operation failed"),
            message = poll_fn(|cx| connection.poll_message(cx)) => match message {
                Some(Ok(AsyncMessage::Notification(notification))) => {
                    if notification.channel() == PRICE_CHANGE_CHANNEL {
                        price_notification_received = true;
                    }
                }
                Some(Ok(_)) => {}
                Some(Err(error)) => return Err(error).context("price change PostgreSQL connection failed"),
                None => return Err(anyhow!("price change PostgreSQL connection closed")),
            },
        }
    }
}

pub(crate) fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_owned())
}

pub(crate) fn optional_env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|value| !value.is_empty())
}

pub(crate) fn clickhouse_http_url() -> String {
    std::env::var("CLICKHOUSE_HTTP_URL")
        .or_else(|_| std::env::var("CLICKHOUSE_URL"))
        .unwrap_or_else(|_| DEFAULT_CLICKHOUSE_HTTP_URL.to_owned())
}

pub(crate) fn url_with_query(
    base: &str,
    params: &[(&str, String)],
) -> anyhow::Result<reqwest::Url> {
    let mut url = reqwest::Url::parse(base).context("downstream URL is invalid")?;
    {
        let mut query = url.query_pairs_mut();
        for (key, value) in params {
            query.append_pair(key, value);
        }
    }
    Ok(url)
}

pub(crate) fn baggage_or(context: &OtelContext, key: &str, default: &str) -> String {
    context
        .baggage()
        .get(key)
        .map(ToString::to_string)
        .unwrap_or_else(|| default.to_owned())
}

pub(crate) fn catalog_readiness_url(graphql_url: &str) -> anyhow::Result<String> {
    let mut url = reqwest::Url::parse(graphql_url).context("catalog GraphQL URL is invalid")?;
    url.set_path("/actuator/health/readiness");
    url.set_query(None);
    Ok(url.to_string())
}

pub(crate) fn join_endpoint(base: &str, path: &str) -> String {
    format!(
        "{}/{}",
        base.trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

pub(crate) fn params_with_query(query: String, params: Vec<(&str, String)>) -> Vec<(&str, String)> {
    let mut all = Vec::with_capacity(params.len() + 1);
    all.push(("query", query));
    all.extend(params);
    all
}

pub(crate) async fn decode_json(
    response: reqwest::Response,
    operation: &str,
) -> anyhow::Result<Value> {
    let status = response.status();
    let text = response
        .text()
        .await
        .with_context(|| format!("{operation} response read failed"))?;
    if !status.is_success() {
        return Err(anyhow!(DownstreamHttpError {
            status,
            body: truncate(&text),
        }));
    }
    serde_json::from_str(&text).with_context(|| format!("{operation} returned invalid JSON"))
}

pub(crate) fn truncate(value: &str) -> String {
    value.chars().take(512).collect()
}

#[derive(Debug)]
pub(crate) struct DownstreamHttpError {
    pub(crate) status: reqwest::StatusCode,
    pub(crate) body: String,
}

impl std::fmt::Display for DownstreamHttpError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "downstream returned HTTP {}", self.status)
    }
}

impl std::error::Error for DownstreamHttpError {}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CatalogProductPage {
    pub(crate) items: Vec<CatalogProduct>,
    pub(crate) page: i32,
    pub(crate) size: i32,
    pub(crate) total_elements: i32,
    pub(crate) total_pages: i32,
    pub(crate) has_next: bool,
    pub(crate) experience: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CatalogProduct {
    pub(crate) id: String,
    pub(crate) tenant_id: String,
    pub(crate) slug: String,
    pub(crate) sku: String,
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) brand: Option<String>,
    pub(crate) category: CatalogCategory,
    pub(crate) price_minor: Option<i32>,
    pub(crate) price: Option<CatalogPriceSnapshot>,
    #[serde(default)]
    pub(crate) variants: Vec<CatalogProductVariant>,
    #[serde(default)]
    pub(crate) reviews: Vec<CatalogReview>,
    #[serde(default)]
    pub(crate) reviews_slow: Vec<CatalogReview>,
    pub(crate) risk_score: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CatalogCategory {
    pub(crate) id: String,
    pub(crate) tenant_id: String,
    pub(crate) slug: String,
    pub(crate) name: String,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CatalogPriceSnapshot {
    pub(crate) id: String,
    pub(crate) currency: String,
    pub(crate) amount_minor: i32,
    pub(crate) compare_at_minor: Option<i32>,
    pub(crate) valid_from: String,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CatalogProductVariant {
    pub(crate) id: String,
    pub(crate) tenant_id: String,
    pub(crate) product_id: String,
    pub(crate) sku: String,
    pub(crate) name: String,
    pub(crate) options: String,
    pub(crate) price: Option<CatalogPriceSnapshot>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PriceChangeNotification {
    #[serde(default)]
    pub(crate) sequence: i64,
    pub(crate) tenant_id: String,
    pub(crate) sku: String,
    pub(crate) product_id: String,
    pub(crate) variant_id: String,
    pub(crate) price_id: String,
    pub(crate) currency: String,
    pub(crate) amount_minor: i32,
    pub(crate) compare_at_minor: Option<i32>,
    pub(crate) valid_from: String,
    pub(crate) observed_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CatalogReview {
    pub(crate) id: String,
    pub(crate) product_id: String,
    pub(crate) title: String,
    pub(crate) text: String,
    pub(crate) stars: i32,
    pub(crate) verified_purchase: bool,
    pub(crate) created_at: String,
}
