# Scenarios

Run `./scenarios/run.sh` for the full catalog or
`./scenarios/run.sh <id>` for one bounded journey. Start the stack first unless
the scenario says it owns Compose startup.

| ID | Script | Current path |
|---|---|---|
| `a1` | `a1-checkout.sh` | Catalog → Pricing → Payment → Inventory → outbox checkout saga |
| `a2` | `a2-exemplars.sh` | Catalog exemplar traffic |
| `a5` | `a5-rum-error.sh` | Browser catalog-to-checkout journey |
| `a6` | `a6-graphql.sh` | GraphQL batch, N+1, and partial-error shapes |
| `a7` | `a7-subscription.ts` | PostgreSQL-backed GraphQL price subscription |
| `a7b` | `a7b-grpc-stream.sh` | Pricing server stream, rejection, and cancellation |
| `a8` | `a8-java-async.sh` | Authenticated seeded-order replay through Java Fulfillment RabbitMQ |
| `a9` | `a9-field-spike.sh` | CLI log-pattern corpus |
| `a10` | `a10-baggage.sh` | Checkout W3C tenant/tier baggage |
| `a3` | `a3-async.sh` | Checkout transactional outbox → RabbitMQ → Fulfillment → Notifications |
| `a4` | `a4-reverse.sh` | Authenticated seeded-order replay → Fulfillment RabbitMQ → Notifications HTTP |
| `a12` | `a12-cli-run.sh` | Playground CLI checkout driver |
| `a13` | `a13-deploy-regression.sh` | Release attribution comparison |
| `a14` | `a14-flag-flip.sh` | Live OpenFeature checkoutFlow variants |
| `a18` | `a18-canary.sh` | Sentry envelope redaction corpus |
| `a19` | `a19-long-trace.sh` | CLI wide-trace corpus |
| `a20-compare` | `a20-compare-pair.sh` | checkoutFlow topology comparison |
| `a20` | `a20-batch-fanin.sh` | Concurrent synthetic orders-queue messages |
| `a22` | `a22-tokio-saturation.sh` | Bounded delayed checkout pressure |
| `a23` | `a23-storefront-grpc.sh` | Storefront GraphQL → Pricing gRPC |
| `a24` | `a24-storefront-catalog.sh` | Storefront GraphQL → Catalog GraphQL |
| `a25` | `a25-postgres.sh` | Inventory PostgreSQL query and pool behavior |
| `a26` | `a26-cache.sh` | Catalog-backed recommendation stampede |
| `a27` | `a27-execution-stack.sh` | CLI execution-stack corpus |
| `a28` | `a28-rum-journey.sh` | Browser routes, RUM, and journey evidence |
| `a29` | `a29-typed-events.sh` | Typed events across Rust, Java, and web |
| `b-async-chaos` | `b-async-chaos.sh` | Synthetic orders-queue lag, retry, and dead letter |
| `b2` | `b2-inventory-failure.sh` | Explicit inventory failure |
| `b5` | `b5-cpu-pressure.sh` | Bounded checkout latency pressure |
| `b6` | `b6-cache-leak.sh` | Recommendation cache-leak traffic |
| `b10` | `b10-lock-contention.sh` | Concurrent delayed checkout |
| `b13` | `b13-slow-recommendation.sh` | Bounded recommendation slowness |
| `b15` | `b15-rage-click.sh` | Browser order and analytics journeys |
| `b16` | `b16-load.sh` | k6 checkout load |
| `b-chaos` | `b-chaos.sh` | Payment failure and latency |
| `b-checkout-chaos` | `b-checkout-chaos.sh` | Pricing retry/timeout and delayed checkout |
| `b3b` | `b3b-grpc-deadline.sh` | gRPC deadline and retry spans |
| `a-breach-error-rate` | `a-breach-error-rate.sh` | Sustained checkout failure alert |
| `a-breach-p95` | `a-breach-p95.sh` | Sustained recommendation latency alert |
| `a-recover` | `a-recover.sh` | Healthy traffic incident recovery |
| `b-degradation` | `b-degradation.sh` | Provider-unavailable degradation |
| `b17` | `b17-cron.sh` | Cron success/failure/stuck outcomes |
| `b17b` | `b17b-cron-suite.sh` | Cron duplicate and missed-slot suite |
| `b19` | `b19-jvm-gc-pressure.sh` | Catalog GraphQL workload |
| `b20` | `b20-container-oom.sh` | Bounded recommendation memory pressure |
| `b21` | `b21-orphan-consumer.sh` | Linked/orphan synthetic orders consumers |
| `b22` | `b22-sampling-gap.sh` | Low-sample checkout evidence |
| `b23` | `b23-uncorrelated-log.sh` | Correlated checkout logs |
| `t-deep` | `corner-cases.sh` | Deep trace corpus |
| `t-wide` | `corner-cases.sh` | Wide trace corpus |
| `t-multiroot` | `corner-cases.sh` | Multi-root trace corpus |
| `t-orphan` | `corner-cases.sh` | Orphan trace corpus |
| `t-skew` | `corner-cases.sh` | Clock-skew trace corpus |
| `t-zero` | `corner-cases.sh` | Zero-duration trace corpus |
| `t-links` | `corner-cases.sh` | Cross-linked traces |
| `t-longnames` | `corner-cases.sh` | Long-name trace corpus |
| `t-events` | `corner-cases.sh` | Span-event trace corpus |
| `l-burst` | `corner-cases.sh` | Log burst corpus |
| `l-bodies` | `corner-cases.sh` | Log body corpus |
| `l-patterns` | `corner-cases.sh` | Log pattern corpus |
| `m-shapes` | `corner-cases.sh` | Metric shape corpus |
| `m-labels` | `corner-cases.sh` | Metric label corpus |
| `f-attrs` | `corner-cases.sh` | Trace/log attribute corpus |
| `eco-external` | `corner-cases.sh` | External-edge corpus |
| `e-burst` | `corner-cases.sh` | Issue burst corpus |
| `e-multi-lang` | `corner-cases.sh` | Multi-language issue corpus |
| `p-grpc-err` | `corner-cases.sh` | gRPC error corpus |
| `p-grpc-stream` | `corner-cases.sh` | gRPC stream corpus |
| `p-graphql-err` | `corner-cases.sh` | GraphQL error corpus |
| `p-rabbitmq-lag` | `corner-cases.sh` | RabbitMQ lag corpus |
| `j-happy` | `corner-cases.sh` | Successful CLI journey corpus |
| `j-error` | `corner-cases.sh` | Failed CLI journey corpus |
| `j-outside` | `corner-cases.sh` | Unattributed CLI journey corpus |
| `j-reattach` | `corner-cases.sh` | Reattached CLI sessions |
| `j-parallel` | `corner-cases.sh` | Parallel CLI invocations |
| `eco-full` | `corner-cases.sh` | Full ecosystem corpus |
| `c1` | `c1-issue-context.sh` | Issue context and evidence |
| `c2` | `c2-invocation-lifecycle.sh` | Invocation lifecycle |
| `c3` | `c3-live-tail.sh` | Live logs/traces tail |
| `c4` | `c4-alerting.sh` | Alert and incident flow |
| `c5` | `c5-saved-state.sh` | Saved dashboard/investigation state |
| `c6` | `c6-github-ingest.sh` | GitHub deploy webhook |
| `c7` | `c7-agent-session.sh` | Agent session import |
| `c8` | `c8-sentry-envelope.sh` | SDK envelope ingestion |
| `c9` | `c9-lifecycle-ops.sh` | CLI lifecycle operations |
| `c10` | `c10-redaction-egress.sh` | Redaction across egress paths |
| `c11` | `c11-ui-agent-verify.sh` | Agent-browser snapshots for every core surface while health is green |
| `a30` | `a30-metric-shapes.sh` | Checkout request metrics |
| `a31` | `a31-handled-unhandled.sh` | Typed handled/unhandled failures |

Scenario inputs use only seeded tenants/customers/SKUs for normal paths:

- Acme: `tenant-acme`, `customer-acme-ava`, `WIDGET-1`, `WIDGET-2`,
  `GADGET-1`, `GADGET-2`.
- Nova: `tenant-nova`, `customer-nova-mia`, `NOVA-PACK-20`,
  `NOVA-PACK-30`, `NOVA-LAMP-DESK`, `NOVA-LAMP-FLOOR`.

Synthetic `delay`, `fail`, `leak`, `stampede`, poison, and similar controls
are bounded and explicit. They are not product data or normal success paths.

The real checkout/outbox/fulfillment proof is `a1` or `a3`. The orders
`/order` endpoint is deliberately isolated on the private `orders.synthetic`
exchange; `a20`, `b-async-chaos`, `b21`, and the order leg of `a29` use it only
for messaging-shape fixtures. They do not prove checkout outbox delivery.
