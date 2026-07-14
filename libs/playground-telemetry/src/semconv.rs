//! Generated semantic-convention names shared by Parallax producers and consumers.
//!
//! Source: `telemetry/semconv/contract.yaml`. Do not edit by hand;
//! run `cargo xtask semconv generate`. Product builds depend only on this
//! dependency-free crate, never on the generator or Weaver.

pub const SERVICE_NAME: &str = "service.name";
pub const SERVICE_NAMESPACE: &str = "service.namespace";
pub const SERVICE_INSTANCE_ID: &str = "service.instance.id";
pub const SERVICE_VERSION: &str = "service.version";
pub const DEPLOYMENT_ENVIRONMENT_NAME: &str = "deployment.environment.name";
pub const EVENT_NAME: &str = "event.name";
pub const ERROR_TYPE: &str = "error.type";
pub const PARALLAX_RUN_ID: &str = "parallax.run.id";
pub const HTTP_REQUEST_METHOD: &str = "http.request.method";
pub const HTTP_ROUTE: &str = "http.route";
pub const HTTP_RESPONSE_STATUS_CODE: &str = "http.response.status_code";
pub const URL_PATH: &str = "url.path";
pub const HTTP_SERVER_REQUEST_DURATION: &str = "http.server.request.duration";
pub const PLAYGROUND_NAMESPACE: &str = "playground";
pub const DEFAULT_ENVIRONMENT: &str = "playground";
pub const SESSION_ID: &str = "session.id";
pub const APP_SCREEN_NAME: &str = "app.screen.name";
pub const APP_WIDGET_NAME: &str = "app.widget.name";
pub const TELEMETRY_PROPAGATION_DISABLED: &str = "telemetry.propagation.disabled";
pub const OTEL_KIND: &str = "otel.kind";
pub const SPAN_KIND_CLIENT: &str = "client";
pub const SPAN_KIND_SERVER: &str = "server";
pub const SPAN_KIND_INTERNAL: &str = "internal";
pub const SPAN_KIND_PRODUCER: &str = "producer";
pub const SPAN_KIND_CONSUMER: &str = "consumer";
pub const PARALLAX_SESSION_ID: &str = "parallax.session.id";
pub const PARALLAX_EXECUTION_LAYER: &str = "parallax.execution.layer";
pub const PARALLAX_AGENT_ID: &str = "parallax.agent.id";
pub const GEN_AI_OPERATION_NAME: &str = "gen_ai.operation.name";
pub const TOOL_NAME: &str = "tool.name";
pub const SHELL_COMMAND: &str = "shell.command";
pub const USER_TIER: &str = "user.tier";
pub const TENANT_ID: &str = "tenant.id";
pub const CANARY_EMAIL: &str = "canary.email";
pub const CANARY_TOKEN: &str = "canary.token";
pub const CANARY_CARD: &str = "canary.card";
pub const CANARY_JWT: &str = "canary.jwt";
pub const WEB_CHECKOUT_SUBMITTED: &str = "web.checkout.submitted";
pub const CATALOG_PRODUCTS_SERVED: &str = "catalog.products.served";
pub const CATALOG_PRODUCT_QUERIES: &str = "catalog.product.queries";
pub const PAYMENT_AUTHORIZED: &str = "payment.authorized";
pub const UI_CLICK: &str = "ui.click";
pub const UI_SUBMIT: &str = "ui.submit";
pub const BROWSER_WEB_VITAL: &str = "browser.web_vital";
pub const WEB_VITAL_NAME: &str = "web_vital.name";
pub const WEB_VITAL_VALUE: &str = "web_vital.value";
pub const WEB_VITAL_RATING: &str = "web_vital.rating";
pub const WEB_VITAL_ID: &str = "web_vital.id";
pub const WEB_VITAL_DELTA: &str = "web_vital.delta";
pub const WEB_VITAL_NAVIGATION_TYPE: &str = "web_vital.navigation_type";
pub const TOKIO_RUNTIME_WORKERS_COUNT: &str = "tokio.runtime.workers_count";
pub const TOKIO_RUNTIME_ALIVE_TASKS: &str = "tokio.runtime.alive_tasks";
pub const TOKIO_RUNTIME_GLOBAL_QUEUE_DEPTH: &str = "tokio.runtime.global_queue_depth";
pub const TOKIO_RUNTIME_BLOCKING_POOL_DEPTH: &str = "tokio.runtime.blocking_pool_depth";
pub const TOKIO_RUNTIME_TOTAL_PARK_COUNT: &str = "tokio.runtime.total_park_count";
pub const TOKIO_RUNTIME_TOTAL_BUSY_DURATION_MS: &str = "tokio.runtime.total_busy_duration_ms";
pub const TOKIO_RUNTIME_METRIC_NAMES: &[&str] = &[
    "tokio.runtime.workers_count",
    "tokio.runtime.alive_tasks",
    "tokio.runtime.global_queue_depth",
    "tokio.runtime.blocking_pool_depth",
    "tokio.runtime.total_park_count",
    "tokio.runtime.total_busy_duration_ms",
];

#[must_use]
pub fn resource_json_path(attr: &str) -> String {
    format!(r#"$.\"{}\""#, attr.replace('"', "\\\""))
}

#[must_use]
pub fn resource_column(attr: &str) -> String {
    format!("resource_attributes.{attr}")
}
