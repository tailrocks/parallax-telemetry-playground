//! Generated semantic-convention names shared by Parallax producers and consumers.
//!
//! Source: `telemetry/semconv/contract.yaml`. Do not edit by hand;
//! run `cargo xtask semconv generate`. Product builds depend only on this
//! dependency-free crate, never on the generator or Weaver.

pub const SERVICE_NAME: &str = "service.name";
pub const SERVICE_NAMESPACE: &str = "service.namespace";
pub const SERVICE_INSTANCE_ID: &str = "service.instance.id";
pub const SERVICE_VERSION: &str = "service.version";
pub const VCS_REF_HEAD_REVISION: &str = "vcs.ref.head.revision";
pub const DEPLOYMENT_ENVIRONMENT_NAME: &str = "deployment.environment.name";
pub const EVENT_NAME: &str = "event.name";
pub const ERROR_TYPE: &str = "error.type";
pub const CLI_INVOCATION_ID: &str = "cli.invocation.id";
pub const CLI_COMMAND_NAME: &str = "cli.command.name";
pub const APP_MODE: &str = "app.mode";
pub const APP_MODE_ONE_SHOT: &str = "one_shot";
pub const APP_MODE_INTERACTIVE: &str = "interactive";
pub const APP_MODE_DAEMON: &str = "daemon";
pub const APP_MODE_CAPSULE: &str = "capsule";
pub const SESSION_PREVIOUS_ID: &str = "session.previous_id";
pub const SESSION_START_EVENT_NAME: &str = "session.start";
pub const SESSION_END_EVENT_NAME: &str = "session.end";
pub const UI_SCREEN_ENTERED_EVENT_NAME: &str = "ui.screen.entered";
pub const UI_SCREEN_EXITED_EVENT_NAME: &str = "ui.screen.exited";
pub const UI_WIDGET_FOCUSED_EVENT_NAME: &str = "ui.widget.focused";
pub const UI_WIDGET_UNFOCUSED_EVENT_NAME: &str = "ui.widget.unfocused";
pub const APP_SCREEN_ID: &str = "app.screen.id";
pub const UI_ACTION_NAME: &str = "ui.action.name";
pub const UI_SCREEN_VISIT_ID: &str = "ui.screen.visit.id";
pub const UI_NAVIGATION_SEQUENCE: &str = "ui.navigation.sequence";
pub const UI_TRANSITION_REASON: &str = "ui.transition.reason";
pub const BACKGROUND_CYCLE_SPAN_NAME: &str = "background.cycle";
pub const BACKGROUND_CYCLE_NAME: &str = "background.cycle.name";
pub const JOB_ID: &str = "job.id";
pub const JOB_TYPE: &str = "job.type";
pub const OUTCOME: &str = "outcome";
pub const OUTCOME_SUCCESS: &str = "success";
pub const OUTCOME_FAILURE: &str = "failure";
pub const OUTCOME_ERROR: &str = "error";
pub const OUTCOME_TIMEOUT: &str = "timeout";
pub const OUTCOME_SKIP: &str = "skip";
pub const OUTCOME_CANCELLATION: &str = "cancellation";
pub const GEN_AI_AGENT_NAME: &str = "gen_ai.agent.name";
pub const GEN_AI_CONVERSATION_ID: &str = "gen_ai.conversation.id";
pub const GEN_AI_PROVIDER_NAME: &str = "gen_ai.provider.name";
pub const GEN_AI_USAGE_INPUT_TOKENS: &str = "gen_ai.usage.input_tokens";
pub const GEN_AI_USAGE_OUTPUT_TOKENS: &str = "gen_ai.usage.output_tokens";
pub const PROCESS_EXIT_CODE: &str = "process.exit.code";
pub const CLI_COMMAND_SPAN_NAME: &str = "cli.command";
pub const APP_STARTUP_SPAN_NAME: &str = "app.startup";
pub const APP_SHUTDOWN_SPAN_NAME: &str = "app.shutdown";
pub const UI_ACTION_SPAN_NAME: &str = "ui.action";
pub const HTTP_REQUEST_METHOD: &str = "http.request.method";
pub const HTTP_ROUTE: &str = "http.route";
pub const HTTP_RESPONSE_STATUS_CODE: &str = "http.response.status_code";
pub const URL_PATH: &str = "url.path";
pub const HTTP_SERVER_REQUEST_DURATION: &str = "http.server.request.duration";
pub const SERVER_ADDRESS: &str = "server.address";
pub const MESSAGING_SYSTEM: &str = "messaging.system";
pub const MESSAGING_DESTINATION_NAME: &str = "messaging.destination.name";
pub const MESSAGING_OPERATION_NAME: &str = "messaging.operation.name";
pub const MESSAGING_OPERATION_TYPE: &str = "messaging.operation.type";
pub const MESSAGING_MESSAGE_ID: &str = "messaging.message.id";
pub const PLAYGROUND_NAMESPACE: &str = "playground";
pub const DEFAULT_ENVIRONMENT: &str = "playground";
pub const SESSION_ID: &str = "session.id";
pub const JOB_TYPE_ORDER_DISPATCH: &str = "order_dispatch";
pub const JOB_TYPE_FULFILLMENT_SHIPMENT: &str = "fulfillment_shipment";
pub const BACKGROUND_CYCLE_QUEUE_HEALTH: &str = "queue_health";
pub const BACKGROUND_CYCLE_PRICE_REFRESH: &str = "price_refresh";
pub const APP_SCREEN_HOME: &str = "home";
pub const APP_SCREEN_CART: &str = "cart";
pub const APP_SCREEN_CHECKOUT: &str = "checkout";
pub const UI_ACTION_CART_ADD: &str = "cart.add";
pub const UI_ACTION_CHECKOUT_SUBMIT: &str = "checkout.submit";
pub const UI_ACTION_SCREEN_BACK: &str = "screen.back";
pub const GEN_AI_AGENT_NAMES: &[&str] = &["claude", "codex", "amp"];
pub const GEN_AI_PROVIDER_NAMES: &[&str] = &["anthropic", "openai", "sourcegraph"];
pub const APP_SCREEN_NAME: &str = "app.screen.name";
pub const APP_WIDGET_NAME: &str = "app.widget.name";
pub const TELEMETRY_PROPAGATION_DISABLED: &str = "telemetry.propagation.disabled";
pub const OTEL_KIND: &str = "otel.kind";
pub const SPAN_KIND_CLIENT: &str = "client";
pub const SPAN_KIND_SERVER: &str = "server";
pub const SPAN_KIND_INTERNAL: &str = "internal";
pub const SPAN_KIND_PRODUCER: &str = "producer";
pub const SPAN_KIND_CONSUMER: &str = "consumer";
pub const GEN_AI_OPERATION_NAME: &str = "gen_ai.operation.name";
pub const TOOL_NAME: &str = "tool.name";
pub const SHELL_COMMAND: &str = "shell.command";
pub const USER_TIER: &str = "user.tier";
pub const TENANT_ID: &str = "tenant.id";
pub const GRAPHQL_OPERATION_TYPE: &str = "graphql.operation.type";
pub const GRAPHQL_OPERATION_NAME: &str = "graphql.operation.name";
pub const GRAPHQL_DOCUMENT: &str = "graphql.document";
pub const GRAPHQL_FIELD_NAME: &str = "graphql.field.name";
pub const GRAPHQL_FIELD_PATH: &str = "graphql.field.path";
pub const TEST_CASE_NAME: &str = "test.case.name";
pub const TEST_CASE_RESULT_STATUS: &str = "test.case.result.status";
pub const TEST_RESULT_STATUS_PASS: &str = "pass";
pub const TEST_RESULT_STATUS_FAIL: &str = "fail";
pub const TEST_CASE_PARAMETERS: &str = "test.case.parameters";
pub const TEST_CASE_FAILURE_KIND: &str = "test.case.failure.kind";
pub const TEST_FAILURE_KIND_ASSERTION: &str = "assertion_failure";
pub const TEST_FAILURE_KIND_HARNESS: &str = "harness_error";
pub const TEST_FAILURE_EVENT_NAME: &str = "test.failure";
pub const TEST_FAILURE_MESSAGE: &str = "test.failure.message";
pub const TEST_FAILURE_STACKTRACE: &str = "test.failure.stacktrace";
pub const TEST_ATTEMPT_ORDINAL: &str = "test.attempt.ordinal";
pub const TEST_ATTEMPT_ID: &str = "test.attempt.id";
pub const TEST_ATTEMPT_TOTAL: &str = "test.attempt.total";
pub const TEST_CODE_REFERENCE: &str = "test.code_reference";
pub const TEST_CONFIGURATION_OS: &str = "test.configuration.os";
pub const TEST_CONFIGURATION_ENVIRONMENT: &str = "test.configuration.environment";
pub const TEST_CONFIGURATION_BROWSER: &str = "test.configuration.browser";
pub const TEST_ARTIFACT_PATH: &str = "test.artifact.path";
pub const TEST_SUITE_NAME: &str = "test.suite.name";
pub const TEST_SUITE_RUN_STATUS: &str = "test.suite.run.status";
pub const CICD_PIPELINE_RUN_ID: &str = "cicd.pipeline.run.id";
pub const CICD_PIPELINE_TASK_TYPE: &str = "cicd.pipeline.task.type";
pub const PARALLAX_TEST_ID: &str = "parallax.test.id";
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

#[must_use]
pub fn span_column(attr: &str) -> String {
    format!("span_attributes.{attr}")
}
