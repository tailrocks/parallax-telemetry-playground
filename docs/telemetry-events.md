# Telemetry Events

Plan 056 defines this typed business-event taxonomy. Event names are
low-cardinality; variable values stay in attributes.

| event.name | Emitted by | Attributes |
|---|---|---|
| `checkout.completed` | checkout (Rust) | `sku`, `quantity`, `order.total` |
| `checkout.failed` | checkout (Rust) | `sku`, `error.type` |
| `order.consumed` | orders (Rust) | `order_id`, `poison` |
| `catalog.products.served` | catalog (Java) | `product.count`, `catalog.promo` |
| `payment.authorized` | payment (Java) | `payment.method` |
| `web.checkout.submitted` | web (TypeScript) | `sku`, `quantity` |

Rust emits these with the OpenTelemetry logs API `EventName` field through
`playground_telemetry::emit_event`, alongside existing `tracing` logs.

Java emits structured SLF4J application logs with MDC/key-value attributes and
also emits typed OpenTelemetry log records with `setEventName`. Compose enables
the logback appender key-value and MDC capture flags so app log rows carry
`event.name` plus business fields. The local OpenTelemetry API jar exposes
`LogRecordBuilder.setEventName`.

Web emits `web.checkout.submitted` through `@opentelemetry/sdk-logs@0.220.0`
and `@opentelemetry/exporter-logs-otlp-proto@0.220.0`, matching the existing
OTLP/protobuf trace exporter line. Browser logs remain experimental upstream,
but the package versions are compatible with the current OTel JS 0.220/2.8
dependency set.

Before Parallax plan 055 lands, the local SQL fallback is `body` +
`log_attributes` in `opentelemetry_logs`; plan 055 promotes the OTel event name
into a first-class `event_name` column/UI field.
