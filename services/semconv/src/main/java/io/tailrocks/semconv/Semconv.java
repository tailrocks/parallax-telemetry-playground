// Generated from telemetry/semconv/contract.yaml.
// Run `cargo xtask semconv generate`; do not edit by hand.
package io.tailrocks.semconv;

public final class Semconv {
    private Semconv() {}

    public static final String SERVICE_NAME = "service.name";
    public static final String SERVICE_NAMESPACE = "service.namespace";
    public static final String SERVICE_INSTANCE_ID = "service.instance.id";
    public static final String SERVICE_VERSION = "service.version";
    public static final String DEPLOYMENT_ENVIRONMENT_NAME = "deployment.environment.name";
    public static final String EVENT_NAME = "event.name";
    public static final String ERROR_TYPE = "error.type";
    public static final String PARALLAX_RUN_ID = "parallax.run.id";
    public static final String HTTP_REQUEST_METHOD = "http.request.method";
    public static final String HTTP_ROUTE = "http.route";
    public static final String HTTP_RESPONSE_STATUS_CODE = "http.response.status_code";
    public static final String URL_PATH = "url.path";
    public static final String HTTP_SERVER_REQUEST_DURATION = "http.server.request.duration";
    public static final String PLAYGROUND_NAMESPACE = "playground";
    public static final String DEFAULT_ENVIRONMENT = "playground";
    public static final String SESSION_ID = "session.id";
    public static final String APP_SCREEN_NAME = "app.screen.name";
    public static final String APP_WIDGET_NAME = "app.widget.name";
    public static final String TELEMETRY_PROPAGATION_DISABLED = "telemetry.propagation.disabled";
    public static final String OTEL_KIND = "otel.kind";
    public static final String SPAN_KIND_CLIENT = "client";
    public static final String SPAN_KIND_SERVER = "server";
    public static final String SPAN_KIND_INTERNAL = "internal";
    public static final String SPAN_KIND_PRODUCER = "producer";
    public static final String SPAN_KIND_CONSUMER = "consumer";
    public static final String PARALLAX_SESSION_ID = "parallax.session.id";
    public static final String PARALLAX_EXECUTION_LAYER = "parallax.execution.layer";
    public static final String PARALLAX_AGENT_ID = "parallax.agent.id";
    public static final String GEN_AI_OPERATION_NAME = "gen_ai.operation.name";
    public static final String TOOL_NAME = "tool.name";
    public static final String SHELL_COMMAND = "shell.command";
    public static final String USER_TIER = "user.tier";
    public static final String TENANT_ID = "tenant.id";
    public static final String CANARY_EMAIL = "canary.email";
    public static final String CANARY_TOKEN = "canary.token";
    public static final String CANARY_CARD = "canary.card";
    public static final String CANARY_JWT = "canary.jwt";
    public static final String WEB_CHECKOUT_SUBMITTED = "web.checkout.submitted";
    public static final String CATALOG_PRODUCTS_SERVED = "catalog.products.served";
    public static final String CATALOG_PRODUCT_QUERIES = "catalog.product.queries";
    public static final String PAYMENT_AUTHORIZED = "payment.authorized";
    public static final String UI_CLICK = "ui.click";
    public static final String UI_SUBMIT = "ui.submit";
    public static final String BROWSER_WEB_VITAL = "browser.web_vital";
    public static final String WEB_VITAL_NAME = "web_vital.name";
    public static final String WEB_VITAL_VALUE = "web_vital.value";
    public static final String WEB_VITAL_RATING = "web_vital.rating";
    public static final String WEB_VITAL_ID = "web_vital.id";
    public static final String WEB_VITAL_DELTA = "web_vital.delta";
    public static final String WEB_VITAL_NAVIGATION_TYPE = "web_vital.navigation_type";
    public static final String TOKIO_RUNTIME_WORKERS_COUNT = "tokio.runtime.workers_count";
    public static final String TOKIO_RUNTIME_ALIVE_TASKS = "tokio.runtime.alive_tasks";
    public static final String TOKIO_RUNTIME_GLOBAL_QUEUE_DEPTH = "tokio.runtime.global_queue_depth";
    public static final String TOKIO_RUNTIME_BLOCKING_POOL_DEPTH = "tokio.runtime.blocking_pool_depth";
    public static final String TOKIO_RUNTIME_TOTAL_PARK_COUNT = "tokio.runtime.total_park_count";
    public static final String TOKIO_RUNTIME_TOTAL_BUSY_DURATION_MS = "tokio.runtime.total_busy_duration_ms";
    public static final String[] TOKIO_RUNTIME_METRIC_NAMES = {"tokio.runtime.workers_count", "tokio.runtime.alive_tasks", "tokio.runtime.global_queue_depth", "tokio.runtime.blocking_pool_depth", "tokio.runtime.total_park_count", "tokio.runtime.total_busy_duration_ms", };
}
