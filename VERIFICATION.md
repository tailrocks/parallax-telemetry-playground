# Verification contract

This document is an executable-source checklist, not a historical pass report.
Run it against the current checkout and a clean dependency stack.

## Static gates

```bash
rtk cargo fmt --all -- --check
rtk cargo check --workspace --all-targets --locked
rtk cargo test --workspace --all-targets --locked
rtk cargo clippy --workspace --all-targets --locked -- -D warnings
rtk bash scripts/check-scenarios.sh
rtk git diff --check
rtk proxy docker compose -f deploy/docker-compose.yml config --quiet
```

```bash
(cd services/catalog && rtk proxy ./gradlew --no-daemon clean test)
(cd services/payment && rtk proxy ./gradlew --no-daemon clean test)
(cd services/fulfillment && rtk proxy ./gradlew --no-daemon clean test)
(cd web && rtk bun run build)
(cd web && rtk bun run typecheck)
(cd web && rtk bun run test)
(cd web && rtk bun run e2e)
```

The Rust code uses `tokio-postgres`/`deadpool-postgres`; Java uses JDBC. The
tests may use stubs at unit boundaries, but deployed journeys require the
shared PostgreSQL, Redis, RabbitMQ, ClickHouse, flagd, and service containers.

## Canonical executable verifier

Run the clean disposable-stack gate from the repository root:

```bash
rtk bash scripts/verify-commerce-stack.sh
```

It creates a unique Compose project, builds the stack, applies PostgreSQL
migrations, reruns them unchanged, and removes only that project's containers,
volumes, and temporary files on exit. It then proves catalog cold/warm GraphQL,
Storefront pricing GraphQL, W3C-header checkout, async fulfillment and
notification delivery, direct Payment gRPC authorize/capture replay and
changed-fingerprint rejection, PostgreSQL/Redis/RabbitMQ/ClickHouse state, a
live checkout feature flip with behavior change without restart, the real
Compose-backed Playwright browser journey, and checkout readiness failure/
recovery.
Managed mode runs PostgreSQL and Redis assertions through the Compose service
containers; host `psql` and `redis-cli` are only required in external mode.

For an already-running external stack, skip lifecycle mutations and provide
its endpoints:

```bash
VERIFY_MANAGE_STACK=0 \
  VERIFY_EXTERNAL_ALLOW_MUTATION=1 \
  VERIFY_TENANT_ID=tenant-verification \
  VERIFY_CUSTOMER_ID=customer-verification \
  VERIFY_SKU=WIDGET-1 \
  CATALOG_GRAPHQL_URL=http://127.0.0.1:8080/graphql \
  STOREFRONT_GRAPHQL_URL=http://127.0.0.1:8094/graphql \
  CHECKOUT_URL=http://127.0.0.1:8088 \
  PAYMENT_GRPC_URL=http://127.0.0.1:9090 \
  FULFILLMENT_URL=http://127.0.0.1:8093 \
  rtk bash scripts/verify-commerce-stack.sh
```

External mode skips stack start/build, migration rerun, managed cold-cache
reset proof, browser E2E, feature-file mutation, and the stop/start readiness
fault injection. It still runs the business, async, storage, queue, cache,
and readiness-health assertions against the supplied stack; use an isolated
tenant/customer fixture and explicitly acknowledge its durable mutations with
`VERIFY_EXTERNAL_ALLOW_MUTATION=1`; external mode does not clean up data.

`deploy/postgres/migrations/*.sql` owns all commerce tables, constraints,
indexes, and seed data. `deploy/postgres/migrate.sh` creates
`public.schema_migrations`, applies pending versions under a transaction and
advisory lock, and records a version only after its SQL succeeds.
`deploy/clickhouse/init.sql` owns analytics tables. Services do not create lazy
replacement schemas at startup.

## Runtime journeys

The canonical verifier covers the focused distributed commerce slice. Use the
scenario drivers below for additional journeys:

| Journey | Proof |
|---|---|
| `a24` | Web/storefront product page delegates to Catalog GraphQL and returns seeded products, variants, prices, categories, and reviews. |
| `a1` | Checkout crosses Catalog, Pricing gRPC, PostgreSQL order/cart, Payment gRPC, Inventory, Recommendation, analytics, and the outbox. |
| `a4` | Fulfillment publishes seeded orders to RabbitMQ, consumes with a span link, persists shipment state, writes ClickHouse, and calls Notifications. |
| `a7` | Catalog price change is committed in PostgreSQL's durable journal; LISTEN wakes the replayable GraphQL subscription. |
| `a7b` | Pricing server stream emits per-message telemetry and distinct failure/cancellation paths. |
| `a23` | Storefront GraphQL resolver calls the real Pricing gRPC contract. |
| `a25` | Inventory uses real PostgreSQL row locks, slow query, bounded query fan-out, and pool limits. |
| `a26` | Recommendation reads Catalog GraphQL; parallelism is explicit bounded chaos, not a normal fake cache. |
| `a14` | flagd string variants change checkout behavior without restarting checkout and are recorded with business context. The managed verifier restarts only flagd after changing its bind-mounted file so the check is deterministic on Docker Desktop. |
| `b-chaos` / `b2` | Provider decline and inventory failure are explicit typed failure paths using normal SKUs. |
| `a-breach-error-rate` / `a-recover` | Decline traffic then healthy traffic demonstrate error-rate recovery. |
| `a29` | Shared typed business event names appear across Rust, Java, and web telemetry. |
| `c11` | Browser smoke, when the Parallax CLI/browser harness is available. |

The real distributed topology has an executable Parallax assertion. With the
stack and Parallax server running, execute:

```bash
parallax invocation start -- scripts/verify-commerce-trace.sh
```

The script sends a real checkout carrying all three W3C headers, then runs
`playground commerce-verify`. That verifier queries Parallax GraphQL, follows
linked RabbitMQ traces, and fails unless Catalog, Pricing, Inventory, Payment,
Fulfillment/RabbitMQ, Notifications, and analytics evidence are present.

## Storage assertions

After `a1` and `a4`, verify directly through SQL/ClickHouse/RabbitMQ tooling:

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
