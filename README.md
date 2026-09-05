# Parallax Telemetry Playground

Polyglot commerce workload for exercising OpenTelemetry traces, logs, metrics,
Sentry envelopes, and browser RUM through one real business flow. Research
software; breaking changes are expected.

## Architecture

```text
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
[`deploy/postgres/migrate.sh`](deploy/postgres/migrate.sh) applies each
migration once, verifies the required schema, and records it in
`public.schema_migrations` before database clients start.
ClickHouse tables are initialized by [`deploy/clickhouse/init.sql`](deploy/clickhouse/init.sql).
Redis is a catalog/pricing cache, not a source of truth. RabbitMQ is durable;
publisher confirms, manual acknowledgements, retries, dead-letter queues,
W3C `traceparent`/`tracestate`/`baggage`, and consumer idempotency are part of
the application path.

## Run

Start the telemetry receiver first (`parallax serve`, or another OTLP listener
on host ports 4317/4318), then boot the complete stack:

```bash
docker compose -f deploy/docker-compose.yml up --build
```

Compose gates services that depend on the PostgreSQL migration job. The
verification runner waits for ClickHouse initialization before asserting
analytics state; a plain `up` is not itself an analytics-readiness proof.

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

For clean-volume bootstrap, use an isolated Compose project that has not been
used before. The second runner invocation proves the named-volume path without
resetting or deleting the volume:

```bash
project=telemetry-playground-clean-$(date +%s)
docker compose -p "$project" -f deploy/docker-compose.yml \
  up -d postgres postgres-migrate redis rabbitmq clickhouse clickhouse-init
docker compose -p "$project" -f deploy/docker-compose.yml exec -T postgres \
  psql -U postgres -d playground -Atqc \
  "SELECT version, count(*) FROM public.schema_migrations GROUP BY version;"
before="$(docker compose -p "$project" -f deploy/docker-compose.yml exec -T postgres \
  psql -U postgres -d playground -Atqc \
  "SELECT string_agg(version || '=' || applied_at::text, ',' ORDER BY version) FROM public.schema_migrations;")"
docker compose -p "$project" -f deploy/docker-compose.yml run --rm postgres-migrate
after="$(docker compose -p "$project" -f deploy/docker-compose.yml exec -T postgres \
  psql -U postgres -d playground -Atqc \
  "SELECT string_agg(version || '=' || applied_at::text, ',' ORDER BY version) FROM public.schema_migrations;")"
test "$before" = "$after"
docker compose -p "$project" -f deploy/docker-compose.yml down
```

## Verification

Rust gates:

```bash
rtk cargo fmt --all -- --check
rtk cargo check --workspace --all-targets --locked
rtk cargo test --workspace --all-targets --locked
```

Java gates:

```bash
(cd services/catalog && rtk proxy ./gradlew --no-daemon clean test)
(cd services/payment && rtk proxy ./gradlew --no-daemon clean test)
(cd services/fulfillment && rtk proxy ./gradlew --no-daemon clean test)
```

Web gates:

```bash
(cd web && rtk bun run build)
(cd web && rtk bun run test)
```

Then run `rtk git diff --check` and
`rtk proxy docker compose -f deploy/docker-compose.yml config --quiet`.

For the current verification contract, see [`docs/VERIFICATION.md`](docs/VERIFICATION.md).

For an automated Parallax topology assertion over one real checkout:

```bash
parallax invocation start -- scripts/verify-commerce-trace.sh
```

It checks the collected service, RabbitMQ, and analytics evidence through
`playground commerce-verify`. Inspect trace parent/link fields separately when
causal topology is required; service and queue co-presence is not proof of a
causal span edge.

## Journeys and scenarios

```bash
./scenarios/run.sh a1                 # checkout saga
./scenarios/run.sh a6                 # GraphQL batch/N+1/partial error
./scenarios/run.sh a7                 # Postgres NOTIFY price subscription
./scenarios/run.sh a7b                # pricing stream/failure/cancel
./scenarios/run.sh a23                # storefront → pricing gRPC
./scenarios/run.sh a24                # storefront → catalog GraphQL
./scenarios/run.sh a25                # Postgres slow/N+1/pool pressure
./scenarios/run.sh a26                # Catalog-backed recommendation
./scenarios/run.sh a14                # live checkoutFlow variant flip
./scenarios/run.sh a3                 # checkout → transactional outbox → fulfillment
./scenarios/run.sh a4                 # authenticated seeded-order replay → RabbitMQ → Rust
./scenarios/run.sh b-chaos            # provider decline and delay
./scenarios/run.sh b2                 # inventory failure
./scenarios/run.sh c11                # browser smoke, when the Parallax CLI is available
```

`./scenarios/run.sh` prints the complete catalog. The corner-case corpus is
documented in [`docs/corner-case-matrix.md`](docs/corner-case-matrix.md).
Ambient k6 traffic uses the optional Compose `demo` profile:

```bash
docker compose -f deploy/docker-compose.yml --profile demo up loadgen
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
