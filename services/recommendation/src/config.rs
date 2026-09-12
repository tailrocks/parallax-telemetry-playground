use anyhow::Context as _;
use serde::Deserialize;
use std::time::Duration;

pub(crate) const DEFAULT_CATALOG_GRAPHQL_URL: &str = "http://catalog:8080/graphql";
pub(crate) const DEFAULT_SEGMENT: &str = "standard";
pub(crate) const DEFAULT_LIMIT: usize = 8;
pub(crate) const MAX_LIMIT: usize = 100;
pub(crate) const MAX_STAMPEDE: usize = 16;
pub(crate) const MAX_CHAOS_DELAY_MS: u64 = 5_000;
pub(crate) const MAX_CHAOS_LEAK_KB_PER_REQUEST: usize = 1_024;
pub(crate) const MAX_CHAOS_LEAK_KB_TOTAL: usize = 8 * 1_024;
pub(crate) const READINESS_TENANT_ID: &str = "tenant-acme";
pub(crate) const READINESS_QUERY: &str = r#"
query Readiness($tenantId: ID!) {
  product(sku: "WIDGET-1", tenantId: $tenantId, segment: "standard") {
    id
    tenantId
    sku
  }
}
"#;

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) http: reqwest::Client,
    pub(crate) catalog_url: String,
}

impl AppState {
    pub(crate) fn from_env() -> anyhow::Result<Self> {
        let catalog_url = std::env::var("CATALOG_GRAPHQL_URL")
            .unwrap_or_else(|_| DEFAULT_CATALOG_GRAPHQL_URL.to_owned());
        Self::new(catalog_url)
    }

    pub(crate) fn new(catalog_url: String) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(10))
            .build()
            .context("build catalog HTTP client")?;
        Ok(Self { http, catalog_url })
    }
}

pub(crate) fn catalog_readiness_url(graphql_url: &str) -> anyhow::Result<String> {
    let mut url = reqwest::Url::parse(graphql_url).context("catalog GraphQL URL is invalid")?;
    url.set_path("/actuator/health/readiness");
    url.set_query(None);
    Ok(url.to_string())
}

#[derive(Debug, Deserialize)]
pub(crate) struct CatalogHealthResponse {
    pub(crate) status: String,
    #[serde(default)]
    pub(crate) components: Option<CatalogHealthComponents>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CatalogHealthComponents {
    #[serde(default)]
    pub(crate) db: Option<CatalogHealthComponent>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CatalogHealthComponent {
    pub(crate) status: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CatalogReadinessResponse {
    pub(crate) data: Option<CatalogReadinessData>,
    #[serde(default)]
    pub(crate) errors: Option<Vec<CatalogGraphQlError>>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CatalogGraphQlError {
    pub(crate) message: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CatalogReadinessData {
    pub(crate) product: Option<CatalogProductSummary>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CatalogProductSummary {
    pub(crate) id: String,
    #[serde(rename = "tenantId")]
    pub(crate) tenant_id: String,
    pub(crate) sku: String,
}

#[derive(Debug, Deserialize, Clone)]
pub(crate) struct Recommend {
    pub(crate) sku: String,
    pub(crate) tenant_id: Option<String>,
    #[serde(default = "default_segment")]
    pub(crate) segment: String,
    #[serde(default = "default_limit")]
    pub(crate) limit: usize,
    /// Bounded deterministic latency for the slow-response scenario.
    #[serde(default)]
    pub(crate) slow: u64,
    /// Isolated bounded memory-retention scenario; never used for results.
    #[serde(default)]
    pub(crate) leak: usize,
    /// Bounded parallel Catalog requests for the thundering-herd scenario.
    #[serde(default)]
    pub(crate) stampede: usize,
}

pub(crate) fn default_segment() -> String {
    DEFAULT_SEGMENT.to_owned()
}

pub(crate) fn default_limit() -> usize {
    DEFAULT_LIMIT
}

pub(crate) fn bounded_limit(value: usize) -> usize {
    value.clamp(1, MAX_LIMIT)
}

pub(crate) fn bounded_delay_ms(value: u64) -> u64 {
    value.min(MAX_CHAOS_DELAY_MS)
}

pub(crate) fn bounded_stampede(value: usize) -> usize {
    value.min(MAX_STAMPEDE)
}
