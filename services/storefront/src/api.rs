//! HTTP, GraphQL, subscription, and CORS transport.

use anyhow::anyhow;
use axum::{
    Extension, Json, Router,
    extract::{Path, Query as QueryParams, Request, State, ws::WebSocketUpgrade},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    middleware,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use futures::{StreamExt, stream::BoxStream};
use juniper::{FieldError, FieldResult, RootNode, graphql_object, graphql_subscription};
use juniper_axum::{extract::JuniperRequest, graphiql, response::JuniperResponse, subscriptions};
use juniper_graphql_ws::{ConnectionConfig, Schema as GraphqlSchema};
use opentelemetry::{Context as OtelContext, trace::TraceContextExt};
use playground_telemetry::semconv;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, mpsc, watch};
use tokio_stream::wrappers::ReceiverStream;
use tracing::Instrument;
use tracing_opentelemetry::OpenTelemetrySpanExt;

use crate::{
    application::{CatalogProductsRequest, StoreContext},
    domain::*,
    infrastructure::*,
};

const SUBSCRIPTION_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) struct SubscriptionRegistry {
    cancelled: AtomicBool,
    shutdown: watch::Sender<bool>,
    tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

impl SubscriptionRegistry {
    pub(crate) fn new() -> Self {
        let (shutdown, _) = watch::channel(false);
        Self {
            cancelled: AtomicBool::new(false),
            shutdown,
            tasks: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn subscribe(&self) -> watch::Receiver<bool> {
        self.shutdown.subscribe()
    }

    pub(crate) fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        let _ = self.shutdown.send(true);
    }

    pub(crate) async fn spawn<F>(&self, task: F) -> bool
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let mut tasks = self.tasks.lock().await;
        tasks.retain(|task| !task.is_finished());
        if self.cancelled.load(Ordering::Acquire) {
            return false;
        }
        tasks.push(tokio::spawn(task));
        true
    }

    pub(crate) async fn shutdown(&self) -> anyhow::Result<()> {
        self.cancel();
        let mut tasks = {
            let mut registered = self.tasks.lock().await;
            std::mem::take(&mut *registered)
        };
        let deadline = Instant::now() + SUBSCRIPTION_DRAIN_TIMEOUT;
        let mut first_error = None;

        for task in &mut tasks {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let result = if task.is_finished() {
                Ok((&mut *task).await)
            } else {
                tokio::time::timeout(remaining, &mut *task).await
            };
            match result {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    tracing::error!(error = %error, "storefront price subscription task failed");
                    if first_error.is_none() {
                        first_error = Some(anyhow!(
                            "storefront price subscription task failed: {error}"
                        ));
                    }
                }
                Err(_) => {
                    tracing::error!(
                        "storefront price subscription did not stop before the drain deadline; aborting"
                    );
                    task.abort();
                    let _ = (&mut *task).await;
                    if first_error.is_none() {
                        first_error = Some(anyhow!(
                            "storefront price subscription did not stop before the drain deadline"
                        ));
                    }
                }
            }
        }

        first_error.map_or(Ok(()), Err)
    }
}

async fn run_price_change_subscription(
    context: StoreContext,
    subscription_context: OtelContext,
    tenant_id: String,
    requested_sku: Option<String>,
    sender: mpsc::Sender<PriceChange>,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut sequence = 0_i64;
    let mut baseline_captured = false;

    loop {
        if sender.is_closed() || *shutdown.borrow() {
            return;
        }
        let connection = tokio::select! {
            result = PriceChangeListener::connect(&context.database_url) => result,
            changed = shutdown.changed() => {
                let _ = changed;
                return;
            }
        };
        let mut listener = match connection {
            Ok(listener) => listener,
            Err(error) => {
                tracing::warn!(error = %error, "storefront price LISTEN unavailable; reconnecting");
                if wait_for_price_change_reconnect(&sender, &mut shutdown).await {
                    return;
                }
                continue;
            }
        };

        if !baseline_captured {
            let baseline = tokio::select! {
                result = listener.latest_sequence(&tenant_id, requested_sku.as_deref()) => result,
                changed = shutdown.changed() => {
                    let _ = changed;
                    return;
                }
            };
            match baseline {
                Ok(latest_sequence) => {
                    sequence = latest_sequence;
                    baseline_captured = true;
                }
                Err(error) => {
                    tracing::warn!(error = %error, "storefront price journal baseline unavailable; reconnecting");
                    if wait_for_price_change_reconnect(&sender, &mut shutdown).await {
                        return;
                    }
                    continue;
                }
            }
        }

        'connection: loop {
            let events_result = tokio::select! {
                result = listener.events_after(&tenant_id, requested_sku.as_deref(), sequence) => result,
                changed = shutdown.changed() => {
                    let _ = changed;
                    return;
                }
            };
            let (events, notification_received) = match events_result {
                Ok(events) => events,
                Err(error) => {
                    tracing::warn!(error = %error, "storefront price journal replay failed; reconnecting");
                    break 'connection;
                }
            };

            for event in events {
                if event.sequence <= sequence {
                    continue;
                }
                let event_sequence = event.sequence;
                let span = resolver_span(
                    "Subscription.priceChanges",
                    "Subscription.priceChanges",
                    "subscription",
                );
                if subscription_context.span().span_context().is_valid() {
                    span.add_link(subscription_context.span().span_context().clone());
                }
                let projection = context
                    .price_change_from_notification(event)
                    .instrument(span);
                tokio::pin!(projection);
                let projection = tokio::select! {
                    result = &mut projection => result,
                    changed = shutdown.changed() => {
                        let _ = changed;
                        return;
                    }
                };
                match projection {
                    Ok(Some(change)) => {
                        sequence = event_sequence;
                        if sender.send(change).await.is_err() {
                            return;
                        }
                    }
                    Ok(None) => sequence = event_sequence,
                    Err(error) => {
                        tracing::warn!(error = %error, "storefront price journal event projection failed; reconnecting");
                        break 'connection;
                    }
                }
            }

            if notification_received {
                continue 'connection;
            }

            tokio::select! {
                result = listener.wait_for_price_change() => {
                    if let Err(error) = result {
                        tracing::warn!(error = %error, "storefront price LISTEN connection lost; reconnecting");
                        break 'connection;
                    }
                }
                _ = sender.closed() => return,
                changed = shutdown.changed() => {
                    let _ = changed;
                    return;
                }
            }
        }

        if wait_for_price_change_reconnect(&sender, &mut shutdown).await {
            return;
        }
    }
}

async fn wait_for_price_change_reconnect(
    sender: &mpsc::Sender<PriceChange>,
    shutdown: &mut watch::Receiver<bool>,
) -> bool {
    if sender.is_closed() || *shutdown.borrow() {
        return true;
    }
    // Backoff is only for failed connections; journal reads happen after LISTEN wake-ups.
    tokio::select! {
        _ = tokio::time::sleep(PRICE_CHANGE_RECONNECT_DELAY) => false,
        _ = sender.closed() => true,
        changed = shutdown.changed() => {
            let _ = changed;
            true
        }
    }
}

struct Query;

#[derive(Clone, Copy, Debug, Eq, PartialEq, juniper::GraphQLEnum)]
#[graphql(name = "ProductSort")]
pub enum ProductSort {
    Featured,
    Newest,
    PriceAsc,
    PriceDesc,
    Relevance,
}

impl ProductSort {
    pub(crate) const fn catalog_value(self) -> &'static str {
        match self {
            Self::Featured => "FEATURED",
            Self::Newest => "NEWEST",
            Self::PriceAsc => "PRICE_ASC",
            Self::PriceDesc => "PRICE_DESC",
            Self::Relevance => "RELEVANCE",
        }
    }
}

#[graphql_object(context = StoreContext)]
impl Query {
    async fn product(
        context: &StoreContext,
        sku: String,
        tenant_id: Option<String>,
        segment: Option<String>,
    ) -> FieldResult<Option<Product>> {
        let span = resolver_span("Query.product", "Query.product", "query");
        async move {
            let tenant_id = context
                .resolve_tenant_id(tenant_id.as_deref())
                .map_err(field_error)?;
            context
                .catalog_product(&tenant_id, &sku, segment.as_deref().unwrap_or("standard"))
                .await
                .map_err(field_error)
        }
        .instrument(span)
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn products(
        context: &StoreContext,
        search: Option<String>,
        category: Option<String>,
        sort: Option<ProductSort>,
        tenant_id: Option<String>,
        page: Option<i32>,
        size: Option<i32>,
        segment: Option<String>,
    ) -> FieldResult<ProductPage> {
        let span = resolver_span("Query.products", "Query.products", "query");
        async move {
            let tenant_id = context
                .resolve_tenant_id(tenant_id.as_deref())
                .map_err(field_error)?;
            let page = page.unwrap_or(0).max(0);
            let size = size.unwrap_or(20).clamp(1, 100);
            context
                .catalog_products(CatalogProductsRequest {
                    tenant_id: &tenant_id,
                    search: search.as_deref(),
                    category: category.as_deref(),
                    sort,
                    page,
                    size,
                    segment: segment.as_deref().unwrap_or("standard"),
                })
                .await
                .map_err(field_error)
        }
        .instrument(span)
        .await
    }

    async fn categories(
        context: &StoreContext,
        tenant_id: Option<String>,
    ) -> FieldResult<Vec<Category>> {
        let span = resolver_span("Query.categories", "Query.categories", "query");
        async move {
            let tenant_id = context
                .resolve_tenant_id(tenant_id.as_deref())
                .map_err(field_error)?;
            context
                .catalog_categories(&tenant_id)
                .await
                .map_err(field_error)
        }
        .instrument(span)
        .await
    }

    async fn quote(context: &StoreContext, input: QuoteInput) -> FieldResult<Quote> {
        let span = resolver_span("Query.quote", "Query.quote", "query");
        async move { context.quote(&input).await.map_err(field_error) }
            .instrument(span)
            .await
    }

    async fn cart(
        context: &StoreContext,
        tenant_id: Option<String>,
        customer_id: Option<String>,
        session_id: Option<String>,
    ) -> FieldResult<Option<Cart>> {
        let span = resolver_span("Query.cart", "Query.cart", "query");
        async move {
            let tenant_id = context
                .resolve_tenant_id(tenant_id.as_deref())
                .map_err(field_error)?;
            let customer_id = customer_id
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| field_error("customer_id is required"))?;
            context
                .cart(&tenant_id, customer_id, session_id.as_deref())
                .await
                .map_err(field_error)
        }
        .instrument(span)
        .await
    }

    async fn order(
        context: &StoreContext,
        order_id: String,
        tenant_id: Option<String>,
        customer_id: Option<String>,
        session_id: Option<String>,
    ) -> FieldResult<Option<Order>> {
        let span = resolver_span("Query.order", "Query.order", "query");
        async move {
            let tenant_id = context
                .resolve_tenant_id(tenant_id.as_deref())
                .map_err(field_error)?;
            let customer_id = customer_id
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| field_error("customer_id is required"))?;
            context
                .order(&tenant_id, &order_id, customer_id, session_id.as_deref())
                .await
                .map_err(field_error)
        }
        .instrument(span)
        .await
    }

    async fn orders(
        context: &StoreContext,
        tenant_id: Option<String>,
        customer_id: Option<String>,
        session_id: Option<String>,
    ) -> FieldResult<Vec<Order>> {
        let span = resolver_span("Query.orders", "Query.orders", "query");
        async move {
            let tenant_id = context
                .resolve_tenant_id(tenant_id.as_deref())
                .map_err(field_error)?;
            context
                .orders(&tenant_id, customer_id.as_deref(), session_id.as_deref())
                .await
                .map_err(field_error)
        }
        .instrument(span)
        .await
    }

    async fn analytics(
        context: &StoreContext,
        tenant_id: Option<String>,
        event_name: Option<String>,
        limit: Option<i32>,
    ) -> FieldResult<Vec<AnalyticsEvent>> {
        let span = resolver_span("Query.analytics", "Query.analytics", "query");
        async move {
            let tenant_id = context
                .resolve_tenant_id(tenant_id.as_deref())
                .map_err(field_error)?;
            context
                .analytics(
                    &tenant_id,
                    event_name.as_deref(),
                    limit.unwrap_or(50).clamp(1, 100),
                )
                .await
                .map_err(field_error)
        }
        .instrument(span)
        .await
    }

    async fn analytics_events(
        context: &StoreContext,
        tenant_id: Option<String>,
        event_name: Option<String>,
        limit: Option<i32>,
    ) -> FieldResult<Vec<AnalyticsEvent>> {
        let span = resolver_span("Query.analyticsEvents", "Query.analyticsEvents", "query");
        async move {
            let tenant_id = context
                .resolve_tenant_id(tenant_id.as_deref())
                .map_err(field_error)?;
            context
                .analytics(
                    &tenant_id,
                    event_name.as_deref(),
                    limit.unwrap_or(50).clamp(1, 100),
                )
                .await
                .map_err(field_error)
        }
        .instrument(span)
        .await
    }

    async fn analytics_summary(
        context: &StoreContext,
        tenant_id: Option<String>,
        event_name: Option<String>,
    ) -> FieldResult<AnalyticsSummary> {
        let span = resolver_span("Query.analyticsSummary", "Query.analyticsSummary", "query");
        async move {
            let tenant_id = context
                .resolve_tenant_id(tenant_id.as_deref())
                .map_err(field_error)?;
            context
                .analytics_summary(&tenant_id, event_name.as_deref())
                .await
                .map_err(field_error)
        }
        .instrument(span)
        .await
    }
}

struct Mutation;

#[graphql_object(context = StoreContext)]
impl Mutation {
    async fn checkout(context: &StoreContext, input: CheckoutInput) -> FieldResult<CheckoutResult> {
        let span = resolver_span("Mutation.checkout", "Mutation.checkout", "mutation");
        async move {
            context
                .checkout(&input)
                .await
                .and_then(|value| CheckoutResult::try_from_value(&value))
                .map_err(field_error)
        }
        .instrument(span)
        .await
    }

    async fn add_cart_item(
        context: &StoreContext,
        input: AddCartItemInput,
    ) -> FieldResult<CartItemAdded> {
        let span = resolver_span("Mutation.addCartItem", "Mutation.addCartItem", "mutation");
        async move { context.add_cart_item(&input).await.map_err(field_error) }
            .instrument(span)
            .await
    }

    async fn record_analytics(
        context: &StoreContext,
        input: AnalyticsInput,
    ) -> FieldResult<AnalyticsAck> {
        let span = resolver_span(
            "Mutation.recordAnalytics",
            "Mutation.recordAnalytics",
            "mutation",
        );
        async move { context.record_analytics(&input).await.map_err(field_error) }
            .instrument(span)
            .await
    }
}

struct Subscription;
type PriceChangeStream = BoxStream<'static, Result<PriceChange, FieldError>>;

#[graphql_subscription(context = StoreContext)]
impl Subscription {
    async fn price_changes(
        context: &StoreContext,
        tenant_id: Option<String>,
        sku: Option<String>,
    ) -> PriceChangeStream {
        let tenant_id = match context.resolve_tenant_id(tenant_id.as_deref()) {
            Ok(tenant_id) => tenant_id,
            Err(_error) => {
                return Box::pin(futures::stream::once(async move {
                    Err(FieldError::new(
                        "storefront request could not be completed",
                        juniper::Value::null(),
                    ))
                }));
            }
        };
        let (sender, receiver) = mpsc::channel(16);
        let subscription_context = context.outbound_context();
        let task = run_price_change_subscription(
            context.clone(),
            subscription_context,
            tenant_id,
            sku,
            sender,
            context.subscription_registry.subscribe(),
        );
        context.subscription_registry.spawn(task).await;
        Box::pin(ReceiverStream::new(receiver).map(Ok::<PriceChange, FieldError>))
    }
}

type Schema = RootNode<Query, Mutation, Subscription>;

async fn graphql_handler(
    Extension(schema): Extension<Arc<Schema>>,
    Extension(base_context): Extension<StoreContext>,
    headers: HeaderMap,
    JuniperRequest(request): JuniperRequest,
) -> JuniperResponse {
    let context = base_context.for_request(&headers);
    JuniperResponse(request.execute(schema.root_node(), &context).await)
}

async fn checkout_route(
    State(base_context): State<StoreContext>,
    headers: HeaderMap,
    Json(input): Json<CheckoutInput>,
) -> ApiResult {
    let context = base_context.for_request(&headers);
    context.checkout(&input).await.map(Json).map_err(api_error)
}

#[derive(Debug, Deserialize)]
struct OrderQuery {
    tenant_id: Option<String>,
    #[serde(default)]
    customer_id: Option<String>,
    #[serde(default)]
    session_id: Option<String>,
}

async fn orders_route(
    State(base_context): State<StoreContext>,
    headers: HeaderMap,
    QueryParams(query): QueryParams<OrderQuery>,
) -> ApiResult {
    let context = base_context.for_request(&headers);
    let tenant_id = context
        .resolve_tenant_id(query.tenant_id.as_deref())
        .map_err(api_error)?;
    let session_id = context
        .resolve_session_id(query.session_id.as_deref())
        .map_err(api_error)?;
    let orders = context
        .orders(
            &tenant_id,
            query.customer_id.as_deref(),
            session_id.as_deref(),
        )
        .await
        .map_err(api_error)?;
    serde_json::to_value(json!({
        "orders": orders,
        "tenant_id": tenant_id,
        "customer_id": query.customer_id,
        "session_id": session_id,
    }))
    .map(Json)
    .map_err(|error| api_error(anyhow!(error)))
}

async fn order_route(
    State(base_context): State<StoreContext>,
    headers: HeaderMap,
    Path(order_id): Path<String>,
    QueryParams(query): QueryParams<OrderQuery>,
) -> ApiResult {
    let context = base_context.for_request(&headers);
    let tenant_id = context
        .resolve_tenant_id(query.tenant_id.as_deref())
        .map_err(api_error)?;
    let customer_id = query
        .customer_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| api_error(anyhow!("customer_id is required")))?;
    let session_id = context
        .resolve_session_id(query.session_id.as_deref())
        .map_err(api_error)?;
    let order = context
        .order(&tenant_id, &order_id, customer_id, session_id.as_deref())
        .await
        .map_err(api_error)?;
    match order {
        Some(order) => serde_json::to_value(order)
            .map(Json)
            .map_err(|error| api_error(anyhow!(error))),
        None => Err((
            StatusCode::NOT_FOUND,
            Json(json!({"error": "order_not_found", "order_id": order_id})),
        )),
    }
}

#[derive(Debug, Deserialize)]
struct AnalyticsQuery {
    tenant_id: Option<String>,
    event_name: Option<String>,
    limit: Option<i32>,
}

async fn analytics_route(
    State(base_context): State<StoreContext>,
    headers: HeaderMap,
    QueryParams(query): QueryParams<AnalyticsQuery>,
) -> ApiResult {
    let context = base_context.for_request(&headers);
    let tenant_id = context
        .resolve_tenant_id(query.tenant_id.as_deref())
        .map_err(api_error)?;
    let events = context
        .analytics(
            &tenant_id,
            query.event_name.as_deref(),
            query.limit.unwrap_or(50).clamp(1, 100),
        )
        .await
        .map_err(api_error)?;
    serde_json::to_value(events)
        .map(Json)
        .map_err(|error| api_error(anyhow!(error)))
}

async fn record_analytics_route(
    State(base_context): State<StoreContext>,
    headers: HeaderMap,
    Json(input): Json<AnalyticsInput>,
) -> ApiResult {
    let context = base_context.for_request(&headers);
    context
        .record_analytics(&input)
        .await
        .map(|ack| Json(json!({"event_key": ack.event_key, "status": ack.status})))
        .map_err(api_error)
}

type ApiResult = Result<Json<Value>, (StatusCode, Json<Value>)>;

async fn subscriptions_route(
    Extension(schema): Extension<Arc<Schema>>,
    State(base_context): State<StoreContext>,
    headers: HeaderMap,
    websocket: WebSocketUpgrade,
) -> Response {
    // The HTTP upgrade is request-bounded; the resulting long-lived stream
    // gets independent per-hop limits instead of inheriting an expired
    // one-shot request deadline.
    let context = StoreContext {
        request_deadline: None,
        ..base_context.for_request(&headers)
    };
    websocket
        .protocols(["graphql-transport-ws", "graphql-ws"])
        .on_upgrade(move |socket| {
            subscriptions::serve_ws(socket, schema, ConnectionConfig::new(context))
        })
}

#[derive(Clone, Debug)]
struct CorsConfig {
    allowed_origins: Vec<HeaderValue>,
}

impl CorsConfig {
    fn from_env() -> Self {
        match std::env::var("WEB_ORIGIN") {
            Ok(origins) => Self::from_origins(origins.split(',').map(str::trim)),
            Err(_) => Self::from_origins([DEFAULT_WEB_ORIGIN]),
        }
    }

    fn from_origins<I, S>(origins: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        Self {
            allowed_origins: origins
                .into_iter()
                .filter_map(|origin| origin.as_ref().parse::<HeaderValue>().ok())
                .collect(),
        }
    }

    fn allowed_origin(&self, origin: &HeaderValue) -> Option<HeaderValue> {
        self.allowed_origins
            .iter()
            .find(|allowed| allowed.as_bytes() == origin.as_bytes())
            .cloned()
    }
}

async fn cors_middleware(config: CorsConfig, request: Request, next: middleware::Next) -> Response {
    let Some(origin) = request.headers().get(header::ORIGIN).cloned() else {
        return next.run(request).await;
    };
    let Some(allowed_origin) = config.allowed_origin(&origin) else {
        return StatusCode::FORBIDDEN.into_response();
    };

    if request.method() == Method::OPTIONS {
        if !preflight_is_allowed(request.headers()) {
            return StatusCode::FORBIDDEN.into_response();
        }
        let mut response = StatusCode::NO_CONTENT.into_response();
        add_cors_headers(response.headers_mut(), &allowed_origin, true);
        return response;
    }

    let mut response = next.run(request).await;
    add_cors_headers(response.headers_mut(), &allowed_origin, false);
    response
}

fn preflight_is_allowed(headers: &HeaderMap) -> bool {
    let method_allowed = headers
        .get(header::ACCESS_CONTROL_REQUEST_METHOD)
        .map(|value| {
            Method::from_bytes(value.as_bytes())
                .map(|method| matches!(method, Method::GET | Method::POST))
                .unwrap_or(false)
        })
        .unwrap_or(true);
    let headers_allowed = headers
        .get(header::ACCESS_CONTROL_REQUEST_HEADERS)
        .map(request_headers_are_allowed)
        .unwrap_or(true);
    method_allowed && headers_allowed
}

fn request_headers_are_allowed(value: &HeaderValue) -> bool {
    let Ok(value) = value.to_str() else {
        return false;
    };
    value.split(',').all(|name| {
        let name = name.trim();
        !name.is_empty()
            && CORS_ALLOWED_HEADER_NAMES
                .iter()
                .any(|allowed| allowed.eq_ignore_ascii_case(name))
    })
}

fn add_cors_headers(headers: &mut HeaderMap, origin: &HeaderValue, preflight: bool) {
    headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin.clone());
    headers.append(header::VARY, HeaderValue::from_static("Origin"));
    if preflight {
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static(CORS_ALLOWED_METHODS),
        );
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_HEADERS,
            HeaderValue::from_static(CORS_ALLOWED_HEADERS),
        );
        headers.insert(
            header::ACCESS_CONTROL_MAX_AGE,
            HeaderValue::from_static(CORS_MAX_AGE_SECONDS),
        );
        headers.append(
            header::VARY,
            HeaderValue::from_static("Access-Control-Request-Method"),
        );
        headers.append(
            header::VARY,
            HeaderValue::from_static("Access-Control-Request-Headers"),
        );
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ClientSafeError {
    code: &'static str,
    message: &'static str,
}

const DOWNSTREAM_ERROR: ClientSafeError = ClientSafeError {
    code: "downstream_error",
    message: "downstream request failed",
};

fn client_safe_downstream_error(body: &str) -> ClientSafeError {
    let code = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|payload| {
            payload
                .get("error")
                .and_then(Value::as_str)
                .map(str::trim)
                .map(str::to_owned)
        });

    match code.as_deref() {
        Some("invalid_request") => ClientSafeError {
            code: "invalid_request",
            message: "the request is invalid",
        },
        Some("invalid_identity") => ClientSafeError {
            code: "invalid_identity",
            message: "the request identity is invalid",
        },
        Some("invalid_tenant") => ClientSafeError {
            code: "invalid_tenant",
            message: "the tenant identity is invalid",
        },
        Some("tenant_required") => ClientSafeError {
            code: "tenant_required",
            message: "tenant identity is required",
        },
        Some("identity_conflict") => ClientSafeError {
            code: "identity_conflict",
            message: "request identity conflicts with the authenticated identity",
        },
        Some("customer_required") => ClientSafeError {
            code: "customer_required",
            message: "customer identity is required",
        },
        Some("invalid_session") => ClientSafeError {
            code: "invalid_session",
            message: "the session identity is invalid",
        },
        Some("invalid_cart") => ClientSafeError {
            code: "invalid_cart",
            message: "the cart is invalid",
        },
        Some("cart_not_found") => ClientSafeError {
            code: "cart_not_found",
            message: "the cart was not found",
        },
        Some("invalid_cart_item") => ClientSafeError {
            code: "invalid_cart_item",
            message: "the cart item is invalid",
        },
        Some("cart_item_not_found") => ClientSafeError {
            code: "cart_item_not_found",
            message: "the cart item was not found",
        },
        Some("invalid_cart_quantity" | "invalid_quantity") => ClientSafeError {
            code: "invalid_quantity",
            message: "the item quantity is invalid",
        },
        Some("cart_quantity_limit") => ClientSafeError {
            code: "cart_quantity_limit",
            message: "the item quantity exceeds the allowed limit",
        },
        Some("invalid_currency") => ClientSafeError {
            code: "invalid_currency",
            message: "the currency is invalid",
        },
        Some("invalid_items") => ClientSafeError {
            code: "invalid_items",
            message: "the checkout items are invalid",
        },
        Some("invalid_sku" | "invalid_cart_sku") => ClientSafeError {
            code: "invalid_sku",
            message: "the product SKU is invalid",
        },
        Some("invalid_promotion") => ClientSafeError {
            code: "invalid_promotion",
            message: "the promotion is invalid",
        },
        Some("payment_method_required") => ClientSafeError {
            code: "payment_method_required",
            message: "a payment method is required",
        },
        Some("invalid_payment_method_token") => ClientSafeError {
            code: "invalid_payment_method_token",
            message: "the payment method is invalid",
        },
        Some("invalid_payment_method_type") => ClientSafeError {
            code: "invalid_payment_method_type",
            message: "the payment method type is invalid",
        },
        Some("checkout_in_progress") => ClientSafeError {
            code: "checkout_in_progress",
            message: "checkout is already in progress",
        },
        Some("checkout_request_conflict") => ClientSafeError {
            code: "checkout_request_conflict",
            message: "the checkout request conflicts with an existing request",
        },
        Some("order_not_found") => ClientSafeError {
            code: "order_not_found",
            message: "the order was not found",
        },
        Some("order_state_conflict") => ClientSafeError {
            code: "order_state_conflict",
            message: "the order cannot be changed in its current state",
        },
        Some("product_not_found") => ClientSafeError {
            code: "product_not_found",
            message: "the product was not found",
        },
        Some("payment_invalid_request") => ClientSafeError {
            code: "payment_invalid_request",
            message: "the payment request is invalid",
        },
        Some("payment_invalid_method") => ClientSafeError {
            code: "payment_invalid_method",
            message: "the payment method was rejected",
        },
        Some("payment_invalid_state") => ClientSafeError {
            code: "payment_invalid_state",
            message: "the payment is in an invalid state",
        },
        Some("payment_not_found") => ClientSafeError {
            code: "payment_not_found",
            message: "the payment was not found",
        },
        Some("payment_already_processed") => ClientSafeError {
            code: "payment_already_processed",
            message: "the payment was already processed",
        },
        Some("payment_declined") => ClientSafeError {
            code: "payment_declined",
            message: "the payment was declined",
        },
        Some("payment_insufficient_funds") => ClientSafeError {
            code: "payment_insufficient_funds",
            message: "the payment could not be completed with the available funds",
        },
        Some("payment_pending") => ClientSafeError {
            code: "payment_pending",
            message: "payment confirmation is pending",
        },
        Some("inventory_unavailable") => ClientSafeError {
            code: "inventory_unavailable",
            message: "inventory is temporarily unavailable",
        },
        Some("payment_provider_unavailable") => ClientSafeError {
            code: "payment_provider_unavailable",
            message: "the payment provider is temporarily unavailable",
        },
        Some("payment_timeout") => ClientSafeError {
            code: "payment_timeout",
            message: "payment confirmation timed out",
        },
        Some("catalog_unavailable") => ClientSafeError {
            code: "catalog_unavailable",
            message: "the catalog is temporarily unavailable",
        },
        Some("pricing_unavailable") => ClientSafeError {
            code: "pricing_unavailable",
            message: "pricing is temporarily unavailable",
        },
        Some("recommendation_unavailable") => ClientSafeError {
            code: "recommendation_unavailable",
            message: "recommendations are temporarily unavailable",
        },
        Some("messaging_unavailable") => ClientSafeError {
            code: "messaging_unavailable",
            message: "messaging is temporarily unavailable",
        },
        _ => DOWNSTREAM_ERROR,
    }
}

const REDACTED_LOG_VALUE: &str = "[REDACTED]";

fn redacted_downstream_body(body: &str) -> String {
    let Ok(payload) = serde_json::from_str::<Value>(body) else {
        return format!("non_json_body(bytes={})", body.len());
    };
    serde_json::to_string(&redact_downstream_value(payload, None))
        .unwrap_or_else(|_| REDACTED_LOG_VALUE.to_owned())
}

fn redact_downstream_value(value: Value, key: Option<&str>) -> Value {
    match value {
        Value::Object(fields) => Value::Object(
            fields
                .into_iter()
                .map(|(field, value)| {
                    let value = if matches!(field.as_str(), "error" | "code" | "type") {
                        redact_downstream_value(value, Some(field.as_str()))
                    } else {
                        Value::String(REDACTED_LOG_VALUE.to_owned())
                    };
                    (field, value)
                })
                .collect(),
        ),
        Value::Array(values) => Value::Array(
            values
                .into_iter()
                .map(|value| redact_downstream_value(value, None))
                .collect(),
        ),
        Value::String(value) if key.is_some() => Value::String(redact_log_token(&value)),
        Value::String(_) => Value::String(REDACTED_LOG_VALUE.to_owned()),
        value => value,
    }
}

fn redact_log_token(value: &str) -> String {
    value
        .trim()
        .chars()
        .take(64)
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.' | ':') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

fn api_error(error: anyhow::Error) -> (StatusCode, Json<Value>) {
    if let Some(downstream) = error.downcast_ref::<DownstreamHttpError>() {
        let client_error = client_safe_downstream_error(&downstream.body);
        let status = if downstream.status.is_client_error() {
            downstream.status
        } else {
            StatusCode::BAD_GATEWAY
        };
        tracing::warn!(
            status = %downstream.status,
            client_error = client_error.code,
            diagnostic = %redacted_downstream_body(&downstream.body),
            "storefront downstream returned a typed error"
        );
        return (
            status,
            Json(json!({
                "error": client_error.code,
                "message": client_error.message
            })),
        );
    }
    let error_text = error.to_string();
    if error_text.contains("tenant identity conflicts") {
        return (
            StatusCode::CONFLICT,
            Json(json!({
                "error": "identity_conflict",
                "message": "tenant identity conflicts with the supplied customer"
            })),
        );
    }
    if error_text.contains("tenant_id is required") || error_text.contains("tenant_id is invalid") {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "tenant_required",
                "message": "tenant identity is required and must be valid"
            })),
        );
    }
    tracing::error!(error = %error, "storefront downstream request failed");
    (
        StatusCode::BAD_GATEWAY,
        Json(json!({"error": "downstream_unavailable"})),
    )
}

fn field_error(error: impl std::fmt::Display) -> FieldError {
    playground_telemetry::mark_span_error("storefront_upstream_error");
    tracing::warn!(error = %error, "storefront GraphQL resolver failed");
    FieldError::new(
        "storefront request could not be completed",
        juniper::Value::null(),
    )
}

fn resolver_span(name: &'static str, path: &'static str, operation: &'static str) -> tracing::Span {
    tracing::info_span!(
        "graphql.resolver",
        otel.kind = semconv::SPAN_KIND_INTERNAL,
        graphql.operation.type = operation,
        graphql.operation.name = "Storefront",
        graphql.document = "storefront-gateway",
        graphql.field.name = name,
        graphql.field.path = path,
    )
}

fn app(schema: Arc<Schema>, context: StoreContext) -> Router {
    app_with_cors(schema, context, CorsConfig::from_env())
}

fn app_with_cors(schema: Arc<Schema>, context: StoreContext, cors: CorsConfig) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(healthz))
        .route("/analytics/readyz", get(analytics_readyz))
        .route("/graphql", get(graphql_handler).post(graphql_handler))
        .route("/subscriptions", get(subscriptions_route))
        .route("/graphiql", get(graphiql("/graphql", "/subscriptions")))
        .route("/api/checkout", post(checkout_route))
        .route("/api/orders", get(orders_route))
        .route("/api/orders/{order_id}", get(order_route))
        .route(
            "/api/analytics",
            get(analytics_route).post(record_analytics_route),
        )
        .layer(Extension(schema))
        .layer(Extension(context.clone()))
        .with_state(context)
        .layer(middleware::from_fn(
            move |request: Request, next: middleware::Next| {
                let cors = cors.clone();
                async move { cors_middleware(cors, request, next).await }
            },
        ))
        .layer(middleware::from_fn(
            playground_telemetry::http_server_observability,
        ))
        .layer(middleware::from_fn(request_timeout_middleware))
}

async fn request_timeout_middleware(request: Request, next: middleware::Next) -> Response {
    timed_response(STOREFRONT_REQUEST_TIMEOUT, next.run(request)).await
}

async fn timed_response<F>(timeout: Duration, future: F) -> Response
where
    F: Future<Output = Response>,
{
    match tokio::time::timeout(timeout, future).await {
        Ok(response) => response,
        Err(_) => (
            StatusCode::REQUEST_TIMEOUT,
            Json(json!({
                "error": "request_timeout",
                "message": "storefront request exceeded its deadline"
            })),
        )
            .into_response(),
    }
}

async fn healthz(State(context): State<StoreContext>) -> impl IntoResponse {
    context.readiness().await.into_response()
}

async fn analytics_readyz(State(context): State<StoreContext>) -> impl IntoResponse {
    context.analytics_readiness().await.into_response()
}

pub(crate) async fn run() -> anyhow::Result<()> {
    let telemetry = playground_telemetry::init("storefront")?;
    let context = StoreContext::default();
    let subscriptions = context.subscription_registry.clone();
    let schema = Arc::new(Schema::new(Query, Mutation, Subscription));
    let app = app(schema, context);
    let address = env_or("STOREFRONT_ADDR", "0.0.0.0:8094");
    let listener = tokio::net::TcpListener::bind(&address).await?;
    tracing::info!(%address, "storefront GraphQL ready");
    let shutdown_subscriptions = subscriptions.clone();
    let server = axum::serve(listener, app).with_graceful_shutdown(async move {
        playground_telemetry::shutdown_signal().await;
        shutdown_subscriptions.cancel();
    });
    let server_result = server.await;
    let subscription_result = subscriptions.shutdown().await;
    telemetry.shutdown();
    server_result?;
    subscription_result?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode, Uri},
    };
    use opentelemetry::{
        Context as OtelContext, KeyValue,
        baggage::BaggageExt,
        global,
        propagation::{Injector, TextMapPropagator, text_map_propagator::FieldIter},
        trace::{SpanContext, SpanId, TraceContextExt, TraceFlags, TraceId, TraceState},
    };
    use playground_proto::pricing::v1::{
        Money as ProtoMoney, QuoteLine as ProtoQuoteLine, QuoteResponse, QuoteStatus,
    };
    use std::collections::HashMap;
    use tower::ServiceExt;

    const TEST_TENANT: &str = "tenant-acme";

    #[tokio::test]
    async fn subscription_registry_cancels_and_joins_registered_tasks() {
        let empty_registry = SubscriptionRegistry::new();
        empty_registry
            .shutdown()
            .await
            .expect("empty registry shutdown");
        assert!(!empty_registry.spawn(async {}).await);

        let registry = SubscriptionRegistry::new();
        let completed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let task_completed = completed.clone();
        let mut shutdown = registry.subscribe();
        assert!(
            registry
                .spawn(async move {
                    let _ = shutdown.changed().await;
                    task_completed.store(true, std::sync::atomic::Ordering::Release);
                })
                .await
        );

        registry.shutdown().await.expect("subscription tasks drain");

        assert!(completed.load(std::sync::atomic::Ordering::Acquire));
        assert!(!registry.spawn(async {}).await);
    }

    #[derive(Debug)]
    struct HeaderPropagator;

    impl TextMapPropagator for HeaderPropagator {
        fn inject_context(&self, context: &OtelContext, injector: &mut dyn Injector) {
            let span = context.span();
            let span_context = span.span_context();
            if span_context.is_valid() {
                injector.set(
                    "traceparent",
                    format!(
                        "00-{}-{}-{:02x}",
                        span_context.trace_id(),
                        span_context.span_id(),
                        span_context.trace_flags().to_u8()
                    ),
                );
                let tracestate = span_context.trace_state().header();
                if !tracestate.is_empty() {
                    injector.set("tracestate", tracestate);
                }
            }
            let baggage = context
                .baggage()
                .iter()
                .map(|(key, (value, _))| format!("{}={value}", key.as_str()))
                .collect::<Vec<_>>();
            if !baggage.is_empty() {
                injector.set("baggage", baggage.join(","));
            }
        }

        fn extract_with_context(
            &self,
            context: &OtelContext,
            _extractor: &dyn opentelemetry::propagation::Extractor,
        ) -> OtelContext {
            context.clone()
        }

        fn fields(&self) -> FieldIter<'_> {
            FieldIter::new(&[])
        }
    }

    #[test]
    fn request_context_preserves_inbound_trace_and_baggage() {
        let trace_id =
            TraceId::from_hex("4bf92f3577b34da6a3ce929d0e0e4736").expect("valid trace id");
        let inbound = OtelContext::new()
            .with_remote_span_context(SpanContext::new(
                trace_id,
                SpanId::from_hex("00f067aa0ba902b7").expect("valid span id"),
                TraceFlags::SAMPLED,
                true,
                TraceState::default(),
            ))
            .with_baggage([
                KeyValue::new(semconv::TENANT_ID, "tenant-request"),
                KeyValue::new(semconv::USER_TIER, "premium"),
                KeyValue::new("customer.segment", "vip"),
                KeyValue::new("request.priority", "urgent"),
                KeyValue::new("feature.variant", "preserved"),
                KeyValue::new("subscription.request", "dropped"),
            ]);

        let context = StoreContext::default().with_request_context(&inbound);

        assert_eq!(
            context.propagation.span().span_context().trace_id(),
            trace_id
        );
        assert_eq!(
            baggage_or(&context.propagation, semconv::TENANT_ID, ""),
            "tenant-request"
        );
        assert_eq!(
            baggage_or(&context.propagation, "feature.variant", ""),
            "preserved"
        );
        assert_eq!(
            baggage_or(&context.propagation, "subscription.request", ""),
            ""
        );
    }

    #[tokio::test]
    async fn healthz_reports_unavailable_dependencies() {
        let unavailable = "http://127.0.0.1:1";
        let context = StoreContext {
            catalog_url: format!("{unavailable}/graphql"),
            pricing_endpoint: unavailable.to_owned(),
            checkout_endpoint: unavailable.to_owned(),
            clickhouse_url: unavailable.to_owned(),
            database_url: "postgres://postgres:playground@127.0.0.1:1/playground".to_owned(),
            ..StoreContext::default()
        };
        let response = app_with_cors(
            Arc::new(Schema::new(Query, Mutation, Subscription)),
            context,
            CorsConfig::from_origins(["http://web.test"]),
        )
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/healthz")
                .body(Body::empty())
                .expect("health request"),
        )
        .await
        .expect("health response");

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("health body");
        let body: Value = serde_json::from_slice(&body).expect("health JSON");
        assert_eq!(body["status"], "DOWN");
        for dependency in ["catalog", "pricing", "checkout", "postgres"] {
            assert_eq!(
                body["dependencies"][dependency], "DOWN",
                "{dependency} status"
            );
        }
    }

    #[test]
    fn core_readiness_does_not_gate_on_clickhouse() {
        let (_, Json(body)) = DependencyReadiness {
            catalog: true,
            pricing: true,
            checkout: true,
            postgres: true,
        }
        .into_response();

        assert_eq!(body["status"], "UP");
        assert!(body["dependencies"].get("clickhouse").is_none());
        assert!(body["dependencies"].get("orders").is_none());
    }

    #[tokio::test]
    async fn analytics_readiness_reports_clickhouse_separately() {
        let context = StoreContext {
            clickhouse_url: "http://127.0.0.1:1".to_owned(),
            ..StoreContext::default()
        };
        let response = app_with_cors(
            Arc::new(Schema::new(Query, Mutation, Subscription)),
            context,
            CorsConfig::from_origins(["http://web.test"]),
        )
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/analytics/readyz")
                .body(Body::empty())
                .expect("analytics readiness request"),
        )
        .await
        .expect("analytics readiness response");

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("analytics readiness body");
        let body: Value = serde_json::from_slice(&body).expect("analytics readiness JSON");
        assert_eq!(body["status"], "DOWN");
        assert_eq!(body["dependencies"]["clickhouse"], "DOWN");
    }

    #[test]
    fn maps_catalog_product_without_fabricating_price_or_identity() {
        let product = Product::from_catalog(CatalogProduct {
            id: "product-real".to_owned(),
            tenant_id: TEST_TENANT.to_owned(),
            slug: "real-product".to_owned(),
            sku: "SKU-REAL".to_owned(),
            name: "Real product".to_owned(),
            description: "Loaded from Catalog".to_owned(),
            brand: Some("Tailrocks".to_owned()),
            category: CatalogCategory {
                id: "category-1".to_owned(),
                tenant_id: TEST_TENANT.to_owned(),
                slug: "hardware".to_owned(),
                name: "Hardware".to_owned(),
            },
            price_minor: Some(4275),
            price: Some(CatalogPriceSnapshot {
                id: "price-1".to_owned(),
                currency: "USD".to_owned(),
                amount_minor: 4275,
                compare_at_minor: None,
                valid_from: "2026-09-04T00:00:00Z".to_owned(),
            }),
            variants: Vec::new(),
            reviews: Vec::new(),
            reviews_slow: Vec::new(),
            risk_score: None,
        });
        assert_eq!(product.sku, "SKU-REAL");
        assert_eq!(product.price_minor, Some(4275));
        assert_eq!(product.name, "Real product");
    }

    #[test]
    fn maps_current_itemized_pricing_contract() {
        let quote = Quote::try_from_proto(QuoteResponse {
            quote_id: "quote-1".to_owned(),
            status: QuoteStatus::Ready as i32,
            lines: vec![ProtoQuoteLine {
                sku: "SKU-REAL".to_owned(),
                quantity: 2,
                unit_price: Some(ProtoMoney {
                    currency_code: "USD".to_owned(),
                    amount_minor: 4275,
                }),
                line_total: Some(ProtoMoney {
                    currency_code: "USD".to_owned(),
                    amount_minor: 8550,
                }),
            }],
            subtotal: Some(ProtoMoney {
                currency_code: "USD".to_owned(),
                amount_minor: 8550,
            }),
            discount_total: Some(ProtoMoney {
                currency_code: "USD".to_owned(),
                amount_minor: 0,
            }),
            tax_total: Some(ProtoMoney {
                currency_code: "USD".to_owned(),
                amount_minor: 0,
            }),
            grand_total: Some(ProtoMoney {
                currency_code: "USD".to_owned(),
                amount_minor: 8550,
            }),
            valid_for_seconds: 45,
            pricing_version: "seed-2026-01".to_owned(),
        })
        .expect("current pricing response maps");
        assert_eq!(quote.status, "QUOTE_STATUS_READY");
        assert_eq!(
            quote.lines[0].line_total.as_ref().unwrap().amount_minor,
            8550
        );
        assert_eq!(quote.grand_total.as_ref().unwrap().amount_minor, 8550);
    }

    #[tokio::test]
    async fn catalog_queries_use_embedded_operation_names() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind catalog fixture");
        let address = listener.local_addr().expect("catalog fixture address");
        let operation_names = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let requests = Arc::new(tokio::sync::Mutex::new(Vec::<Value>::new()));
        let operation_names_for_fixture = Arc::clone(&operation_names);
        let requests_for_fixture = Arc::clone(&requests);
        let fixture = Router::new().route(
            "/",
            post(move |Json(body): Json<Value>| {
                let operation_names = Arc::clone(&operation_names_for_fixture);
                let requests = Arc::clone(&requests_for_fixture);
                async move {
                    let operation_name = body
                        .get("operationName")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    operation_names.lock().await.push(operation_name.clone());
                    requests.lock().await.push(body);
                    let data = match operation_name.as_str() {
                        "StorefrontProduct" => json!({"product": null}),
                        "StorefrontProducts" => json!({
                            "products": {
                                "items": [],
                                "page": 0,
                                "size": 20,
                                "totalElements": 0,
                                "totalPages": 0,
                                "hasNext": false,
                                "experience": "standard"
                            }
                        }),
                        "StorefrontCategories" => json!({"categories": []}),
                        _ => json!({}),
                    };
                    Json(json!({"data": data}))
                }
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, fixture)
                .await
                .expect("serve catalog fixture");
        });
        let context = StoreContext {
            catalog_url: format!("http://{address}/"),
            ..StoreContext::default()
        };
        assert!(
            context
                .catalog_product(TEST_TENANT, "WIDGET-1", "standard")
                .await
                .expect("catalog product response")
                .is_none()
        );
        assert!(
            context
                .catalog_products(CatalogProductsRequest {
                    tenant_id: TEST_TENANT,
                    search: Some("widget"),
                    category: Some("kitchen"),
                    sort: Some(ProductSort::PriceAsc),
                    page: 0,
                    size: 20,
                    segment: "standard",
                })
                .await
                .expect("catalog products response")
                .items
                .is_empty()
        );
        assert!(
            context
                .catalog_categories(TEST_TENANT)
                .await
                .expect("catalog categories response")
                .is_empty()
        );
        assert_eq!(
            *operation_names.lock().await,
            vec![
                "StorefrontProduct",
                "StorefrontProducts",
                "StorefrontCategories"
            ]
        );
        let requests = requests.lock().await;
        let products_request = requests
            .iter()
            .find(|request| request["operationName"] == "StorefrontProducts")
            .expect("products request");
        assert_eq!(products_request["variables"]["search"], "widget");
        assert_eq!(products_request["variables"]["category"], "kitchen");
        assert_eq!(products_request["variables"]["sort"], "PRICE_ASC");
        server.abort();
    }

    #[tokio::test]
    async fn cart_graphql_backend_operations_use_durable_checkout_and_w3c_context() {
        global::set_text_map_propagator(HeaderPropagator);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind checkout cart fixture");
        let address = listener
            .local_addr()
            .expect("checkout cart fixture address");
        let seen = Arc::new(tokio::sync::Mutex::new(Vec::<(
            String,
            String,
            Value,
            HeaderMap,
        )>::new()));
        let seen_get = Arc::clone(&seen);
        let seen_post = Arc::clone(&seen);
        let fixture = Router::new()
            .route(
                "/api/cart",
                get(move |headers: HeaderMap, uri: Uri| {
                    let seen = Arc::clone(&seen_get);
                    async move {
                        seen.lock().await.push((
                            "GET".to_owned(),
                            uri.to_string(),
                            Value::Null,
                            headers,
                        ));
                        Json(json!({
                            "cart": {
                                "id": "cart-nova",
                                "status": "active",
                                "currency": "USD",
                                "items": [{
                                    "sku": "WIDGET-1",
                                    "product_name": "Widget",
                                    "quantity": 2,
                                    "unit_price_minor": 1999,
                                    "line_total_minor": 3998
                                }]
                            },
                            "tenant_id": "tenant-nova"
                        }))
                    }
                }),
            )
            .route(
                "/api/cart/items",
                post(
                    move |headers: HeaderMap, uri: Uri, Json(body): Json<Value>| {
                        let seen = Arc::clone(&seen_post);
                        async move {
                            seen.lock().await.push((
                                "POST".to_owned(),
                                uri.to_string(),
                                body,
                                headers,
                            ));
                            Json(json!({
                                "cart_id": "cart-nova",
                                "sku": "WIDGET-1",
                                "quantity_added": 1,
                                "unit_price_minor": 1999
                            }))
                        }
                    },
                ),
            );
        let server = tokio::spawn(async move {
            axum::serve(listener, fixture)
                .await
                .expect("serve checkout cart fixture");
        });

        let trace_id =
            TraceId::from_hex("4bf92f3577b34da6a3ce929d0e0e4736").expect("valid trace id");
        let trace_state =
            TraceState::from_key_value([("playground", "commerce")]).expect("valid trace state");
        let inbound = OtelContext::new()
            .with_remote_span_context(SpanContext::new(
                trace_id,
                SpanId::from_hex("00f067aa0ba902b7").expect("valid span id"),
                TraceFlags::SAMPLED,
                true,
                trace_state,
            ))
            .with_baggage([
                KeyValue::new(semconv::TENANT_ID, "tenant-nova"),
                KeyValue::new(semconv::USER_TIER, "premium"),
            ]);
        let context = StoreContext {
            checkout_endpoint: format!("http://{address}"),
            ..StoreContext::default()
        }
        .with_request_context(&inbound);

        let cart = context
            .cart("tenant-nova", "customer-nova", None)
            .await
            .expect("durable cart query");
        let cart = cart.expect("active cart");
        assert_eq!(cart.id, "cart-nova");
        assert_eq!(cart.items[0].quantity, 2);

        let added = context
            .add_cart_item(&AddCartItemInput {
                sku: "WIDGET-1".to_owned(),
                quantity: 1,
                tenant_id: Some("tenant-nova".to_owned()),
                customer_id: Some("customer-nova".to_owned()),
                cart_id: Some("cart-nova".to_owned()),
                session_id: Some("session-nova".to_owned()),
                currency_code: Some("USD".to_owned()),
            })
            .await
            .expect("durable cart item add");
        assert_eq!(added.cart_id, "cart-nova");
        assert_eq!(added.quantity_added, 1);

        let seen = seen.lock().await;
        assert_eq!(seen.len(), 2);
        assert!(seen[0].1.contains("tenant_id=tenant-nova"));
        assert!(seen[0].1.contains("customer_id=customer-nova"));
        assert_eq!(seen[1].2["tenant_id"], "tenant-nova");
        assert_eq!(seen[1].2["customer_id"], "customer-nova");
        assert_eq!(seen[1].2["cart_id"], "cart-nova");
        for (_, _, _, headers) in seen.iter() {
            assert_eq!(
                headers["traceparent"],
                "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"
            );
            assert_eq!(headers["tracestate"], "playground=commerce");
            assert!(
                headers["baggage"]
                    .to_str()
                    .expect("baggage header")
                    .contains("tenant.id=tenant-nova")
            );
        }
        server.abort();
    }

    #[tokio::test]
    async fn browser_preflight_allows_json_and_trace_headers() {
        let response = app_with_cors(
            Arc::new(Schema::new(Query, Mutation, Subscription)),
            StoreContext::default(),
            CorsConfig::from_origins(["http://web.test"]),
        )
        .oneshot(
            Request::builder()
                .method("OPTIONS")
                .uri("/api/checkout")
                .header("origin", "http://web.test")
                .header("access-control-request-method", "POST")
                .header(
                    "access-control-request-headers",
                    "content-type, traceparent, tracestate, baggage",
                )
                .body(Body::empty())
                .expect("preflight request"),
        )
        .await
        .expect("preflight response");

        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            response
                .headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .and_then(|value| value.to_str().ok()),
            Some("http://web.test")
        );
        assert_eq!(
            response
                .headers()
                .get(header::ACCESS_CONTROL_ALLOW_METHODS)
                .and_then(|value| value.to_str().ok()),
            Some(CORS_ALLOWED_METHODS)
        );
        assert_eq!(
            response
                .headers()
                .get(header::ACCESS_CONTROL_ALLOW_HEADERS)
                .and_then(|value| value.to_str().ok()),
            Some(CORS_ALLOWED_HEADERS)
        );

        let response = app_with_cors(
            Arc::new(Schema::new(Query, Mutation, Subscription)),
            StoreContext::default(),
            CorsConfig::from_origins(["http://web.test"]),
        )
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/graphiql")
                .header("origin", "http://web.test")
                .body(Body::empty())
                .expect("actual cross-origin request"),
        )
        .await
        .expect("actual cross-origin response");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .and_then(|value| value.to_str().ok()),
            Some("http://web.test")
        );
    }

    #[tokio::test]
    async fn cors_rejects_unconfigured_origins() {
        let response = app_with_cors(
            Arc::new(Schema::new(Query, Mutation, Subscription)),
            StoreContext::default(),
            CorsConfig::from_origins(["http://web.test"]),
        )
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/graphiql")
                .header("origin", "http://untrusted.test")
                .body(Body::empty())
                .expect("cross-origin request"),
        )
        .await
        .expect("cross-origin response");

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(
            response
                .headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .is_none()
        );
    }

    #[tokio::test]
    async fn request_timeout_returns_bounded_error() {
        let response = timed_response(Duration::from_millis(1), async {
            tokio::time::sleep(Duration::from_millis(25)).await;
            StatusCode::OK.into_response()
        })
        .await;

        assert_eq!(response.status(), StatusCode::REQUEST_TIMEOUT);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("timeout body");
        let body: Value = serde_json::from_slice(&body).expect("timeout JSON");
        assert_eq!(body["error"], "request_timeout");
    }

    #[test]
    fn downstream_errors_use_allowlisted_client_messages() {
        let error = anyhow!(DownstreamHttpError {
            status: reqwest::StatusCode::BAD_REQUEST,
            body: r#"{
                "error":"payment_invalid_request",
                "message":"card=4111111111111111; secret=do-not-expose"
            }"#
            .to_owned(),
        });

        let (status, Json(body)) = api_error(error);

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "payment_invalid_request");
        assert_eq!(body["message"], "the payment request is invalid");
        assert!(!body.to_string().contains("do-not-expose"));
        assert!(!body.to_string().contains("4111111111111111"));
    }

    #[test]
    fn downstream_errors_redact_diagnostics_and_preserve_gateway_mapping() {
        let body = r#"{
            "error":"unknown_internal_code;secret",
            "message":"sql=SELECT * FROM payments; token=tok_secret",
            "details":{"customer_id":"customer-private"}
        }"#;
        let error = anyhow!(DownstreamHttpError {
            status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            body: body.to_owned(),
        });

        let (status, Json(response_body)) = api_error(error);

        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert_eq!(response_body["error"], "downstream_error");
        assert_eq!(response_body["message"], "downstream request failed");
        let diagnostics = redacted_downstream_body(body);
        assert!(diagnostics.contains("[REDACTED]"));
        assert!(!diagnostics.contains("SELECT * FROM payments"));
        assert!(!diagnostics.contains("tok_secret"));
        assert!(!diagnostics.contains("customer-private"));
    }

    #[tokio::test]
    async fn synthetic_order_dispatch_surface_is_removed() {
        let response = app_with_cors(
            Arc::new(Schema::new(Query, Mutation, Subscription)),
            StoreContext::default(),
            CorsConfig::from_origins(["http://web.test"]),
        )
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/orders")
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .expect("synthetic order dispatch request"),
        )
        .await
        .expect("synthetic order dispatch response");

        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    #[tokio::test]
    async fn orders_route_forwards_tenant_and_customer_filters() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind checkout fixture");
        let address = listener.local_addr().expect("checkout fixture address");
        let fixture = Router::new().route(
            "/api/orders",
            get(
                |QueryParams(query): QueryParams<HashMap<String, String>>| async move {
                    Json(json!({
                        "orders": [{
                            "id": "order-1",
                            "order_number": "NOVA-1",
                            "tenant_id": query.get("tenant_id"),
                            "customer_id": query.get("customer_id"),
                            "status": "pending",
                            "currency": "USD"
                        }]
                    }))
                },
            ),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, fixture)
                .await
                .expect("serve checkout fixture");
        });
        let context = StoreContext {
            checkout_endpoint: format!("http://{address}"),
            ..StoreContext::default()
        };
        let response = app_with_cors(
            Arc::new(Schema::new(Query, Mutation, Subscription)),
            context,
            CorsConfig::from_origins(["http://web.test"]),
        )
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/orders?tenant_id=tenant-nova&customer_id=customer-nova-ava")
                .header("origin", "http://web.test")
                .body(Body::empty())
                .expect("orders request"),
        )
        .await
        .expect("orders response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("orders body");
        let body: Value = serde_json::from_slice(&body).expect("orders JSON");
        assert_eq!(body["tenant_id"], "tenant-nova");
        assert_eq!(body["customer_id"], "customer-nova-ava");
        assert_eq!(body["orders"][0]["tenant_id"], "tenant-nova");
        assert_eq!(body["orders"][0]["customer_id"], "customer-nova-ava");
        server.abort();
    }

    #[test]
    fn maps_checkout_order_detail_projection_without_dropping_totals() {
        let order = Order::try_from_value(&json!({
            "id": "order-1",
            "order_number": "ACME-1",
            "tenant_id": "tenant-acme",
            "customer_id": "customer-1",
            "status": "processing",
            "currency": "USD",
            "subtotal_minor": 1999,
            "discount_minor": 100,
            "tax_minor": 190,
            "shipping_minor": 500,
            "total_minor": 2589,
            "created_at": 1788494350_i64,
            "items": [{
                "sku": "WIDGET-1",
                "product_name": "Everyday Widget",
                "quantity": 1,
                "unit_price_minor": 1999,
                "discount_minor": 100,
                "line_total_minor": 1899
            }]
        }))
        .expect("checkout order detail maps");

        assert_eq!(order.order_number, "ACME-1");
        assert_eq!(order.subtotal_minor.as_deref(), Some("1999"));
        assert_eq!(order.discount_minor.as_deref(), Some("100"));
        assert_eq!(order.tax_minor.as_deref(), Some("190"));
        assert_eq!(order.shipping_minor.as_deref(), Some("500"));
        assert_eq!(order.items[0].line_total_minor.as_deref(), Some("1899"));
        assert_eq!(order.created_at.as_deref(), Some("1788494350"));
    }

    #[test]
    fn decodes_clickhouse_analytics_rows_in_storage_shape() {
        let event: AnalyticsEvent = serde_json::from_value(json!({
            "event_id": "event-1",
            "tenant_id": "tenant-acme",
            "event_key": "order-1:paid",
            "customer_id": "customer-1",
            "event_name": "order.paid",
            "event_version": 1,
            "source": "fulfillment",
            "entity_type": "order",
            "entity_id": "order-1",
            "occurred_at": "2026-09-04 03:59:10.298",
            "trace_id": "trace-1",
            "span_id": "span-1",
            "traceparent": "00-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-bbbbbbbbbbbbbbbb-01",
            "tracestate": "",
            "baggage": "tenant.id=tenant-acme",
            "feature_variant": "orchestrated",
            "properties": "{}",
            "context": "{}"
        }))
        .expect("ClickHouse analytics row maps");

        assert_eq!(event.event_name, "order.paid");
        assert_eq!(event.trace_id, "trace-1");
        assert_eq!(event.span_id, "span-1");
    }

    #[test]
    fn decodes_committed_price_change_notification_payload() {
        let event: PriceChangeNotification = serde_json::from_str(
            r#"{
                "tenantId":"tenant-acme",
                "sku":"WIDGET-1",
                "productId":"prod-acme-widget",
                "variantId":"var-acme-widget-1",
                "priceId":"price-committed",
                "currency":"USD",
                "amountMinor":2199,
                "compareAtMinor":2499,
                "validFrom":"2026-09-04T00:00:00Z",
                "observedAt":"2026-09-04T00:00:00Z"
            }"#,
        )
        .expect("price change notification payload maps");

        assert_eq!(event.tenant_id, "tenant-acme");
        assert_eq!(event.sku, "WIDGET-1");
        assert_eq!(event.amount_minor, 2199);
        assert_eq!(event.observed_at, "2026-09-04T00:00:00Z");
    }

    #[tokio::test]
    async fn graphql_schema_exposes_real_commerce_boundaries() {
        let schema = Arc::new(Schema::new(Query, Mutation, Subscription));
        let response = app(schema, StoreContext::default())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/graphql")
                    .header("content-type", "application/json")
                .body(Body::from(
                        r#"{"query":"{ __schema { queryType { fields { name args { name type { kind name ofType { kind name } } } } } mutationType { fields { name } } subscriptionType { fields { name } } types { name kind enumValues { name } } } }"} "#, 
                    ))
                    .expect("introspection request"),
            )
            .await
            .expect("introspection response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("introspection body");
        let body = std::str::from_utf8(&body).expect("introspection utf8");
        for field in ["category", "sort", "ProductSort", "PRICE_ASC"] {
            assert!(
                body.contains(field),
                "missing products contract field {field}"
            );
        }
        assert!(!body.contains("publishOrder"));
        for field in [
            "products",
            "product",
            "categories",
            "quote",
            "cart",
            "checkout",
            "addCartItem",
            "recordAnalytics",
            "analytics",
            "analyticsEvents",
            "analyticsSummary",
            "priceChanges",
        ] {
            assert!(body.contains(field), "missing GraphQL field {field}");
        }
    }

    #[test]
    fn cors_allows_the_browser_w3c_and_sentry_trace_headers() {
        let headers = HeaderValue::from_static(
            "content-type, traceparent, tracestate, baggage, sentry-trace",
        );
        assert!(request_headers_are_allowed(&headers));
        assert!(!request_headers_are_allowed(&HeaderValue::from_static(
            "content-type, authorization",
        )));
    }

    #[test]
    fn cors_matches_configured_compose_origins_exactly() {
        let config = CorsConfig::from_origins(["http://localhost:5173", "http://127.0.0.1:15173"]);
        assert!(
            config
                .allowed_origin(&HeaderValue::from_static("http://127.0.0.1:15173"))
                .is_some()
        );
        assert!(
            config
                .allowed_origin(&HeaderValue::from_static("http://127.0.0.1:15174"))
                .is_none()
        );
    }
}
