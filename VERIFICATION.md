# Verification contract

This document is an executable-source checklist, not a historical pass report.
Run it against the current checkout and a clean dependency stack.

## Static gates

```bash
mise run fmt
mise run check
mise run nextest
mise run lint
mise run ci
mise run check:scenarios
mise run check:typescript
mise run verify:postgres_idempotence
mise run verify:commerce_stack
rtk git diff --check
```

```bash
parallax invocation start -- mise run test:observable -- java
parallax invocation start -- mise run test:observable -- web
```

The Rust code uses `tokio-postgres`/`deadpool-postgres`; Java uses JDBC. The
tests may use stubs at unit boundaries, but deployed journeys require the
shared PostgreSQL, Redis, RabbitMQ, ClickHouse, flagd, and service containers.

## Compose configuration, service, and dependency check

Run the Rust-backed Compose, service-readiness, and dependency check from the
repository root:

```bash
mise run verify:commerce_stack
```

The task validates `deploy/docker-compose.yml` with `docker compose config
--quiet`. If `VERIFY_MANAGE_STACK=1`, it also starts the Compose `demo` profile
with `--build -d`; otherwise it checks the already-running stack. It requires
all 16 Compose services to be running and healthy, and the `flagd-health-tools`,
`postgres-migrate`, and `clickhouse-init` jobs to have exited with code 0. It
then checks HTTP readiness for Parallax, Checkout, Catalog, Inventory,
Recommendation, Orders, Fulfillment, Storefront, Storefront analytics, and
Web, plus in-container PostgreSQL, Redis, RabbitMQ, ClickHouse, flagd, Pricing
gRPC, Payment HTTP/gRPC, and Notifications probes.

This task does not create a unique disposable Compose project, prove migration
idempotence, execute business journeys, inspect business storage state, verify
Parallax causal topology, run browser E2E, inject readiness failures, or clean
up Compose resources. Use the focused scenario tasks, `cd web && bun run
e2e:compose`, and `mise run verify:commerce_trace` for those separate proofs.

For an already-running stack, leave `VERIFY_MANAGE_STACK=0` (the default) and
override health endpoints only when needed:

```bash
VERIFY_MANAGE_STACK=0 \
  PARALLAX_API_URL=http://127.0.0.1:4000/health \
  CHECKOUT_URL=http://127.0.0.1:8088/healthz \
  CATALOG_URL=http://127.0.0.1:8080/healthz \
  INVENTORY_URL=http://127.0.0.1:8089/healthz \
  RECOMMENDATION_URL=http://127.0.0.1:8090/healthz \
  mise run verify:commerce_stack
```

The optional `VERIFY_HTTP_TIMEOUT_SECONDS` controls the health-request timeout
and defaults to 20 seconds. The task does not tear down a managed stack.

`deploy/postgres/migrations/*.sql` owns all commerce tables, constraints,
indexes, and seed data. `mise run infra:postgres_migrate` creates
`public.schema_migrations`, applies pending versions under a transaction and
advisory lock, and records a version only after its SQL succeeds.
`deploy/clickhouse/init.sql` owns analytics tables. Services do not create lazy
replacement schemas at startup.

## Runtime journeys

The stack task above validates readiness and dependency surfaces. It does not
execute the journeys below; run each scenario task against a healthy dependency
stack:

| Journey | Proof |
|---|---|
| `graphql:storefront_catalog` | Web/storefront product page delegates to Catalog GraphQL and returns seeded products, variants, prices, categories, and reviews. |
| `commerce:checkout_saga` | Checkout crosses Catalog, Pricing gRPC, PostgreSQL order/cart, Payment gRPC, Inventory, Recommendation, analytics, and the outbox. |
| `messaging:seeded_order_replay` | Fulfillment publishes seeded orders to RabbitMQ, consumes with a span link, persists shipment state, writes ClickHouse, and calls Notifications. |
| `database:price_subscription` | Catalog price change is committed in PostgreSQL's durable journal; LISTEN wakes the replayable GraphQL subscription. |
| `grpc:pricing_stream` | Pricing server stream emits per-message telemetry and distinct failure/cancellation paths. |
| `grpc:storefront_pricing` | Storefront GraphQL resolver calls the real Pricing gRPC contract. |
| `postgres:query_pressure` | Inventory uses real PostgreSQL row locks, slow query, bounded query fan-out, and pool limits. |
| `cache:recommendation_stampede` | Recommendation reads Catalog GraphQL; parallelism is explicit bounded chaos, not a normal fake cache. |
| `feature_flags:checkout_variants` | With a configured flagd variant, checkout behavior and business context are observable; flag configuration changes and restart behavior are outside the stack health task. |
| `failures:payment_latency` / `failures:inventory` | Provider decline and inventory failure are explicit typed failure paths using normal SKUs. |
| `alerts:error_rate_breach` / `alerts:recovery` | Decline traffic then healthy traffic demonstrate error-rate recovery. |
| `events:typed_business_events` | Shared typed business event names appear across Rust, Java, and web telemetry. |
| `product:ui_agent_verify` | Browser smoke, when the Parallax CLI/browser harness is available. |

The real distributed topology has an executable Parallax assertion. With the
stack and Parallax server running, execute:

```bash
mise run verify:commerce_trace
```

The Rust verifier task sends a real checkout carrying all three W3C headers, then runs
`playground commerce-verify`. That verifier queries Parallax GraphQL, follows
linked RabbitMQ traces, and checks Catalog, Pricing, Inventory, Payment,
Fulfillment/RabbitMQ, Notifications, and analytics evidence. The dedicated
Parallax failure corpus and browser causal assertion remain pending; a passing
stack verifier or browser E2E does not close those gates.

## Storage assertions

After `mise run commerce:checkout_saga` and
`mise run messaging:seeded_order_replay`, verify directly through
SQL/ClickHouse/RabbitMQ tooling:

- PostgreSQL has a paid order, order items, payment lifecycle row, analytics
  event, outbox event, shipment/processing claim with lease, and notification
  delivery. Failed checkout compensation has durable reservation/task rows.
- Replaying the same RabbitMQ event does not create a second shipment or
  duplicate notification delivery.
- ClickHouse `analytics.analytics_events` contains the checkout/fulfillment
  event with tenant, entity, trace, span, properties, and context columns.
- RabbitMQ has durable commerce/fulfillment queues and dead-letter queues.
- Redis contains catalog/pricing cache entries whose values came from
  PostgreSQL-backed reads.

## Failure and security checks

- All payment lifecycle operations require tenant context and enforce tenant
  qualified database access.
- Payment decline, provider-unavailable, invalid-state, and pending outcomes
  remain distinguishable through gRPC and Checkout status responses.
- Inventory reservation compensation releases previously reserved lines when a
  later checkout step fails; captured payments are refunded by the recovery
  path and authorized payments are voided. If a direct recovery is ambiguous,
  PostgreSQL owns a bounded retryable compensation task; exhaustion leaves a
  terminal `failed` task for reconciliation instead of an infinite retry loop.
- Every async message carries W3C `traceparent`, `tracestate`, and safe
  business `baggage`; consumers create links to producer contexts.
- Tenant, customer, SKU, and event keys are bounded and validated at service
  boundaries.
- No normal response depends on in-memory product, price, order, queue, or
  analytics fixtures. Synthetic delay/failure/leak/stampede controls are
  bounded and explicit query/body/config inputs; normal Catalog reads do not
  fail by SKU.

## Browser contract

The TanStack app must build and expose real routes for browse, product detail,
cart, checkout, orders, and analytics. Storefront GraphQL must expose the
durable Checkout-backed cart query/mutation, and checkout must submit actual
JSON. The UI must render typed API failures, preserve trace context, and keep
RUM spans/events. No demo-only SKU or retired endpoint is valid acceptance
evidence.

The canonical real-Compose browser gate is:

```bash
cd web
bun run e2e:compose
```

It is separate from the Parallax topology and failure assertions.
