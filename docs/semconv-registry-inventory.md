# Semantic-convention migration inventory

Date: 2026-07-14

This is the frozen pre-generation inventory for the shared Parallax semantic
convention registry. It preserves every existing wire-name value as a migration
constraint; it does not rename telemetry.

## Current emitters

| Language | Current module | Consumers | Migration state |
| --- | --- | --- | --- |
| Rust | `libs/playground-telemetry/src/semconv.rs` | CLI and every Rust service through `playground_telemetry::semconv` | Shared source today |
| TypeScript | `web/src/semconv.ts` | Browser telemetry and web routes | Web-only source today |
| Java | `services/catalog/.../Semconv.java`, `services/payment/.../Semconv.java` | Catalog and payment events | Duplicated local sources |

## Frozen shared values

| Group | Names and exact values |
| --- | --- |
| Resource | `SERVICE_NAME=service.name`, `SERVICE_VERSION=service.version`, `SERVICE_NAMESPACE=service.namespace`, `SERVICE_INSTANCE_ID=service.instance.id`, `DEPLOYMENT_ENVIRONMENT_NAME=deployment.environment.name`, `PLAYGROUND_NAMESPACE=playground`, `DEFAULT_ENVIRONMENT=playground` |
| Event and error | `EVENT_NAME=event.name`, `SESSION_ID=session.id`, `APP_SCREEN_NAME=app.screen.name`, `ERROR_TYPE=error.type` |
| Span kind | `OTEL_KIND=otel.kind`, `SPAN_KIND_CLIENT=client`, `SPAN_KIND_SERVER=server`, `SPAN_KIND_INTERNAL=internal`, `SPAN_KIND_PRODUCER=producer`, `SPAN_KIND_CONSUMER=consumer` |
| Neutral CLI contract | `CLI_INVOCATION_ID=cli.invocation.id`, `session.id`, `app.mode`, `outcome` (the retired `parallax.*` vendor keys were removed 2026-07-17) |
| Agent | `GEN_AI_OPERATION_NAME=gen_ai.operation.name`, `TOOL_NAME=tool.name`, `SHELL_COMMAND=shell.command` |
| Test/redaction | `USER_TIER=user.tier`, `CANARY_EMAIL=canary.email`, `CANARY_TOKEN=canary.token`, `CANARY_CARD=canary.card`, `CANARY_JWT=canary.jwt` |
| Named events | `WEB_CHECKOUT_SUBMITTED=web.checkout.submitted`, `CATALOG_PRODUCTS_SERVED=catalog.products.served`, `PAYMENT_AUTHORIZED=payment.authorized`, `CATALOG_PRODUCT_QUERIES=catalog.product.queries` |
| Browser-only today | `APP_WIDGET_NAME=app.widget.name`, `TELEMETRY_PROPAGATION_DISABLED=telemetry.propagation.disabled`, `UI_CLICK=ui.click`, `UI_SUBMIT=ui.submit`, `BROWSER_WEB_VITAL=browser.web_vital`, `WEB_VITAL_NAME=web_vital.name`, `WEB_VITAL_VALUE=web_vital.value`, `WEB_VITAL_RATING=web_vital.rating`, `WEB_VITAL_ID=web_vital.id`, `WEB_VITAL_DELTA=web_vital.delta`, `WEB_VITAL_NAVIGATION_TYPE=web_vital.navigation_type` |
| Tokio metrics | `TOKIO_RUNTIME_WORKERS_COUNT=tokio.runtime.workers_count`, `TOKIO_RUNTIME_ALIVE_TASKS=tokio.runtime.alive_tasks`, `TOKIO_RUNTIME_GLOBAL_QUEUE_DEPTH=tokio.runtime.global_queue_depth`, `TOKIO_RUNTIME_BLOCKING_POOL_DEPTH=tokio.runtime.blocking_pool_depth`, `TOKIO_RUNTIME_TOTAL_PARK_COUNT=tokio.runtime.total_park_count`, `TOKIO_RUNTIME_TOTAL_BUSY_DURATION_MS=tokio.runtime.total_busy_duration_ms` |

## Required generated targets

The registry must generate the Rust shared module, the web TypeScript module,
and one Java source shared by catalog and payment. The two Java package-local
`Semconv` classes are removed only after generated output compiles with the same
values. Normal Rust, Java, and Bun builds consume checked-in generated files;
they never invoke Weaver or download registry inputs.
