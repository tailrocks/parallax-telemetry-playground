# Parallax Telemetry Playground

Polyglot commerce workload for exercising OpenTelemetry traces, logs, metrics,
Sentry envelopes, and browser RUM through one real business flow. Research
software; breaking changes are expected.

## Architecture

```bash
TanStack web
    │ HTTP / GraphQL
    ▼
Rust storefront ── GraphQL ──► Java catalog ── JDBC ──► PostgreSQL
    │             └─ Pricing gRPC ──► Rust pricing ── Postgres + Redis
    │             └─ checkout HTTP ──► Rust checkout
    ▼
Rust checkout ── Payment gRPC ──► Java payment ── JDBC ──► PostgreSQL
       ├─ Inventory HTTP ──► Rust inventory ── PostgreSQL row locks
       ├─ Recommendation HTTP ──► Rust recommendation ── Catalog GraphQL
       └─ transactional outbox ──► RabbitMQ ──► Java fulfillment
                                             ├─ PostgreSQL shipment/idempotency state
                                             ├─ ClickHouse analytics
                                             └─ Rust notifications

flagd/OpenFeature controls bounded explicit variants and chaos cases.
Rust CLI and k6 loadgen are additional real clients.
```

Normal product data, prices, inventory, customers, orders, payments, and
shipments are seeded by the versioned PostgreSQL migrations in
[`deploy/postgres/migrations/`](deploy/postgres/migrations/), starting with
  [`001-commerce.sql`](deploy/postgres/migrations/001-commerce.sql), plus
  additive durable checkout and compensation migrations.
The `mise run infra:postgres_migrate` task applies each migration once,
verifies the required schema, and records it in
`public.schema_migrations` before database clients start.
ClickHouse tables are initialized by [`deploy/clickhouse/init.sql`](deploy/clickhouse/init.sql).
Redis is a catalog/pricing cache, not a source of truth. RabbitMQ is durable;
publisher confirms, manual acknowledgements, retries, dead-letter queues,
W3C `traceparent`/`tracestate`/`baggage`, and consumer idempotency are part of
the application path.

## Run

Install the pinned toolchain, start the telemetry receiver, then boot the stack:

```bash
mise install
parallax serve
mise run demo:fresh -- --yes
```

`demo:fresh` destroys only this playground's Compose volumes before rebuilding.
Use `mise run demo:stack` when existing local data should remain.

Compose gates services that depend on the PostgreSQL migration job. The
`mise run verify:commerce_stack` task checks Compose configuration and core
HTTP health only; a plain `up` is not itself an analytics-readiness proof.
Use `mise run verify:commerce_trace` for collected service, RabbitMQ, and
analytics evidence.

Useful surfaces:

- Web: <http://localhost:5173>
- Storefront GraphQL/GraphiQL: <http://localhost:8094/graphiql>
- Catalog GraphQL: <http://localhost:8080/graphiql>
- Checkout: <http://localhost:8088>
- Inventory: <http://localhost:8089>
- Recommendation: <http://localhost:8090>
- Orders synthetic RabbitMQ publisher: <http://localhost:8092/order>
- Fulfillment authenticated seeded-order replay: <http://localhost:8093/publish>
- RabbitMQ management: <http://localhost:15672>
- ClickHouse HTTP: <http://localhost:8123>

`/order` is an isolated synthetic orders-service publisher on the private
`orders.synthetic` exchange. It does not prove checkout outbox delivery. The
real async proof submits `/checkout`, then verifies the returned order through
fulfillment with `Authorization: Bearer $FULFILLMENT_INTERNAL_TOKEN` and
`X-Tenant-Id`.

Run a real checkout:

```bash
curl --fail-with-body -X POST http://localhost:8088/checkout \
  -H 'content-type: application/json' \
  --data '{"tenant_id":"tenant-acme","customer_id":"customer-acme-ava","items":[{"sku":"WIDGET-1","quantity":1}],"currency_code":"USD","payment_method_token":"tok_visa","payment_method_type":"card","request_id":"readme-1"}'
```

For clean-volume bootstrap and migration idempotence, use the Rust-owned
disposable Compose proof:

```bash
mise run verify:postgres_idempotence
```

## Verification

Rust gates:

```bash
mise run quality:fmt
mise run quality:ci
mise run quality:test
mise run quality:lint
mise run check:scenarios
mise run check:typescript
```

Java observable gate:

```text
parallax invocation start -- mise run test:observable -- java
```

The Rust observable runner invokes Gradle's `GradleWrapperMain` directly for
Catalog, Payment, and Fulfillment; service-local POSIX wrappers are not needed.

Web observable gate:

```bash
parallax invocation start -- mise run test:observable -- web
```

Then run rtk git diff --check and
`mise run verify:commerce_stack` for Compose configuration and core service
health.

For the current verification contract, see [`docs/VERIFICATION.md`](docs/VERIFICATION.md).

For an automated Parallax topology assertion over one real checkout:

```bash
mise run verify:commerce_trace
```

It checks the collected service, RabbitMQ, and analytics evidence through
`playground commerce-verify`. Inspect trace parent/link fields separately when
causal topology is required; service and queue co-presence is not proof of a
causal span edge.

## Journeys and scenarios

```bash
mise run commerce:checkout_saga       # checkout saga
mise run graphql:batching_errors       # GraphQL batch/N+1/partial error
mise run database:price_subscription   # PostgreSQL price subscription
mise run grpc:pricing_stream           # pricing stream/failure/cancel
mise run grpc:storefront_pricing       # storefront → pricing gRPC
mise run graphql:storefront_catalog    # storefront → catalog GraphQL
mise run postgres:query_pressure       # PostgreSQL slow/N+1/pool pressure; auto-creates a fence
mise run cache:recommendation_stampede # catalog-backed recommendation
mise run feature_flags:checkout_variants # live checkoutFlow variant flip
mise run messaging:checkout_outbox     # checkout → outbox → fulfillment
mise run messaging:seeded_order_replay # seeded-order replay → RabbitMQ → Rust
mise run failures:payment_latency      # provider decline and delay
mise run failures:inventory            # inventory failure
mise run jvm:memory_pressure            # JVM GC/memory-pressure workload
mise run container:recommendation_oom_probe -- --yes # explicit destructive OOM probe
mise run product:ui_agent_verify       # browser smoke, Parallax CLI required
```

Tasks use stable `group:semantic_name` names; numeric IDs are internal fixture
references, not public aliases. Run
`mise tasks ls --sort name` to see every grouped task and description. Run
`mise run demo:full` for the ordered capability tour. The corner-case corpus
is documented in [`docs/corner-case-matrix.md`](docs/corner-case-matrix.md).
With the stack running, use the load scenario for sustained k6 traffic:

```text
mise run load:checkout
```

## Contracts

- [`proto/pricing.proto`](proto/pricing.proto): versioned itemized quote and
  server stream.
- [`proto/payment.proto`](proto/payment.proto): separate authorize, capture,
  void, refund, and status lifecycle.
- Catalog GraphQL schema: [`services/catalog/src/main/resources/graphql/schema.graphqls`](services/catalog/src/main/resources/graphql/schema.graphqls).
- Shared propagation and semantic conventions: `libs/playground-telemetry`.
- Storefront GraphQL also exposes the durable Checkout-backed `cart` query and
  `addCartItem` mutation; browser cart changes remain explicit session state and
  checkout writes the authoritative cart/order transaction.
- Browser routes: `/`, `/catalog`, `/products/:sku`, `/cart`, `/checkout`,
  `/orders`, `/orders/:orderId`, and `/analytics`.

The docs describe the current source tree. No historical live-run counts or
retired broker/API claims are acceptance evidence.
