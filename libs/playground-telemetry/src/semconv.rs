//! Shared telemetry wire-name registry for playground producers.

pub const SERVICE_NAME: &str = "service.name";
pub const SERVICE_VERSION: &str = "service.version";
pub const SERVICE_NAMESPACE: &str = "service.namespace";
pub const SERVICE_INSTANCE_ID: &str = "service.instance.id";
pub const DEPLOYMENT_ENVIRONMENT_NAME: &str = "deployment.environment.name";
pub const PLAYGROUND_NAMESPACE: &str = "playground";
pub const DEFAULT_ENVIRONMENT: &str = "playground";

pub const EVENT_NAME: &str = "event.name";
pub const SESSION_ID: &str = "session.id";
pub const APP_SCREEN_NAME: &str = "app.screen.name";
pub const ERROR_TYPE: &str = "error.type";

pub const OTEL_KIND: &str = "otel.kind";
pub const SPAN_KIND_CLIENT: &str = "client";
pub const SPAN_KIND_SERVER: &str = "server";
pub const SPAN_KIND_INTERNAL: &str = "internal";
pub const SPAN_KIND_PRODUCER: &str = "producer";
pub const SPAN_KIND_CONSUMER: &str = "consumer";

pub const PARALLAX_RUN_ID: &str = "parallax.run.id";
pub const PARALLAX_SESSION_ID: &str = "parallax.session.id";
pub const PARALLAX_EXECUTION_LAYER: &str = "parallax.execution.layer";
pub const PARALLAX_AGENT_ID: &str = "parallax.agent.id";

pub const GEN_AI_OPERATION_NAME: &str = "gen_ai.operation.name";
pub const TOOL_NAME: &str = "tool.name";
pub const SHELL_COMMAND: &str = "shell.command";

pub const USER_TIER: &str = "user.tier";
pub const CANARY_EMAIL: &str = "canary.email";
pub const CANARY_TOKEN: &str = "canary.token";
pub const CANARY_CARD: &str = "canary.card";
pub const CANARY_JWT: &str = "canary.jwt";

pub const WEB_CHECKOUT_SUBMITTED: &str = "web.checkout.submitted";
pub const CATALOG_PRODUCTS_SERVED: &str = "catalog.products.served";
pub const PAYMENT_AUTHORIZED: &str = "payment.authorized";

pub const TOKIO_RUNTIME_METRIC_NAMES: &[&str] = &[
    "tokio.runtime.workers_count",
    "tokio.runtime.alive_tasks",
    "tokio.runtime.global_queue_depth",
    "tokio.runtime.blocking_pool_depth",
    "tokio.runtime.total_park_count",
    "tokio.runtime.total_busy_duration_ms",
];
