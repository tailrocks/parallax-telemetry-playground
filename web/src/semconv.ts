// Generated from telemetry/semconv/contract.yaml.
// Run `cargo xtask semconv generate`; do not edit by hand.

export const SERVICE_NAME = "service.name" as const
export const SERVICE_NAMESPACE = "service.namespace" as const
export const SERVICE_INSTANCE_ID = "service.instance.id" as const
export const SERVICE_VERSION = "service.version" as const
export const VCS_REF_HEAD_REVISION = "vcs.ref.head.revision" as const
export const DEPLOYMENT_ENVIRONMENT_NAME =
  "deployment.environment.name" as const
export const EVENT_NAME = "event.name" as const
export const ERROR_TYPE = "error.type" as const
export const CLI_INVOCATION_ID = "cli.invocation.id" as const
export const CLI_COMMAND_NAME = "cli.command.name" as const
export const APP_MODE = "app.mode" as const
export const APP_MODE_ONE_SHOT = "one_shot" as const
export const APP_MODE_INTERACTIVE = "interactive" as const
export const APP_MODE_DAEMON = "daemon" as const
export const APP_MODE_CAPSULE = "capsule" as const
export const SESSION_PREVIOUS_ID = "session.previous_id" as const
export const SESSION_START_EVENT_NAME = "session.start" as const
export const SESSION_END_EVENT_NAME = "session.end" as const
export const UI_SCREEN_ENTERED_EVENT_NAME = "ui.screen.entered" as const
export const UI_SCREEN_EXITED_EVENT_NAME = "ui.screen.exited" as const
export const UI_WIDGET_FOCUSED_EVENT_NAME = "ui.widget.focused" as const
export const UI_WIDGET_UNFOCUSED_EVENT_NAME = "ui.widget.unfocused" as const
export const APP_SCREEN_ID = "app.screen.id" as const
export const UI_ACTION_NAME = "ui.action.name" as const
export const UI_SCREEN_VISIT_ID = "ui.screen.visit.id" as const
export const UI_NAVIGATION_SEQUENCE = "ui.navigation.sequence" as const
export const UI_TRANSITION_REASON = "ui.transition.reason" as const
export const BACKGROUND_CYCLE_SPAN_NAME = "background.cycle" as const
export const BACKGROUND_CYCLE_NAME = "background.cycle.name" as const
export const JOB_ID = "job.id" as const
export const JOB_TYPE = "job.type" as const
export const OUTCOME = "outcome" as const
export const OUTCOME_SUCCESS = "success" as const
export const OUTCOME_FAILURE = "failure" as const
export const OUTCOME_ERROR = "error" as const
export const OUTCOME_TIMEOUT = "timeout" as const
export const OUTCOME_SKIP = "skip" as const
export const OUTCOME_CANCELLATION = "cancellation" as const
export const GEN_AI_AGENT_NAME = "gen_ai.agent.name" as const
export const GEN_AI_CONVERSATION_ID = "gen_ai.conversation.id" as const
export const GEN_AI_PROVIDER_NAME = "gen_ai.provider.name" as const
export const GEN_AI_USAGE_INPUT_TOKENS = "gen_ai.usage.input_tokens" as const
export const GEN_AI_USAGE_OUTPUT_TOKENS = "gen_ai.usage.output_tokens" as const
export const PROCESS_EXIT_CODE = "process.exit.code" as const
export const CLI_COMMAND_SPAN_NAME = "cli.command" as const
export const APP_STARTUP_SPAN_NAME = "app.startup" as const
export const APP_SHUTDOWN_SPAN_NAME = "app.shutdown" as const
export const UI_ACTION_SPAN_NAME = "ui.action" as const
export const HTTP_REQUEST_METHOD = "http.request.method" as const
export const HTTP_ROUTE = "http.route" as const
export const HTTP_RESPONSE_STATUS_CODE = "http.response.status_code" as const
export const URL_PATH = "url.path" as const
export const HTTP_SERVER_REQUEST_DURATION =
  "http.server.request.duration" as const
export const SERVER_ADDRESS = "server.address" as const
export const MESSAGING_SYSTEM = "messaging.system" as const
export const MESSAGING_DESTINATION_NAME = "messaging.destination.name" as const
export const MESSAGING_OPERATION_NAME = "messaging.operation.name" as const
export const MESSAGING_OPERATION_TYPE = "messaging.operation.type" as const
export const MESSAGING_MESSAGE_ID = "messaging.message.id" as const
export const PLAYGROUND_NAMESPACE = "playground" as const
export const DEFAULT_ENVIRONMENT = "playground" as const
export const SESSION_ID = "session.id" as const
export const JOB_TYPE_ORDER_DISPATCH = "order_dispatch" as const
export const JOB_TYPE_FULFILLMENT_SHIPMENT = "fulfillment_shipment" as const
export const BACKGROUND_CYCLE_QUEUE_HEALTH = "queue_health" as const
export const BACKGROUND_CYCLE_PRICE_REFRESH = "price_refresh" as const
export const APP_SCREEN_HOME = "home" as const
export const APP_SCREEN_CART = "cart" as const
export const APP_SCREEN_CHECKOUT = "checkout" as const
export const UI_ACTION_CART_ADD = "cart.add" as const
export const UI_ACTION_CHECKOUT_SUBMIT = "checkout.submit" as const
export const UI_ACTION_SCREEN_BACK = "screen.back" as const
export const GEN_AI_AGENT_NAMES = [
  "claude",
  "codex",
  "amp",
] as const
export const GEN_AI_PROVIDER_NAMES = [
  "anthropic",
  "openai",
  "sourcegraph",
] as const
export const APP_SCREEN_NAME = "app.screen.name" as const
export const APP_WIDGET_NAME = "app.widget.name" as const
export const TELEMETRY_PROPAGATION_DISABLED =
  "telemetry.propagation.disabled" as const
export const OTEL_KIND = "otel.kind" as const
export const SPAN_KIND_CLIENT = "client" as const
export const SPAN_KIND_SERVER = "server" as const
export const SPAN_KIND_INTERNAL = "internal" as const
export const SPAN_KIND_PRODUCER = "producer" as const
export const SPAN_KIND_CONSUMER = "consumer" as const
export const GEN_AI_OPERATION_NAME = "gen_ai.operation.name" as const
export const TOOL_NAME = "tool.name" as const
export const SHELL_COMMAND = "shell.command" as const
export const USER_TIER = "user.tier" as const
export const TENANT_ID = "tenant.id" as const
export const GRAPHQL_OPERATION_TYPE = "graphql.operation.type" as const
export const GRAPHQL_OPERATION_NAME = "graphql.operation.name" as const
export const GRAPHQL_DOCUMENT = "graphql.document" as const
export const GRAPHQL_FIELD_NAME = "graphql.field.name" as const
export const GRAPHQL_FIELD_PATH = "graphql.field.path" as const
export const TEST_CASE_NAME = "test.case.name" as const
export const TEST_CASE_RESULT_STATUS = "test.case.result.status" as const
export const TEST_RESULT_STATUS_PASS = "pass" as const
export const TEST_RESULT_STATUS_FAIL = "fail" as const
export const TEST_CASE_PARAMETERS = "test.case.parameters" as const
export const TEST_CASE_FAILURE_KIND = "test.case.failure.kind" as const
export const TEST_FAILURE_KIND_ASSERTION = "assertion_failure" as const
export const TEST_FAILURE_KIND_HARNESS = "harness_error" as const
export const TEST_FAILURE_EVENT_NAME = "test.failure" as const
export const TEST_FAILURE_MESSAGE = "test.failure.message" as const
export const TEST_FAILURE_STACKTRACE = "test.failure.stacktrace" as const
export const TEST_ATTEMPT_ORDINAL = "test.attempt.ordinal" as const
export const TEST_ATTEMPT_ID = "test.attempt.id" as const
export const TEST_ATTEMPT_TOTAL = "test.attempt.total" as const
export const TEST_CODE_REFERENCE = "test.code_reference" as const
export const TEST_CONFIGURATION_OS = "test.configuration.os" as const
export const TEST_CONFIGURATION_ENVIRONMENT =
  "test.configuration.environment" as const
export const TEST_CONFIGURATION_BROWSER = "test.configuration.browser" as const
export const TEST_ARTIFACT_PATH = "test.artifact.path" as const
export const TEST_SUITE_NAME = "test.suite.name" as const
export const TEST_SUITE_RUN_STATUS = "test.suite.run.status" as const
export const CICD_PIPELINE_RUN_ID = "cicd.pipeline.run.id" as const
export const CICD_PIPELINE_TASK_TYPE = "cicd.pipeline.task.type" as const
export const PARALLAX_TEST_ID = "parallax.test.id" as const
export const CANARY_EMAIL = "canary.email" as const
export const CANARY_TOKEN = "canary.token" as const
export const CANARY_CARD = "canary.card" as const
export const CANARY_JWT = "canary.jwt" as const
export const WEB_CHECKOUT_SUBMITTED = "web.checkout.submitted" as const
export const CATALOG_PRODUCTS_SERVED = "catalog.products.served" as const
export const CATALOG_PRODUCT_QUERIES = "catalog.product.queries" as const
export const PAYMENT_AUTHORIZED = "payment.authorized" as const
export const UI_CLICK = "ui.click" as const
export const UI_SUBMIT = "ui.submit" as const
export const BROWSER_WEB_VITAL = "browser.web_vital" as const
export const WEB_VITAL_NAME = "web_vital.name" as const
export const WEB_VITAL_VALUE = "web_vital.value" as const
export const WEB_VITAL_RATING = "web_vital.rating" as const
export const WEB_VITAL_ID = "web_vital.id" as const
export const WEB_VITAL_DELTA = "web_vital.delta" as const
export const WEB_VITAL_NAVIGATION_TYPE = "web_vital.navigation_type" as const
export const TOKIO_RUNTIME_WORKERS_COUNT =
  "tokio.runtime.workers_count" as const
export const TOKIO_RUNTIME_ALIVE_TASKS = "tokio.runtime.alive_tasks" as const
export const TOKIO_RUNTIME_GLOBAL_QUEUE_DEPTH =
  "tokio.runtime.global_queue_depth" as const
export const TOKIO_RUNTIME_BLOCKING_POOL_DEPTH =
  "tokio.runtime.blocking_pool_depth" as const
export const TOKIO_RUNTIME_TOTAL_PARK_COUNT =
  "tokio.runtime.total_park_count" as const
export const TOKIO_RUNTIME_TOTAL_BUSY_DURATION_MS =
  "tokio.runtime.total_busy_duration_ms" as const
export const TOKIO_RUNTIME_METRIC_NAMES = [
  "tokio.runtime.workers_count",
  "tokio.runtime.alive_tasks",
  "tokio.runtime.global_queue_depth",
  "tokio.runtime.blocking_pool_depth",
  "tokio.runtime.total_park_count",
  "tokio.runtime.total_busy_duration_ms",
] as const
