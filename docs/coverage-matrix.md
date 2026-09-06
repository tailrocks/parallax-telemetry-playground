# Coverage matrix

Current source-to-journey map. A row is complete only when the path is backed
by the real dependency named in the row and its failure behavior is observable.

| Area | Source path | Scenario / gate | Required evidence |
|---|---|---|---|
| Browser browse | TanStack web → storefront → catalog GraphQL | web build; `graphql:storefront_catalog` | Seeded product/category/variant/price/review data; W3C browser-to-service context. |
| Browser cart/checkout | web → checkout POST | web tests; `commerce:checkout_saga` | Cart and pending/paid order state in PostgreSQL; typed error response and RUM event. |
| Pricing | checkout/storefront → Pricing tonic | Rust tests; `grpc:storefront_pricing`, `grpc:pricing_stream` | Versioned itemized quote, promotions, currency validation, stream message events, failure/cancel status. |
| Payment | checkout → Java Payment gRPC | Java tests; `commerce:checkout_saga`, `failures:payment_latency` | Separate lifecycle RPCs, tenant-scoped persistence, idempotency, decline/provider/pending distinction, compensation. |
| Catalog | storefront/checkout/recommendation → Java GraphQL | Java tests; `graphql:batching_errors`, `graphql:storefront_catalog` | PostgreSQL reads, Redis cache-aside, OpenFeature ordering, batched reviews, deliberate partial field error. |
| Inventory | checkout → Rust inventory | Rust tests; `postgres:query_pressure`, `failures:inventory` | PostgreSQL transaction, tenant/SKU/location joins, row locks, bounded pool behavior, reserve/release. |
| Recommendation | checkout/web → Rust recommendation → catalog GraphQL | Rust tests; `cache:recommendation_stampede`, `recommendation:slow_query` | Catalog-backed results, propagated context, bounded slow/stampede/leak chaos only. |
| Synchronous storage | services → PostgreSQL | clean boot; `commerce:checkout_saga`, `postgres:query_pressure` | Shared init schema, foreign keys, indexes, generated inventory availability, no lazy replacement schema. |
| Cache | catalog/pricing → Redis | clean boot; `graphql:storefront_catalog`, `grpc:storefront_pricing` | Cache keys include tenant/query dimensions; misses read PostgreSQL; invalidation follows price updates. |
| Async checkout event | checkout transactional outbox → `commerce.events` RabbitMQ → Java fulfillment | `commerce:checkout_saga`, `messaging:checkout_outbox` | Checkout payload has tenant/customer, seeded SKU, USD, `payment_method_token`, and `payment_method_type`; fulfillment verification uses authenticated tenant-scoped polling; durable publish/confirm, W3C headers, consumer link, shipment, and notification record. |
| Fulfillment replay | authenticated seeded-order replay → `commerce.events` → Java fulfillment → notifications | `messaging:seeded_order_replay`, `messaging:java_fulfillment_replay` | `FULFILLMENT_INTERNAL_TOKEN` plus `Authorization` and `X-Tenant-Id`; seeded Acme/Nova order identity; duplicate-safe shipment and notification state. This is an operational replay seam, not checkout-outbox evidence. |
| Synthetic messaging fixtures | orders `/order` → private `orders.synthetic` exchange → orders consumer | `messaging:batch_fanin`, `messaging:poison_retry`, `messaging:orphan_consumer`, `events:typed_business_events` | Explicit tenant/customer query, HTTP failures stop the scenario, and lag/poison/orphan behavior is labeled synthetic; it does not claim fulfillment or checkout-outbox coverage. |
| Analytics | checkout/fulfillment → ClickHouse; storefront read | `commerce:checkout_saga`, `messaging:seeded_order_replay`, browser Analytics | JSONEachRow ingestion and application query of `analytics.analytics_events`; tenant/entity/trace fields. |
| Feature variants | flagd/OpenFeature in catalog/checkout/pricing/recommendation | `feature_flags:checkout_variants` | String variant, request context, span attributes, durable exposure, behavior difference. |
| Subscription | catalog updatePrice → PostgreSQL commit/NOTIFY → GraphQL WS | `database:price_subscription` | Committed change wakes listener; no timer polling or in-memory event bus. |
| Propagation | all HTTP/gRPC/RabbitMQ edges | `commerce:checkout_saga`, `messaging:checkout_outbox`, `messaging:seeded_order_replay`, `propagation:baggage` | `traceparent`, `tracestate`, safe `baggage`, business attributes, producer/consumer links; async consumers receive authenticated tenant context at the HTTP verification seam. |
| Typed telemetry | Rust/Java/TypeScript semantic conventions | `events:typed_business_events`; language gates | Common event names and resource identity; no hand-copied wire names. |
| Failure paths | explicit token/delay/fail inputs | `failures:payment_latency`, `failures:inventory`, `failures:checkout_chaos` | Normal product data remains valid; failure code/status is typed and traceable. |
| Browser RUM | TanStack routes, fetch instrumentation, Sentry RUM | web tests; `browser:rum_error`, `browser:rum_journey` when harness exists | Route/action spans, backend trace context, handled exception and web vitals. |
| Compose | all service/dependency definitions | `verify:commerce_stack` | Compose configuration plus HTTP health for Parallax, Checkout, Catalog, Inventory, and Recommendation; dependency and full-service runtime evidence comes from the scenario and trace tasks. |

## Static completion gates

```bash
mise run quality:fmt
mise run quality:ci
mise run quality:test
mise run quality:lint
mise run check:scenarios
mise run check:typescript
parallax invocation start -- mise run test:observable -- java
parallax invocation start -- mise run test:observable -- web
mise run verify:commerce_stack  # Compose config + core HTTP health
rtk git diff --check
```

Unverified runtime rows remain a work item; they are not labeled as passed by
source inspection alone.

## Fixture contract

Normal checkout fixtures use the seeded `tenant-acme` / `customer-acme-ava`
identity and a seeded SKU. Every direct checkout request supplies
`payment_method_token` and `payment_method_type: "card"`; failure scenarios
change only the explicit provider token or bounded delay control.

The real async checkout proof is `commerce:checkout_saga` or
`messaging:checkout_outbox`: submit checkout, let the
transactional outbox publish to `commerce.events`, then poll fulfillment with
`Authorization: Bearer "$FULFILLMENT_INTERNAL_TOKEN"` and
`X-Tenant-Id`. The `orders /order` endpoint publishes an intentionally
incompatible message on the private `orders.synthetic` exchange. It remains
useful for messaging lag, poison, orphan, and fan-in fixtures, but must not be
described as checkout-outbox or fulfillment evidence.
