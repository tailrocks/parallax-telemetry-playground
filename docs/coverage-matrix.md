# Coverage matrix

Current source-to-journey map. A row is complete only when the path is backed
by the real dependency named in the row and its failure behavior is observable.

| Area | Source path | Scenario / gate | Required evidence |
|---|---|---|---|
| Browser browse | TanStack web → storefront → catalog GraphQL | web build; `a24` | Seeded product/category/variant/price/review data; W3C browser-to-service context. |
| Browser cart/checkout | web → checkout POST | web tests; `a1` | Cart and pending/paid order state in PostgreSQL; typed error response and RUM event. |
| Pricing | checkout/storefront → Pricing tonic | Rust tests; `a23`, `a7b` | Versioned itemized quote, promotions, currency validation, stream message events, failure/cancel status. |
| Payment | checkout → Java Payment gRPC | Java tests; `a1`, `b-chaos` | Separate lifecycle RPCs, tenant-scoped persistence, idempotency, decline/provider/pending distinction, compensation. |
| Catalog | storefront/checkout/recommendation → Java GraphQL | Java tests; `a6`, `a24` | PostgreSQL reads, Redis cache-aside, OpenFeature ordering, batched reviews, deliberate partial field error. |
| Inventory | checkout → Rust inventory | Rust tests; `a25`, `b2` | PostgreSQL transaction, tenant/SKU/location joins, row locks, bounded pool behavior, reserve/release. |
| Recommendation | checkout/web → Rust recommendation → catalog GraphQL | Rust tests; `a26`, `b13` | Catalog-backed results, propagated context, bounded slow/stampede/leak chaos only. |
| Synchronous storage | services → PostgreSQL | clean boot; `a1`, `a25` | Shared init schema, foreign keys, indexes, generated inventory availability, no lazy replacement schema. |
| Cache | catalog/pricing → Redis | clean boot; `a24`, `a23` | Cache keys include tenant/query dimensions; misses read PostgreSQL; invalidation follows price updates. |
| Async checkout event | checkout transactional outbox → `commerce.events` RabbitMQ → Java fulfillment | `a1`, `a3` | Checkout payload has tenant/customer, seeded SKU, USD, `payment_method_token`, and `payment_method_type`; fulfillment verification uses authenticated tenant-scoped polling; durable publish/confirm, W3C headers, consumer link, shipment, and notification record. |
| Fulfillment replay | authenticated seeded-order replay → `commerce.events` → Java fulfillment → notifications | `a4`, `a8` | `FULFILLMENT_INTERNAL_TOKEN` plus `Authorization` and `X-Tenant-Id`; seeded Acme/Nova order identity; duplicate-safe shipment and notification state. This is an operational replay seam, not checkout-outbox evidence. |
| Synthetic messaging fixtures | orders `/order` → private `orders.synthetic` exchange → orders consumer | `a20`, `b-async-chaos`, `b21`, `a29` | Explicit tenant/customer query, HTTP failures stop the scenario, and lag/poison/orphan behavior is labeled synthetic; it does not claim fulfillment or checkout-outbox coverage. |
| Analytics | checkout/fulfillment → ClickHouse; storefront read | `a1`, `a4`, browser Analytics | JSONEachRow ingestion and application query of `analytics.analytics_events`; tenant/entity/trace fields. |
| Feature variants | flagd/OpenFeature in catalog/checkout/pricing/recommendation | `a14` | String variant, request context, span attributes, durable exposure, behavior difference. |
| Subscription | catalog updatePrice → PostgreSQL commit/NOTIFY → GraphQL WS | `a7` | Committed change wakes listener; no timer polling or in-memory event bus. |
| Propagation | all HTTP/gRPC/RabbitMQ edges | `a1`, `a3`, `a4`, `a10` | `traceparent`, `tracestate`, safe `baggage`, business attributes, producer/consumer links; async consumers receive authenticated tenant context at the HTTP verification seam. |
| Typed telemetry | Rust/Java/TypeScript semantic conventions | `a29`; language gates | Common event names and resource identity; no hand-copied wire names. |
| Failure paths | explicit token/delay/fail inputs | `b-chaos`, `b2`, `b-checkout-chaos` | Normal product data remains valid; failure code/status is typed and traceable. |
| Browser RUM | TanStack routes, fetch instrumentation, Sentry RUM | web tests; `a5`, `a28` when harness exists | Route/action spans, backend trace context, handled exception and web vitals. |
| Compose | all service/dependency definitions | compose config; clean boot | Postgres, Redis, RabbitMQ, ClickHouse, flagd, all Rust/Java/web services healthy/gated. |

## Static completion gates

```bash
rtk cargo fmt --all -- --check
rtk cargo check --workspace --all-targets --locked
rtk cargo test --workspace --all-targets --locked
(cd services/catalog && rtk proxy ./gradlew --no-daemon clean test)
(cd services/payment && rtk proxy ./gradlew --no-daemon clean test)
(cd services/fulfillment && rtk proxy ./gradlew --no-daemon clean test)
(cd web && rtk bun run build && rtk bun run test)
rtk proxy docker compose -f deploy/docker-compose.yml config --quiet
rtk git diff --check
```

Unverified runtime rows remain a work item; they are not labeled as passed by
source inspection alone.

## Fixture contract

Normal checkout fixtures use the seeded `tenant-acme` / `customer-acme-ava`
identity and a seeded SKU. Every direct checkout request supplies
`payment_method_token` and `payment_method_type: "card"`; failure scenarios
change only the explicit provider token or bounded delay control.

The real async checkout proof is `a1` or `a3`: submit checkout, let the
transactional outbox publish to `commerce.events`, then poll fulfillment with
`Authorization: Bearer "$FULFILLMENT_INTERNAL_TOKEN"` and
`X-Tenant-Id`. The `orders /order` endpoint publishes an intentionally
incompatible message on the private `orders.synthetic` exchange. It remains
useful for messaging lag, poison, orphan, and fan-in fixtures, but must not be
described as checkout-outbox or fulfillment evidence.
