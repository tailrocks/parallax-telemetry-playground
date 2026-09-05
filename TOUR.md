# Parallax Demo Tour

Start an OTLP receiver, then run `docker compose -f deploy/docker-compose.yml
up --build`. The browser is at `http://localhost:5173`; Parallax is expected
at its normal local UI. Each stop below drives the current commerce path.

## 1. Browse and cache

Open the web Catalog route or run `./scenarios/run.sh a24`.

The browser calls the Rust storefront GraphQL gateway. Catalog resolves durable
PostgreSQL products, variants, prices, categories, and reviews. Redis records
cache-aside hits without becoming the source of truth. Change
`catalogExperience` in flagd to `standard`, `featured`, or `newest` and reload;
the returned page order and trace attributes change.

## 2. Checkout saga

Run `./scenarios/run.sh a1` or submit Checkout in the browser.

One request validates Catalog, obtains an itemized Pricing gRPC quote, creates a
pending order and cart in PostgreSQL, authorizes and captures through Java
Payment gRPC, reserves Inventory with PostgreSQL row locks, writes analytics
and a transactional outbox record, and asks Recommendation for catalog-backed
related products. W3C context and business baggage cross every hop.

Use `payment_method_token=tok_decline` for a deliberate provider decline. This
is the isolated failure seam; normal SKUs and prices never encode failure.

## 3. Async fulfillment

Run `./scenarios/run.sh a4`.

Java Fulfillment publishes seeded Acme and Nova order events to durable
RabbitMQ with publisher confirms. Its consumer uses manual acknowledgements,
bounded retry, dead-letter routing, durable PostgreSQL processing claims, and a
span link to the producer. Shipment state, ClickHouse analytics, and the Rust
Notifications hop are visible downstream.

Run `./scenarios/run.sh b-async-chaos` for bounded lag and poison retry/DLQ.

## 4. Streaming and GraphQL failure shape

- `./scenarios/run.sh a7b`: Pricing server stream with message events, failure,
  and client cancellation.
- `./scenarios/run.sh a6`: Catalog batched reviews, deliberate `reviewsSlow`
  N+1 shape, and an optional explicit synthetic `riskScore` error. Set
  `CATALOG_SYNTHETIC_RISK_SCORE_FAILURE_SKU` and recreate Catalog to exercise it;
  unset, normal products return a risk score.
- `bun scenarios/a7-subscription.ts`: subscribe to price changes, then execute
  the Catalog `updatePrice` mutation. The event is committed in PostgreSQL and
  delivered from the durable PostgreSQL price journal, with LISTEN only as the
  low-latency wake-up. After receiving the event, the harness restores seeded
  `WIDGET-1` to 1,999 minor units (`$19.99`, compare-at `$22.99`) before it
  exits, so repeated runs start from the same price. The
  `./scenarios/run.sh a7` wrapper invokes this same script.

## 5. Analytics and status

Open the browser Analytics and Orders routes, or call Storefront GraphQL. The
Analytics resolver reads durable ClickHouse `analytics_events`. Orders are read
from Checkout's PostgreSQL API. Storefront's `cart` query and `addCartItem`
mutation forward to Checkout's durable PostgreSQL cart; the browser's staged
SKU list is never treated as authoritative pricing or inventory.
The web routes show pending/paid/processing/delivered distinctions returned by
the services.

## 6. Feature variants

Run `./scenarios/run.sh a14`. Flip `checkoutFlow` between `control` and
`orchestrated` while services stay up. The selected string variant is evaluated
through flagd with tenant/customer context, recorded in the response and
trace, and changes optional orchestration behavior. Catalog ordering uses
`catalogExperience`; pricing and recommendation expose their selected strategy
in request context.

## 7. Database, cache, and runtime edges

- `./scenarios/run.sh a25`: Postgres slow query, bounded N+1, and pool
  pressure.
- `./scenarios/run.sh a26`: repeated and parallel Catalog-backed
  recommendations; explicit bounded stampede/slow/leak cases only.
- `./scenarios/run.sh b-chaos`: provider decline and delayed checkout.
- `./scenarios/run.sh b2`: deterministic Inventory failure.

Inspect spans, logs, metrics, baggage, span links, outbox rows, RabbitMQ
queues/DLQs, PostgreSQL state, and ClickHouse rows together. That cross-signal
join is the point of the workload.

The complete command catalog is `./scenarios/run.sh`; the stable synthetic
rendering corpus is [`docs/corner-case-matrix.md`](docs/corner-case-matrix.md).
