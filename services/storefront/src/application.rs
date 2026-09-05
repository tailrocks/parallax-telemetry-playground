//! Commerce use cases and downstream orchestration.

use anyhow::{Context as _, anyhow};
use axum::http::HeaderMap;
use opentelemetry::{
    Context as OtelContext, KeyValue, baggage::BaggageExt, trace::TraceContextExt,
};
use playground_proto::{
    payment::v1::PaymentMethodType,
    pricing::v1::{QuoteItem, QuoteRequest, pricing_client::PricingClient},
};
use playground_telemetry::semconv;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::{
    collections::{HashMap, HashSet},
    future::Future,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::OnceCell;
use tonic::{
    Request,
    transport::{Channel, Endpoint},
};
use tonic_health::pb::{
    HealthCheckRequest, health_check_response::ServingStatus, health_client::HealthClient,
};
use tracing_opentelemetry::OpenTelemetrySpanExt;

use crate::{
    api::{ProductSort, SubscriptionRegistry},
    domain::*,
    infrastructure::*,
};

#[derive(Clone)]
pub(crate) struct StoreContext {
    pub(crate) http: reqwest::Client,
    pub(crate) catalog_url: String,
    pub(crate) pricing_endpoint: String,
    pub(crate) checkout_endpoint: String,
    pub(crate) clickhouse_url: String,
    pub(crate) clickhouse_user: Option<String>,
    pub(crate) clickhouse_password: Option<String>,
    pub(crate) database_url: String,
    pub(crate) propagation: OtelContext,
    pub(crate) inbound_tenant_id: Option<String>,
    pub(crate) inbound_tenant_error: Option<String>,
    pub(crate) inbound_session_id: Option<String>,
    pub(crate) pricing_channel: Arc<OnceCell<Channel>>,
    pub(crate) request_deadline: Option<Instant>,
    pub(crate) subscription_registry: Arc<SubscriptionRegistry>,
}

#[derive(Clone, Debug)]
pub(crate) struct CommerceIdentity {
    pub(crate) tenant_id: String,
    pub(crate) customer_id: String,
    pub(crate) session_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ValidatedBusinessContext {
    pub(crate) segment: String,
    pub(crate) tier: String,
    pub(crate) region: String,
    pub(crate) priority: String,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct CatalogProductsRequest<'a> {
    pub(crate) tenant_id: &'a str,
    pub(crate) search: Option<&'a str>,
    pub(crate) category: Option<&'a str>,
    pub(crate) sort: Option<ProductSort>,
    pub(crate) page: i32,
    pub(crate) size: i32,
    pub(crate) segment: &'a str,
}

#[derive(Debug, Deserialize)]
struct GraphQlEnvelope {
    data: Option<Value>,
    errors: Option<Vec<GraphQlError>>,
}

#[derive(Debug, Deserialize)]
struct GraphQlError {
    message: String,
    path: Option<Vec<Value>>,
}

pub(crate) fn parse_graphql_data(
    text: &str,
    operation_label: &str,
    root_name: &str,
    root_may_be_null: bool,
) -> anyhow::Result<Value> {
    let envelope: GraphQlEnvelope = serde_json::from_str(text)
        .with_context(|| format!("{operation_label} returned invalid GraphQL JSON"))?;
    let data = envelope
        .data
        .filter(|data| !data.is_null())
        .ok_or_else(|| anyhow!("{operation_label} returned no GraphQL data"))?;
    let root = data
        .as_object()
        .and_then(|object| object.get(root_name))
        .ok_or_else(|| anyhow!("{operation_label} response omitted {root_name}"))?;
    if !root_may_be_null && root.is_null() {
        return Err(anyhow!(
            "{operation_label} response returned a null required root"
        ));
    }

    let errors = envelope.errors.unwrap_or_default();
    if errors.is_empty() {
        return Ok(data);
    }

    let partial_only = !root.is_null()
        && errors.iter().all(|error| {
            error.path.as_ref().is_some_and(|path| {
                path.len() > 1
                    && path
                        .first()
                        .and_then(Value::as_str)
                        .is_some_and(|path_root| path_root == root_name)
            })
        });
    if !partial_only {
        return Err(anyhow!(
            "{operation_label} returned a GraphQL error affecting {root_name}"
        ));
    }

    let empty_message_count = errors
        .iter()
        .filter(|error| error.message.trim().is_empty())
        .count();
    tracing::warn!(
        operation = operation_label,
        error_count = errors.len(),
        empty_message_count,
        root = root_name,
        "GraphQL response contained nullable-field errors; retaining valid root data"
    );
    Ok(data)
}

impl juniper::Context for StoreContext {}

impl Default for StoreContext {
    fn default() -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(HTTP_CONNECT_TIMEOUT)
            .read_timeout(HTTP_READ_TIMEOUT)
            .timeout(DOWNSTREAM_REQUEST_TIMEOUT)
            .build()
            .expect("storefront HTTP client configuration is valid");
        Self::from_env_with_client(http)
    }
}

impl StoreContext {
    pub(crate) fn from_env_with_client(http: reqwest::Client) -> Self {
        Self {
            http,
            catalog_url: env_or("CATALOG_GRAPHQL_URL", DEFAULT_CATALOG_URL),
            pricing_endpoint: env_or("PRICING_ENDPOINT", DEFAULT_PRICING_ENDPOINT),
            checkout_endpoint: env_or("CHECKOUT_ENDPOINT", DEFAULT_CHECKOUT_ENDPOINT),
            clickhouse_url: clickhouse_http_url(),
            clickhouse_user: optional_env("CLICKHOUSE_USER"),
            clickhouse_password: optional_env("CLICKHOUSE_PASSWORD"),
            database_url: env_or("DATABASE_URL", DEFAULT_DATABASE_URL),
            propagation: OtelContext::new(),
            inbound_tenant_id: None,
            inbound_tenant_error: None,
            inbound_session_id: None,
            pricing_channel: Arc::new(OnceCell::new()),
            request_deadline: None,
            subscription_registry: Arc::new(SubscriptionRegistry::new()),
        }
    }

    pub(crate) fn for_request(&self, headers: &HeaderMap) -> Self {
        let inbound = playground_telemetry::extract_context(headers);
        let context = self.with_request_context(&inbound);
        let (inbound_tenant_id, inbound_tenant_error) =
            match playground_telemetry::resolve_http_tenant_identity(headers, None) {
                Ok(tenant_id) => (Some(tenant_id), None),
                Err(playground_telemetry::TenantIdentityError::Missing) => (None, None),
                Err(error) => (None, Some(error.to_string())),
            };
        Self {
            inbound_tenant_id,
            inbound_tenant_error,
            request_deadline: Some(Instant::now() + STOREFRONT_REQUEST_TIMEOUT),
            ..context
        }
    }

    pub(crate) fn with_request_context(&self, inbound: &OtelContext) -> Self {
        let inbound = playground_telemetry::sanitize_context(inbound);
        let inbound_tenant_id = inbound
            .baggage()
            .get(semconv::TENANT_ID)
            .map(ToString::to_string)
            .filter(|value| !value.is_empty());
        let inbound_session_id = inbound
            .baggage()
            .get("session.id")
            .map(ToString::to_string)
            .filter(|value| !value.is_empty());
        let span_context = tracing::Span::current().context();
        let base = if span_context.span().span_context().is_valid() {
            span_context
        } else {
            inbound.clone()
        };
        let tier = baggage_or(&inbound, semconv::USER_TIER, "standard");
        let segment = baggage_or(&inbound, "customer.segment", "standard");
        let region = baggage_or(&inbound, "region", "us-east-1");
        let priority = baggage_or(&inbound, "request.priority", "normal");
        let propagation = playground_telemetry::extend_baggage(
            &playground_telemetry::with_safe_parent_baggage(&base, &inbound),
            [
                KeyValue::new(semconv::USER_TIER, tier),
                KeyValue::new("customer.segment", segment),
                KeyValue::new("region", region),
                KeyValue::new("request.priority", priority),
            ],
        );
        playground_telemetry::stamp_business_baggage(&tracing::Span::current(), &propagation);
        Self {
            propagation,
            inbound_tenant_id,
            inbound_session_id,
            ..self.clone()
        }
    }

    pub(crate) fn remaining_timeout(&self, operation: &str) -> anyhow::Result<Duration> {
        let timeout = self
            .request_deadline
            .map(|deadline| deadline.saturating_duration_since(Instant::now()))
            .unwrap_or(DOWNSTREAM_REQUEST_TIMEOUT)
            .min(DOWNSTREAM_REQUEST_TIMEOUT);
        if timeout.is_zero() {
            return Err(anyhow!(
                "{operation} exceeded the storefront request deadline"
            ));
        }
        Ok(timeout)
    }

    async fn within_deadline<T, F>(&self, operation: &str, future: F) -> anyhow::Result<T>
    where
        F: Future<Output = anyhow::Result<T>>,
    {
        let timeout = self.remaining_timeout(operation)?;
        tokio::time::timeout(timeout, future)
            .await
            .map_err(|_| anyhow!("{operation} exceeded the storefront request deadline"))?
    }

    pub(crate) fn resolve_tenant_id(&self, requested: Option<&str>) -> anyhow::Result<String> {
        if let Some(error) = self.inbound_tenant_error.as_deref() {
            return Err(anyhow!(error.to_owned()));
        }
        let requested = requested.map(str::trim).filter(|value| !value.is_empty());
        let propagated = self.inbound_tenant_id.as_deref();
        if let (Some(requested), Some(propagated)) = (requested, propagated)
            && requested != propagated
        {
            return Err(anyhow!(
                "tenant identity conflicts with propagated request identity"
            ));
        }
        let tenant_id = requested
            .or(propagated)
            .ok_or_else(|| anyhow!("tenant_id is required in the request or tenant.id baggage"))?;
        validate_identity_component(tenant_id, "tenant_id")?;
        Ok(tenant_id.to_owned())
    }

    pub(crate) fn resolve_session_id(
        &self,
        requested: Option<&str>,
    ) -> anyhow::Result<Option<String>> {
        let requested = requested.map(str::trim).filter(|value| !value.is_empty());
        if let Some(value) = requested {
            validate_identity_component(value, "session_id")?;
        }
        let propagated = self.inbound_session_id.as_deref();
        if let (Some(requested), Some(propagated)) = (requested, propagated)
            && requested != propagated
        {
            return Err(anyhow!(
                "session identity conflicts with propagated request identity"
            ));
        }
        Ok(requested.or(propagated).map(str::to_owned))
    }

    pub(crate) fn resolve_identity(
        &self,
        tenant_id: Option<&str>,
        customer_id: Option<&str>,
        session_id: Option<&str>,
        require_session: bool,
    ) -> anyhow::Result<CommerceIdentity> {
        let tenant_id = self.resolve_tenant_id(tenant_id)?;
        let customer_id = customer_id
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow!("customer_id is required"))?;
        validate_identity_component(customer_id, "customer_id")?;
        let session_id = self.resolve_session_id(session_id)?;
        let session_id = match (require_session, session_id) {
            (true, Some(session_id)) => Some(session_id),
            (true, None) => Some(format!("session-{}", uuid::Uuid::new_v4())),
            (false, session_id) => session_id,
        };
        if let Some(session_id) = session_id.as_deref() {
            validate_identity_component(session_id, "session_id")?;
        }
        Ok(CommerceIdentity {
            tenant_id,
            customer_id: customer_id.to_owned(),
            session_id,
        })
    }

    pub(crate) fn context_for_identity(&self, identity: &CommerceIdentity) -> OtelContext {
        let mut values = vec![KeyValue::new(
            semconv::TENANT_ID,
            identity.tenant_id.clone(),
        )];
        if let Some(session_id) = identity.session_id.as_deref() {
            values.push(KeyValue::new("session.id", session_id.to_owned()));
        }
        playground_telemetry::extend_baggage(&self.propagation, values)
    }

    pub(crate) fn validated_business_context(
        context: &OtelContext,
        segment_override: Option<&str>,
    ) -> anyhow::Result<ValidatedBusinessContext> {
        let segment = segment_override
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| baggage_or(context, "customer.segment", "standard"));
        let tier = baggage_or(context, semconv::USER_TIER, "standard");
        let region = baggage_or(context, "region", "us-east-1");
        let priority = baggage_or(context, "request.priority", "normal");
        for (name, value) in [
            ("segment", segment.as_str()),
            ("tier", tier.as_str()),
            ("region", region.as_str()),
            ("priority", priority.as_str()),
        ] {
            validate_identity_component(value, name)?;
        }
        Ok(ValidatedBusinessContext {
            segment,
            tier,
            region,
            priority,
        })
    }

    pub(crate) fn outbound_context_for_identity(&self, identity: &CommerceIdentity) -> OtelContext {
        let propagation = self.context_for_identity(identity);
        let active = tracing::Span::current().context();
        if !active.span().span_context().is_valid() {
            return propagation;
        }
        let tier = baggage_or(&propagation, semconv::USER_TIER, "standard");
        let segment = baggage_or(&propagation, "customer.segment", "standard");
        let region = baggage_or(&propagation, "region", "us-east-1");
        let priority = baggage_or(&propagation, "request.priority", "normal");
        playground_telemetry::with_business_context_from_parent(
            &active,
            &propagation,
            &identity.tenant_id,
            &tier,
            &segment,
            &region,
            &priority,
        )
    }

    pub(crate) fn outbound_context_for_tenant(
        &self,
        tenant_id: &str,
    ) -> anyhow::Result<OtelContext> {
        let tenant_id = self.resolve_tenant_id(Some(tenant_id))?;
        let propagation = playground_telemetry::extend_baggage(
            &self.propagation,
            [KeyValue::new(semconv::TENANT_ID, tenant_id.clone())],
        );
        let active = tracing::Span::current().context();
        if !active.span().span_context().is_valid() {
            return Ok(propagation);
        }
        let tier = baggage_or(&propagation, semconv::USER_TIER, "standard");
        let segment = baggage_or(&propagation, "customer.segment", "standard");
        let region = baggage_or(&propagation, "region", "us-east-1");
        let priority = baggage_or(&propagation, "request.priority", "normal");
        Ok(playground_telemetry::with_business_context_from_parent(
            &active,
            &propagation,
            &tenant_id,
            &tier,
            &segment,
            &region,
            &priority,
        ))
    }

    pub(crate) async fn pricing_channel(&self) -> anyhow::Result<&Channel> {
        let timeout = self.remaining_timeout("pricing connection")?;
        self.pricing_channel
            .get_or_try_init(|| async move {
                let endpoint = Endpoint::from_shared(self.pricing_endpoint.clone())
                    .context("pricing endpoint is invalid")?
                    .connect_timeout(GRPC_CONNECT_TIMEOUT)
                    .timeout(DOWNSTREAM_REQUEST_TIMEOUT);
                tokio::time::timeout(timeout, endpoint.connect())
                    .await
                    .map_err(|_| {
                        anyhow!("pricing connection exceeded the storefront request deadline")
                    })?
                    .context("pricing connection failed")
            })
            .await
    }

    async fn pricing_ready(&self) -> anyhow::Result<()> {
        let channel = self.pricing_channel().await?.clone();
        let response = HealthClient::new(channel)
            .check(Request::new(HealthCheckRequest {
                service: "playground.pricing.v1.Pricing".to_owned(),
            }))
            .await
            .context("pricing gRPC health check failed")?;
        if response.into_inner().status == ServingStatus::Serving as i32 {
            Ok(())
        } else {
            Err(anyhow!("pricing gRPC health is not serving"))
        }
    }

    pub(crate) async fn readiness(&self) -> DependencyReadiness {
        let catalog_health_url = catalog_readiness_url(&self.catalog_url);
        let (catalog, pricing, checkout, postgres) = tokio::join!(
            self.dependency_ready("catalog", async {
                let url = catalog_health_url
                    .as_ref()
                    .map_err(|error| anyhow!(error.to_string()))?;
                self.http_dependency_ready(url, "catalog readiness").await
            }),
            self.dependency_ready("pricing", async { self.pricing_ready().await }),
            self.dependency_ready("checkout", async {
                self.http_dependency_ready(
                    &join_endpoint(&self.checkout_endpoint, "/readyz"),
                    "checkout readiness",
                )
                .await
            }),
            self.dependency_ready("postgres", async {
                let mut listener = PriceChangeListener::connect(&self.database_url).await?;
                listener.healthcheck().await
            }),
        );
        DependencyReadiness {
            catalog,
            pricing,
            checkout,
            postgres,
        }
    }

    pub(crate) async fn analytics_readiness(&self) -> AnalyticsReadiness {
        let clickhouse = self
            .dependency_ready("clickhouse", async { self.clickhouse_ping().await })
            .await;
        AnalyticsReadiness { clickhouse }
    }

    pub(crate) async fn dependency_ready<F>(&self, dependency: &'static str, operation: F) -> bool
    where
        F: Future<Output = anyhow::Result<()>>,
    {
        match tokio::time::timeout(DEPENDENCY_READY_TIMEOUT, operation).await {
            Ok(Ok(())) => true,
            Ok(Err(error)) => {
                tracing::debug!(dependency, error = %error, "storefront dependency is not ready");
                false
            }
            Err(_) => {
                tracing::debug!(dependency, "storefront dependency readiness timed out");
                false
            }
        }
    }

    pub(crate) async fn http_dependency_ready(
        &self,
        url: &str,
        operation: &str,
    ) -> anyhow::Result<()> {
        let response = self
            .send_http(
                self.http.get(url).headers(self.outbound_headers()),
                operation,
            )
            .await?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(anyhow!("{operation} returned HTTP {}", response.status()))
        }
    }

    async fn clickhouse_ping(&self) -> anyhow::Result<()> {
        let request = self
            .clickhouse_request(self.http.get(join_endpoint(&self.clickhouse_url, "/ping")))?
            .headers(self.outbound_headers());
        let response = self.send_http(request, "ClickHouse readiness").await?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(anyhow!(
                "ClickHouse readiness returned HTTP {}",
                response.status()
            ))
        }
    }

    pub(crate) async fn catalog_product(
        &self,
        tenant_id: &str,
        sku: &str,
        segment: &str,
    ) -> anyhow::Result<Option<Product>> {
        let tenant_id = self.resolve_tenant_id(Some(tenant_id))?;
        let identity_context = self.outbound_context_for_tenant(&tenant_id)?;
        let data = self
            .catalog_query_with_context(
                "catalog.product",
                "StorefrontProduct",
                CATALOG_PRODUCT_QUERY,
                json!({"sku": sku, "tenantId": tenant_id, "segment": segment}),
                &identity_context,
            )
            .await?;
        let Some(value) = data.get("product") else {
            return Err(anyhow!("catalog.product response omitted product"));
        };
        if value.is_null() {
            return Ok(None);
        }
        let product: CatalogProduct =
            serde_json::from_value(value.clone()).context("catalog.product payload is invalid")?;
        Ok(Some(Product::from_catalog(product)))
    }

    pub(crate) async fn catalog_products(
        &self,
        request: CatalogProductsRequest<'_>,
    ) -> anyhow::Result<ProductPage> {
        let CatalogProductsRequest {
            tenant_id: requested_tenant_id,
            search,
            category,
            sort,
            page,
            size,
            segment,
        } = request;
        let tenant_id = self.resolve_tenant_id(Some(requested_tenant_id))?;
        let identity_context = self.outbound_context_for_tenant(&tenant_id)?;
        let data = self
            .catalog_query_with_context(
                "catalog.products",
                "StorefrontProducts",
                CATALOG_PRODUCTS_QUERY,
                json!({
                    "search": search,
                    "category": category,
                    "sort": sort.map(ProductSort::catalog_value),
                    "tenantId": tenant_id,
                    "page": page,
                    "size": size,
                    "segment": segment,
                }),
                &identity_context,
            )
            .await?;
        let products = data
            .get("products")
            .ok_or_else(|| anyhow!("catalog.products response omitted products"))?;
        let page: CatalogProductPage = serde_json::from_value(products.clone())
            .context("catalog.products payload is invalid")?;
        Ok(ProductPage::from_catalog(page))
    }

    pub(crate) async fn catalog_categories(
        &self,
        tenant_id: &str,
    ) -> anyhow::Result<Vec<Category>> {
        let tenant_id = self.resolve_tenant_id(Some(tenant_id))?;
        let identity_context = self.outbound_context_for_tenant(&tenant_id)?;
        let data = self
            .catalog_query_with_context(
                "catalog.categories",
                "StorefrontCategories",
                CATALOG_CATEGORIES_QUERY,
                json!({"tenantId": tenant_id}),
                &identity_context,
            )
            .await?;
        let categories = data
            .get("categories")
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow!("catalog.categories response omitted categories"))?;
        categories
            .iter()
            .cloned()
            .map(serde_json::from_value)
            .map(|result| result.map(Category::from_catalog))
            .collect::<Result<Vec<_>, _>>()
            .context("catalog.categories payload is invalid")
    }

    pub(crate) async fn price_change_from_notification(
        &self,
        event: PriceChangeNotification,
    ) -> anyhow::Result<Option<PriceChange>> {
        let Some(mut product) = self
            .catalog_product(&event.tenant_id, &event.sku, "standard")
            .await?
        else {
            return Ok(None);
        };
        if product.id != event.product_id {
            return Ok(None);
        }

        let Some(variant_index) = product
            .variants
            .iter()
            .position(|variant| variant.id == event.variant_id && variant.sku == event.sku)
        else {
            return Ok(None);
        };
        let price = PriceSnapshot {
            id: event.price_id,
            currency: event.currency,
            amount_minor: event.amount_minor,
            compare_at_minor: event.compare_at_minor,
            valid_from: event.valid_from,
        };
        product.variants[variant_index].price = Some(price.clone());
        if product.sku == event.sku {
            product.price_minor = Some(price.amount_minor);
            product.price = Some(price.clone());
        }
        let variant = product.variants[variant_index].clone();
        Ok(Some(PriceChange {
            product,
            variant,
            price,
            observed_at: event.observed_at,
        }))
    }

    pub(crate) async fn catalog_query_with_context(
        &self,
        operation_label: &str,
        operation_name: &str,
        query: &str,
        variables: Value,
        context: &OtelContext,
    ) -> anyhow::Result<Value> {
        let body = json!({
            "operationName": operation_name,
            "query": query,
            "variables": variables,
        });
        let response = self
            .send_http(
                self.http
                    .post(&self.catalog_url)
                    .headers(context_headers(context))
                    .json(&body),
                operation_label,
            )
            .await?;
        let status = response.status();
        let text = self.read_response_text(response, operation_label).await?;
        if !status.is_success() {
            return Err(anyhow!(
                "{operation_label} returned HTTP {status}: {}",
                truncate(&text)
            ));
        }
        let root = match operation_name {
            "StorefrontProduct" => ("product", true),
            "StorefrontProducts" => ("products", false),
            "StorefrontCategories" => ("categories", false),
            _ => {
                return Err(anyhow!(
                    "{operation_label} uses an unsupported GraphQL operation"
                ));
            }
        };
        parse_graphql_data(&text, operation_label, root.0, root.1)
    }

    pub(crate) async fn quote(&self, input: &QuoteInput) -> anyhow::Result<Quote> {
        let identity = self.resolve_identity(
            input.tenant_id.as_deref(),
            input.customer_id.as_deref(),
            None,
            false,
        )?;
        validate_items(&input.items, "quote")?;
        let items = input
            .items
            .iter()
            .map(|item| {
                if item.sku.trim().is_empty() || item.quantity <= 0 {
                    return Err(anyhow!("each quote item needs a SKU and positive quantity"));
                }
                Ok(QuoteItem {
                    sku: item.sku.clone(),
                    quantity: u32::try_from(item.quantity)
                        .context("quote quantity exceeds the pricing contract")?,
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        let currency_code = input
            .currency_code
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| anyhow!("currency_code is required"))?;
        if currency_code.len() != 3
            || currency_code != currency_code.to_ascii_uppercase()
            || !currency_code.bytes().all(|byte| byte.is_ascii_alphabetic())
        {
            return Err(anyhow!(
                "currency_code must be a three-letter uppercase code"
            ));
        }
        let mut pricing_context = HashMap::new();
        if let Some(promotion_code) = input.promotion_code.as_deref() {
            pricing_context.insert("promotion_code".to_owned(), promotion_code.to_owned());
        }
        if let Some(strategy) = input.pricing_strategy.as_deref() {
            pricing_context.insert("pricing_strategy".to_owned(), strategy.to_owned());
        }
        let payment_method_type = input
            .payment_method_type
            .as_deref()
            .map(parse_payment_method_type)
            .transpose()?;
        let identity_context = self.outbound_context_for_identity(&identity);
        let request = QuoteRequest {
            request_id: input
                .request_id
                .clone()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| format!("storefront-{}", uuid::Uuid::new_v4())),
            tenant_id: identity.tenant_id.clone(),
            customer_id: identity.customer_id.clone(),
            items,
            currency_code: currency_code.to_owned(),
            context: pricing_context,
            payment_method_type,
        };
        let channel = self.pricing_channel().await?;
        let mut client = PricingClient::new(channel.clone());
        let mut grpc_request = tonic::Request::new(request);
        grpc_request.set_timeout(self.remaining_timeout("pricing quote")?);
        playground_telemetry::inject_grpc_metadata_with_context(
            &identity_context,
            grpc_request.metadata_mut(),
        );
        let response = self
            .within_deadline("pricing quote", async {
                client
                    .quote(grpc_request)
                    .await
                    .context("pricing quote failed")
            })
            .await?
            .into_inner();
        Quote::try_from_proto(response)
    }

    pub(crate) async fn checkout(&self, input: &CheckoutInput) -> anyhow::Result<Value> {
        validate_items(&input.items, "checkout")?;
        let identity = self.resolve_identity(
            input.tenant_id.as_deref(),
            input.customer_id.as_deref(),
            input.session_id.as_deref(),
            true,
        )?;
        let currency_code = input
            .currency_code
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| anyhow!("currency_code is required"))?;
        validate_currency_code(currency_code)?;
        let payment_method_token = input
            .payment_method_token
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow!("payment_method_token is required"))?;
        let identity_context = self.outbound_context_for_identity(&identity);
        let business =
            Self::validated_business_context(&identity_context, input.segment.as_deref())?;
        let body = json!({
            "request_id": input.request_id,
            "tenant_id": identity.tenant_id,
            "customer_id": identity.customer_id,
            "cart_id": input.cart_id,
            "session_id": identity.session_id,
            "currency_code": currency_code,
            "promotion_code": input.promotion_code,
            "payment_method_token": payment_method_token,
            "payment_method_type": input.payment_method_type.as_deref().unwrap_or("card"),
            "segment": business.segment,
            "tier": business.tier,
            "region": business.region,
            "priority": business.priority,
            "items": input.items,
        });
        self.post_json_value_with_context(
            &join_endpoint(&self.checkout_endpoint, "/checkout"),
            &body,
            "checkout",
            &identity_context,
        )
        .await
    }

    pub(crate) async fn cart(
        &self,
        tenant_id: &str,
        customer_id: &str,
        session_id: Option<&str>,
    ) -> anyhow::Result<Option<Cart>> {
        let identity =
            self.resolve_identity(Some(tenant_id), Some(customer_id), session_id, false)?;
        let identity_context = self.outbound_context_for_identity(&identity);
        let mut params = vec![
            ("tenant_id", identity.tenant_id.clone()),
            ("customer_id", identity.customer_id.clone()),
        ];
        if let Some(session_id) = identity.session_id.clone() {
            params.push(("session_id", session_id));
        }
        let value = self
            .get_query_value_with_context(
                &join_endpoint(&self.checkout_endpoint, "/api/cart"),
                &params,
                "cart query",
                &identity_context,
            )
            .await?;
        let cart = value
            .get("cart")
            .ok_or_else(|| anyhow!("cart query response omitted cart"))?;
        if cart.is_null() {
            return Ok(None);
        }
        serde_json::from_value(cart.clone())
            .context("cart query returned an invalid durable cart payload")
            .map(Some)
    }

    pub(crate) async fn add_cart_item(
        &self,
        input: &AddCartItemInput,
    ) -> anyhow::Result<CartItemAdded> {
        validate_add_cart_item_input(input)?;
        let identity = self.resolve_identity(
            input.tenant_id.as_deref(),
            input.customer_id.as_deref(),
            input.session_id.as_deref(),
            true,
        )?;
        let currency_code = input
            .currency_code
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| anyhow!("currency_code is required"))?;
        validate_currency_code(currency_code)?;
        let identity_context = self.outbound_context_for_identity(&identity);
        let body = json!({
            "tenant_id": identity.tenant_id,
            "customer_id": identity.customer_id,
            "cart_id": input
                .cart_id
                .as_deref()
                .filter(|value| !value.trim().is_empty()),
            "session_id": identity.session_id,
            "sku": input.sku,
            "quantity": input.quantity,
            "currency_code": currency_code,
        });
        let value = self
            .post_json_value_with_context(
                &join_endpoint(&self.checkout_endpoint, "/api/cart/items"),
                &body,
                "cart item add",
                &identity_context,
            )
            .await?;
        CartItemAdded::from_value(&value)
    }

    pub(crate) async fn orders(
        &self,
        tenant_id: &str,
        customer_id: Option<&str>,
        session_id: Option<&str>,
    ) -> anyhow::Result<Vec<Order>> {
        let tenant_id = self.resolve_tenant_id(Some(tenant_id))?;
        let customer_id = customer_id
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| {
                validate_identity_component(value, "customer_id")?;
                Ok::<String, anyhow::Error>(value.to_owned())
            })
            .transpose()?;
        let session_id = self.resolve_session_id(session_id)?;
        let identity_context = self.outbound_context_for_tenant(&tenant_id)?;
        let mut params = vec![("tenant_id", tenant_id.clone())];
        if let Some(customer_id) = customer_id.as_deref() {
            params.push(("customer_id", customer_id.to_owned()));
        }
        if let Some(session_id) = session_id {
            params.push(("session_id", session_id));
        }
        let value = self
            .get_query_value_with_context(
                &join_endpoint(&self.checkout_endpoint, "/api/orders"),
                &params,
                "order query",
                &identity_context,
            )
            .await?;
        order_list_from_value(&value, &tenant_id, customer_id.as_deref())
    }

    pub(crate) async fn order(
        &self,
        tenant_id: &str,
        order_id: &str,
        customer_id: &str,
        session_id: Option<&str>,
    ) -> anyhow::Result<Option<Order>> {
        let identity =
            self.resolve_identity(Some(tenant_id), Some(customer_id), session_id, false)?;
        let identity_context = self.outbound_context_for_identity(&identity);
        let mut params = vec![
            ("tenant_id", identity.tenant_id.clone()),
            ("customer_id", identity.customer_id.clone()),
        ];
        if let Some(session_id) = identity.session_id.clone() {
            params.push(("session_id", session_id));
        }
        let value = self
            .get_query_value_with_context(
                &join_endpoint(&self.checkout_endpoint, &format!("/api/orders/{order_id}")),
                &params,
                "order query",
                &identity_context,
            )
            .await?;
        if value.is_null() {
            return Ok(None);
        }
        let order = Order::try_from_value(&value)?;
        if order.tenant_id.as_deref() != Some(identity.tenant_id.as_str())
            || order.customer_id.as_deref() != Some(identity.customer_id.as_str())
        {
            return Err(anyhow!(
                "order response identity does not match the request"
            ));
        }
        Ok(Some(order))
    }

    pub(crate) async fn analytics(
        &self,
        tenant_id: &str,
        event_name: Option<&str>,
        limit: i32,
    ) -> anyhow::Result<Vec<AnalyticsEvent>> {
        let tenant_id = self.resolve_tenant_id(Some(tenant_id))?;
        let identity_context = self.outbound_context_for_tenant(&tenant_id)?;
        let mut query = String::from(
            "SELECT toString(event_id) AS event_id, tenant_id, event_key, customer_id, \
             event_name, event_version, source, entity_type, entity_id, \
             toString(occurred_at) AS occurred_at, ifNull(trace_id, '') AS trace_id, \
             ifNull(span_id, '') AS span_id, ifNull(traceparent, '') AS traceparent, \
             ifNull(tracestate, '') AS tracestate, ifNull(baggage, '') AS baggage, \
             ifNull(feature_variant, '') AS feature_variant, \
             ifNull(properties, '{}') AS properties, ifNull(context, '{}') AS context \
             FROM analytics.analytics_events FINAL \
             WHERE tenant_id = {tenant_id:String}",
        );
        let mut params = vec![
            ("param_tenant_id", tenant_id.clone()),
            ("param_limit", limit.to_string()),
        ];
        if let Some(event_name) = event_name {
            query.push_str(" AND event_name = {event_name:String}");
            params.push(("param_event_name", event_name.to_owned()));
        }
        query.push_str(" ORDER BY occurred_at DESC LIMIT {limit:UInt32} FORMAT JSON");
        let value = self
            .clickhouse_query_value_with_context(
                &self.clickhouse_url,
                &params_with_query(query, params),
                "analytics query",
                &identity_context,
            )
            .await?;
        let rows = value
            .get("data")
            .cloned()
            .ok_or_else(|| anyhow!("analytics query returned no data"))?;
        serde_json::from_value(rows).context("analytics response rows are invalid")
    }

    pub(crate) async fn analytics_summary(
        &self,
        tenant_id: &str,
        event_name: Option<&str>,
    ) -> anyhow::Result<AnalyticsSummary> {
        let tenant_id = self.resolve_tenant_id(Some(tenant_id))?;
        let identity_context = self.outbound_context_for_tenant(&tenant_id)?;
        let mut query = String::from(
            "SELECT count() AS event_count, uniqExact(customer_id) AS unique_customers, \
             minOrNull(toString(occurred_at)) AS first_occurred_at, \
             maxOrNull(toString(occurred_at)) AS last_occurred_at \
             FROM analytics.analytics_events FINAL \
             WHERE tenant_id = {tenant_id:String}",
        );
        let mut params = vec![("param_tenant_id", tenant_id.clone())];
        if let Some(event_name) = event_name {
            query.push_str(" AND event_name = {event_name:String}");
            params.push(("param_event_name", event_name.to_owned()));
        }
        query.push_str(" FORMAT JSON");
        let value = self
            .clickhouse_query_value_with_context(
                &self.clickhouse_url,
                &params_with_query(query, params),
                "analytics summary query",
                &identity_context,
            )
            .await?;
        let rows = value
            .get("data")
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow!("analytics summary query returned no rows"))?;
        let row = rows
            .first()
            .ok_or_else(|| anyhow!("analytics summary query returned an empty result"))?;
        Ok(AnalyticsSummary {
            tenant_id,
            event_name: event_name.map(str::to_owned),
            event_count: u64_field(row, "event_count")
                .and_then(|value| i32::try_from(value).ok())
                .ok_or_else(|| anyhow!("analytics event count exceeds GraphQL Int"))?,
            unique_customers: u64_field(row, "unique_customers")
                .and_then(|value| i32::try_from(value).ok())
                .ok_or_else(|| anyhow!("analytics customer count exceeds GraphQL Int"))?,
            first_occurred_at: optional_string_field(row, "first_occurred_at")?,
            last_occurred_at: optional_string_field(row, "last_occurred_at")?,
        })
    }

    pub(crate) async fn record_analytics(
        &self,
        input: &AnalyticsInput,
    ) -> anyhow::Result<AnalyticsAck> {
        validate_analytics_input(input)?;
        let tenant_id = self.resolve_tenant_id(Some(&input.tenant_id))?;
        let customer_id = input
            .customer_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| {
                validate_identity_component(value, "customer_id")?;
                Ok::<String, anyhow::Error>(value.to_owned())
            })
            .transpose()?;
        let session_id = self
            .resolve_session_id(input.session_id.as_deref())?
            .ok_or_else(|| anyhow!("analytics session identity is required"))?;
        let properties = input.properties.as_deref().unwrap_or("{}");
        let properties_value: Value =
            serde_json::from_str(properties).context("analytics properties must be valid JSON")?;
        if !properties_value.is_object() {
            return Err(anyhow!("analytics properties must be a JSON object"));
        }
        let context_value = input
            .context
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .context("analytics context must be valid JSON")?
            .unwrap_or_else(|| json!({}));
        if !context_value.is_object() {
            return Err(anyhow!("analytics context must be a JSON object"));
        }
        let propagation = playground_telemetry::extend_baggage(
            &self.outbound_context_for_tenant(&tenant_id)?,
            [KeyValue::new("session.id", session_id.clone())],
        );
        let (traceparent, tracestate, baggage) = propagation_headers(&propagation);
        let feature_variant = baggage_value(&propagation, "feature.variant");
        let event_key = input.event_key.clone();
        let params = vec![
            ("query", ANALYTICS_INSERT_QUERY.to_owned()),
            (
                "param_event_id",
                deterministic_event_id(&tenant_id, &event_key),
            ),
            ("param_tenant_id", tenant_id),
            ("param_event_key", event_key.clone()),
            ("param_customer_id", customer_id.unwrap_or_default()),
            ("param_session_id", session_id),
            ("param_occurred_at", input.occurred_at.clone()),
            ("param_event_name", input.event_name.clone()),
            ("param_event_version", "1".to_owned()),
            ("param_source", "storefront".to_owned()),
            ("param_entity_type", input.entity_type.clone()),
            ("param_entity_id", input.entity_id.clone()),
            ("param_trace_id", trace_id(&propagation)),
            ("param_span_id", span_id(&propagation)),
            ("param_traceparent", traceparent),
            ("param_tracestate", tracestate),
            ("param_baggage", baggage),
            ("param_feature_variant", feature_variant),
            ("param_properties", properties_value.to_string()),
            (
                "param_context",
                analytics_context_json(&propagation, &context_value)?,
            ),
        ];
        let request = self
            .clickhouse_request(
                self.http
                    .post(url_with_query(&self.clickhouse_url, &params)?),
            )?
            .headers(self.outbound_headers());
        let response = self.send_http(request, "analytics insert").await?;
        let status = response.status();
        if !status.is_success() {
            let text = self
                .read_response_text(response, "analytics insert")
                .await
                .unwrap_or_else(|_| String::new());
            return Err(anyhow!(
                "analytics insert returned HTTP {status}: {}",
                truncate(&text)
            ));
        }
        Ok(AnalyticsAck {
            event_key,
            status: "recorded".to_owned(),
        })
    }

    pub(crate) fn outbound_headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        let context = self.outbound_context();
        playground_telemetry::inject_context_headers(&context, &mut headers);
        headers
    }

    /// Use the active resolver/client span as the outbound parent while
    /// retaining the request's sanitized business baggage.
    pub(crate) fn outbound_context(&self) -> OtelContext {
        let active = tracing::Span::current().context();
        if !active.span().span_context().is_valid() {
            return self.propagation.clone();
        }
        let tier = baggage_or(&self.propagation, semconv::USER_TIER, "standard");
        let segment = baggage_or(&self.propagation, "customer.segment", "standard");
        let region = baggage_or(&self.propagation, "region", "us-east-1");
        let priority = baggage_or(&self.propagation, "request.priority", "normal");
        playground_telemetry::extend_baggage(
            &playground_telemetry::with_safe_parent_baggage(&active, &self.propagation),
            [
                KeyValue::new(semconv::USER_TIER, tier),
                KeyValue::new("customer.segment", segment),
                KeyValue::new("region", region),
                KeyValue::new("request.priority", priority),
            ],
        )
    }

    async fn send_http(
        &self,
        request: reqwest::RequestBuilder,
        operation: &str,
    ) -> anyhow::Result<reqwest::Response> {
        let timeout = self.remaining_timeout(operation)?;
        let result = self
            .within_deadline(operation, async move {
                request
                    .timeout(timeout)
                    .send()
                    .await
                    .with_context(|| format!("{operation} request failed"))
            })
            .await;
        match result {
            Err(error)
                if error
                    .chain()
                    .find_map(|cause| cause.downcast_ref::<reqwest::Error>())
                    .is_some_and(reqwest::Error::is_timeout) =>
            {
                Err(anyhow!(
                    "{operation} exceeded the storefront request deadline"
                ))
            }
            other => other,
        }
    }

    async fn read_response_text(
        &self,
        response: reqwest::Response,
        operation: &str,
    ) -> anyhow::Result<String> {
        self.within_deadline(operation, async move {
            response
                .text()
                .await
                .with_context(|| format!("{operation} response read failed"))
        })
        .await
    }

    async fn decode_json_response(
        &self,
        response: reqwest::Response,
        operation: &str,
    ) -> anyhow::Result<Value> {
        self.within_deadline(operation, decode_json(response, operation))
            .await
    }

    fn clickhouse_request(
        &self,
        request: reqwest::RequestBuilder,
    ) -> anyhow::Result<reqwest::RequestBuilder> {
        match (&self.clickhouse_user, &self.clickhouse_password) {
            (None, None) => Ok(request),
            (Some(user), Some(password)) => Ok(request.basic_auth(user, Some(password))),
            _ => Err(anyhow!(
                "CLICKHOUSE_USER and CLICKHOUSE_PASSWORD must be configured together"
            )),
        }
    }

    async fn clickhouse_query_value_with_context(
        &self,
        url: &str,
        params: &[(&str, String)],
        operation: &str,
        context: &OtelContext,
    ) -> anyhow::Result<Value> {
        let request = self
            .clickhouse_request(self.http.get(url_with_query(url, params)?))?
            .headers(context_headers(context));
        let response = self.send_http(request, operation).await?;
        self.decode_json_response(response, operation).await
    }

    pub(crate) async fn post_json_value_with_context(
        &self,
        url: &str,
        body: &Value,
        operation: &str,
        context: &OtelContext,
    ) -> anyhow::Result<Value> {
        let response = self
            .send_http(
                self.http
                    .post(url)
                    .headers(context_headers(context))
                    .json(body),
                operation,
            )
            .await?;
        self.decode_json_response(response, operation).await
    }

    pub(crate) async fn get_query_value_with_context(
        &self,
        url: &str,
        params: &[(&str, String)],
        operation: &str,
        context: &OtelContext,
    ) -> anyhow::Result<Value> {
        let response = self
            .send_http(
                self.http
                    .get(url_with_query(url, params)?)
                    .headers(context_headers(context)),
                operation,
            )
            .await?;
        self.decode_json_response(response, operation).await
    }
}

pub(crate) fn validate_items(items: &[CartItemInput], operation: &str) -> anyhow::Result<()> {
    if items.is_empty() || items.len() > 50 {
        return Err(anyhow!("{operation} requires at least one item"));
    }
    let mut skus = HashSet::with_capacity(items.len());
    for item in items {
        validate_identity_component(&item.sku, "sku")?;
        if item.quantity <= 0 || item.quantity > MAX_CART_ITEM_QUANTITY {
            return Err(anyhow!(
                "{operation} item quantity must be between 1 and {MAX_CART_ITEM_QUANTITY}"
            ));
        }
        if !skus.insert(item.sku.as_str()) {
            return Err(anyhow!("{operation} SKUs must be unique"));
        }
    }
    Ok(())
}

fn context_headers(context: &OtelContext) -> HeaderMap {
    let mut headers = HeaderMap::new();
    playground_telemetry::inject_context_headers(context, &mut headers);
    headers
}

fn validate_identity_component(value: &str, name: &str) -> anyhow::Result<()> {
    if value.trim().is_empty() || value.len() > 128 || value.chars().any(char::is_control) {
        return Err(anyhow!("{name} is invalid"));
    }
    Ok(())
}

fn validate_currency_code(value: &str) -> anyhow::Result<()> {
    if value.len() != 3
        || value != value.to_ascii_uppercase()
        || !value.bytes().all(|byte| byte.is_ascii_alphabetic())
    {
        return Err(anyhow!(
            "currency_code must be a three-letter uppercase code"
        ));
    }
    Ok(())
}

pub(crate) fn validate_add_cart_item_input(input: &AddCartItemInput) -> anyhow::Result<()> {
    if input.quantity <= 0 || input.quantity > MAX_CART_ITEM_QUANTITY {
        return Err(anyhow!(
            "cart item needs a SKU and quantity between 1 and {MAX_CART_ITEM_QUANTITY}"
        ));
    }
    validate_identity_component(&input.sku, "sku")?;
    let currency_code = input
        .currency_code
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("currency_code is required"))?;
    validate_currency_code(currency_code)?;
    Ok(())
}

pub(crate) fn validate_analytics_input(input: &AnalyticsInput) -> anyhow::Result<()> {
    for (name, value) in [
        ("tenant_id", input.tenant_id.as_str()),
        ("event_key", input.event_key.as_str()),
        ("event_name", input.event_name.as_str()),
        ("entity_type", input.entity_type.as_str()),
        ("entity_id", input.entity_id.as_str()),
        ("occurred_at", input.occurred_at.as_str()),
    ] {
        if value.trim().is_empty() {
            return Err(anyhow!("{name} is required"));
        }
    }
    if input.occurred_at.len() > 64 || input.occurred_at.chars().any(char::is_control) {
        return Err(anyhow!("occurred_at is invalid"));
    }
    Ok(())
}

pub(crate) fn parse_payment_method_type(value: &str) -> anyhow::Result<i32> {
    let normalized = value.to_ascii_lowercase();
    let method = match normalized.as_str() {
        "card" => PaymentMethodType::Card,
        "bank_account" | "bank-account" => PaymentMethodType::BankAccount,
        "wallet" => PaymentMethodType::Wallet,
        _ => return Err(anyhow!("unsupported payment method type: {value}")),
    };
    Ok(method as i32)
}

pub(crate) fn order_list_from_value(
    value: &Value,
    tenant_id: &str,
    customer_id: Option<&str>,
) -> anyhow::Result<Vec<Order>> {
    let values = value
        .as_array()
        .or_else(|| value.get("orders").and_then(Value::as_array))
        .ok_or_else(|| anyhow!("order query returned no order list"))?;
    values
        .iter()
        .map(Order::try_from_value)
        .collect::<anyhow::Result<Vec<_>>>()
        .and_then(|orders| {
            for order in &orders {
                if order.tenant_id.as_deref() != Some(tenant_id)
                    || customer_id.is_some_and(|customer_id| {
                        order.customer_id.as_deref() != Some(customer_id)
                    })
                {
                    return Err(anyhow!("order list contains an out-of-scope identity"));
                }
            }
            Ok(orders)
        })
}

pub(crate) fn trace_id(context: &OtelContext) -> String {
    let span = context.span();
    let span_context = span.span_context();
    if span_context.is_valid() {
        span_context.trace_id().to_string()
    } else {
        String::new()
    }
}

pub(crate) fn span_id(context: &OtelContext) -> String {
    let span = context.span();
    let span_context = span.span_context();
    if span_context.is_valid() {
        span_context.span_id().to_string()
    } else {
        String::new()
    }
}

fn propagation_headers(context: &OtelContext) -> (String, String, String) {
    let mut headers = HeaderMap::new();
    playground_telemetry::inject_context_headers(context, &mut headers);
    (
        header_value(&headers, "traceparent"),
        header_value(&headers, "tracestate"),
        header_value(&headers, "baggage"),
    )
}

fn header_value(headers: &HeaderMap, name: &str) -> String {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned()
}

fn baggage_value(context: &OtelContext, key: &str) -> String {
    context
        .baggage()
        .get(key)
        .map(ToString::to_string)
        .unwrap_or_default()
}

fn analytics_context_json(context: &OtelContext, input: &Value) -> anyhow::Result<String> {
    let mut stored = Map::new();
    if let Some(attributes) = input.as_object() {
        stored.insert("attributes".to_owned(), Value::Object(attributes.clone()));
    }
    let (traceparent, tracestate, baggage) = propagation_headers(context);
    stored.insert("traceparent".to_owned(), Value::String(traceparent));
    stored.insert("tracestate".to_owned(), Value::String(tracestate));
    stored.insert("baggage".to_owned(), Value::String(baggage));
    let baggage_items = context
        .baggage()
        .iter()
        .map(|(key, (value, _))| (key.to_string(), Value::String(value.to_string())))
        .collect();
    stored.insert("baggage_items".to_owned(), Value::Object(baggage_items));
    stored.insert(
        "feature_variant".to_owned(),
        Value::String(baggage_value(context, "feature.variant")),
    );
    serde_json::to_string(&Value::Object(stored)).context("analytics context could not be encoded")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Json, Router,
        http::HeaderMap,
        routing::{get, post},
    };
    use opentelemetry::{Context as OtelContext, KeyValue, baggage::BaggageExt};
    use serde_json::json;
    use std::sync::Arc;

    const TEST_TENANT: &str = "tenant-acme";

    #[test]
    fn nullable_graphql_field_errors_retain_valid_root_data() {
        let response = json!({
            "data": {"product": {"id": "product-1", "reviewsSlow": null}},
            "errors": [{
                "message": "slow reviews unavailable",
                "path": ["product", "reviewsSlow"]
            }]
        });

        let data = parse_graphql_data(&response.to_string(), "catalog.product", "product", true)
            .expect("nullable field error preserves product data");
        assert_eq!(data["product"]["id"], "product-1");
    }

    #[test]
    fn required_graphql_root_errors_fail_closed() {
        let response = json!({
            "data": {"products": null},
            "errors": [{"message": "catalog unavailable", "path": ["products"]}]
        });

        let error =
            parse_graphql_data(&response.to_string(), "catalog.products", "products", false)
                .expect_err("required root failure must not become an empty page");
        assert!(error.to_string().contains("required root"));
    }

    #[tokio::test]
    async fn checkout_forwards_validated_business_context() {
        let seen = Arc::new(tokio::sync::Mutex::new(None::<Value>));
        let seen_for_handler = Arc::clone(&seen);
        let fixture = Router::new().route(
            "/checkout",
            post(move |Json(body): Json<Value>| {
                let seen = Arc::clone(&seen_for_handler);
                async move {
                    *seen.lock().await = Some(body);
                    Json(json!({"order_id": "order-1"}))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind checkout fixture");
        let address = listener.local_addr().expect("checkout fixture address");
        let server = tokio::spawn(async move {
            axum::serve(listener, fixture)
                .await
                .expect("serve checkout fixture");
        });

        let inbound = OtelContext::new().with_baggage([
            KeyValue::new(semconv::TENANT_ID, "tenant-acme"),
            KeyValue::new(semconv::USER_TIER, "premium"),
            KeyValue::new("customer.segment", "vip"),
            KeyValue::new("region", "eu-west-1"),
            KeyValue::new("request.priority", "urgent"),
        ]);
        let context = StoreContext {
            checkout_endpoint: format!("http://{address}"),
            ..StoreContext::default()
        }
        .with_request_context(&inbound);
        context
            .checkout(&CheckoutInput {
                items: vec![CartItemInput {
                    sku: "WIDGET-1".to_owned(),
                    quantity: 1,
                }],
                tenant_id: Some("tenant-acme".to_owned()),
                customer_id: Some("customer-1".to_owned()),
                cart_id: None,
                session_id: Some("session-1".to_owned()),
                currency_code: Some("USD".to_owned()),
                promotion_code: None,
                payment_method_token: Some("tok_visa".to_owned()),
                payment_method_type: Some("card".to_owned()),
                segment: None,
                request_id: Some("request-1".to_owned()),
            })
            .await
            .expect("checkout fixture response");

        let body = seen.lock().await.clone().expect("checkout body");
        assert_eq!(body["segment"], "vip");
        assert_eq!(body["tier"], "premium");
        assert_eq!(body["region"], "eu-west-1");
        assert_eq!(body["priority"], "urgent");
        server.abort();
    }

    #[tokio::test]
    async fn clickhouse_queries_send_basic_auth() {
        let seen = Arc::new(tokio::sync::Mutex::new(None::<String>));
        let seen_for_handler = Arc::clone(&seen);
        let fixture = Router::new().route(
            "/",
            get(move |headers: HeaderMap| {
                let seen = Arc::clone(&seen_for_handler);
                async move {
                    *seen.lock().await = headers
                        .get("authorization")
                        .and_then(|value| value.to_str().ok())
                        .map(str::to_owned);
                    Json(json!({"data": []}))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ClickHouse fixture");
        let address = listener.local_addr().expect("ClickHouse fixture address");
        let server = tokio::spawn(async move {
            axum::serve(listener, fixture)
                .await
                .expect("serve ClickHouse fixture");
        });

        let context = StoreContext {
            clickhouse_url: format!("http://{address}"),
            clickhouse_user: Some("analytics".to_owned()),
            clickhouse_password: Some("secret".to_owned()),
            ..StoreContext::default()
        };
        let events = context
            .analytics(TEST_TENANT, None, 10)
            .await
            .expect("analytics fixture response");
        assert!(events.is_empty());
        assert_eq!(
            seen.lock().await.as_deref(),
            Some("Basic YW5hbHl0aWNzOnNlY3JldA==")
        );
        server.abort();
    }

    #[tokio::test]
    async fn downstream_request_respects_remaining_deadline() {
        let fixture = Router::new().route(
            "/",
            post(|| async {
                tokio::time::sleep(Duration::from_millis(100)).await;
                Json(json!({"ok": true}))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind timeout fixture");
        let address = listener.local_addr().expect("timeout fixture address");
        let server = tokio::spawn(async move {
            axum::serve(listener, fixture)
                .await
                .expect("serve timeout fixture");
        });

        let context = StoreContext {
            checkout_endpoint: format!("http://{address}"),
            request_deadline: Some(Instant::now() + Duration::from_millis(10)),
            ..StoreContext::default()
        };
        let started = Instant::now();
        let error = context
            .post_json_value_with_context(
                &join_endpoint(&context.checkout_endpoint, "/"),
                &json!({}),
                "timeout fixture",
                &OtelContext::new(),
            )
            .await
            .expect_err("downstream request must be bounded");
        assert!(started.elapsed() < Duration::from_millis(90));
        assert!(format!("{error:#}").contains("exceeded the storefront request deadline"));
        server.abort();
    }
}
