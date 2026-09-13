# Scenarios

Run `mise tasks ls --sort name` for the full catalog or `mise run <task>` for
one bounded journey. IDs below are internal fixture references only; they are
not public task names and have no aliases. Start the stack first unless the
task says it owns Compose startup.

`mise run check:scenarios` validates the public catalog and legacy-wrapper
policy. `mise run corpus:all` dispatches all 89 proofs exactly once: 61 A/B/C
proofs plus 28 corner proofs. This proves dispatch coverage only; it does not
prove that every runtime journey, browser assertion, or Parallax failure gate
has passed.

| ID (internal) | Mise task | Capability |
|---|---|---|
| `a1` | `commerce:checkout_saga` | Checkout saga across Catalog, Pricing, Payment, Inventory, outbox, and fulfillment |
| `a2` | `metrics:exemplars` | Trace-linked catalog metric exemplars |
| `a5` | `browser:rum_error` | Browser catalog-to-checkout journey with a rendered error |
| `a6` | `graphql:batching_errors` | GraphQL batching, N+1, partial data, and operation-name telemetry |
| `a7` | `database:price_subscription` | Catalog GraphQL price subscription over PostgreSQL notifications |
| `a7b` | `grpc:pricing_stream` | Pricing gRPC stream messages, rejection, and cancellation |
| `a8` | `messaging:java_fulfillment_replay` | Authenticated seeded-order replay through Java fulfillment and RabbitMQ |
| `a9` | `logs:field_spike` | CLI log-pattern corpus with a late spike |
| `a10` | `propagation:baggage` | W3C baggage propagation for tenant and user tier |
| `a3` | `messaging:checkout_outbox` | Checkout transactional outbox linked to Java fulfillment consumer |
| `a4` | `messaging:seeded_order_replay` | Seeded-order replay with Java fulfillment and Rust notification hops |
| `a12` | `cli:checkout_invocation` | Playground CLI checkout driver with invocation telemetry |
| `a13` | `deploy:release_regression` | Compare valid checkout traffic across v1 and v2 releases |
| `a14` | `feature_flags:checkout_variants` | Flip checkoutFlow feature variants without restarting services |
| `a18` | `security:redaction_canary` | Sentry envelope redaction corpus with fake PII and credentials |
| `a19` | `traces:wide_trace` | CLI 521-span trace for waterfall and minimap stress |
| `a20-compare` | `feature_flags:topology_compare` | Compare checkoutFlow recommendation topology variants |
| `a20` | `messaging:batch_fanin` | Synthetic orders producer and consumer links across concurrent messages |
| `a22` | `runtime:request_saturation` | Concurrent delayed checkout spans and active-request pressure |
| `a23` | `grpc:storefront_pricing` | Storefront GraphQL resolver calling Pricing gRPC |
| `a24` | `graphql:storefront_catalog` | Storefront GraphQL resolver calling Catalog GraphQL |
| `a25` | `postgres:query_pressure` | PostgreSQL queries, fan-out, row locks, and pool pressure |
| `a26` | `cache:recommendation_stampede` | Catalog-backed recommendation fan-out and cache stampede |
| `a27` | `agent:execution_stack` | CLI, daemon, capsule, and agent execution-stack story |
| `a28` | `browser:rum_journey` | Browser routes, web vitals, sessions, and backend stitching |
| `a29` | `events:typed_business_events` | Typed business events across checkout, orders, catalog, and web |
| `b-async-chaos` | `messaging:poison_retry` | Synthetic orders lag, poison message, retry, and dead letter |
| `b2` | `failures:inventory` | Deterministic inventory failure correlated with checkout impact |
| `b5` | `runtime:cpu_pressure` | Checkout latency, active requests, and slow spans under pressure |
| `b6` | `memory:cache_leak` | Recommendation cache growth with feature-flag evidence |
| `b10` | `postgres:lock_contention` | Concurrent checkout spans through bounded lock delay |
| `b13` | `recommendation:slow_query` | Bounded slow recommendation latency |
| `b15` | `browser:rage_click` | Browser order and analytics route evidence |
| `b16` | `load:checkout` | Sustained checkout load from k6 |
| `b-chaos` | `failures:payment_latency` | Checkout payment failure and latency rendering |
| `b-checkout-chaos` | `failures:checkout_chaos` | Pricing retry, timeout, and delayed checkout paths |
| `b3b` | `grpc:deadline_retry` | Pricing gRPC deadline and retry spans |
| `a-breach-error-rate` | `alerts:error_rate_breach` | Sustained provider declines opening a checkout error-rate incident |
| `a-breach-p95` | `alerts:p95_breach` | Sustained recommendation latency opening a p95 incident |
| `a-recover` | `alerts:recovery` | Healthy traffic resolving alert incidents |
| `b-degradation` | `failures:provider_degradation` | Provider-unavailable degradation and delayed checkout |
| `b17` | `cron:outcomes` | Cron success, failure, and stuck outcomes |
| `b17b` | `cron:duplicate_missed` | Cron duplicate, missed-slot, and invocation identity outcomes |
| `b19` | `jvm:memory_pressure` | JVM catalog workload for garbage-collection and memory pressure |
| `b20` | `container:recommendation_oom_probe` | Destructive recommendation container OOM probe; requires `--yes` |
| `b21` | `messaging:orphan_consumer` | Linked and orphan synthetic orders consumers |
| `b22` | `sampling:low_sample_gap` | Low-rate root sampling with complete log evidence |
| `b23` | `logs:trace_correlation` | Checkout rows retaining trace and span correlation |
| `t-deep` | `traces:deep` | Fourteen-span linear trace depth corpus |
| `t-wide` | `traces:wide` | 521-span fan-out trace virtualization corpus |
| `t-multiroot` | `traces:multi_root` | Multi-root trace rendering corpus |
| `t-orphan` | `traces:orphan` | Detached child span rendering corpus |
| `t-skew` | `traces:clock_skew` | Clock-skew span timing corpus |
| `t-zero` | `traces:zero_duration` | Zero-duration and one-microsecond span corpus |
| `t-links` | `traces:cross_links` | Bidirectional span-link navigation corpus |
| `t-longnames` | `traces:long_names` | Long Unicode span name and value corpus |
| `t-events` | `traces:events` | Span-event and stacktrace rendering corpus |
| `l-burst` | `logs:burst` | Five-thousand-log burst and live-tail corpus |
| `l-bodies` | `logs:bodies` | JSON, large-body, ANSI, blank, and equal-time log corpus |
| `l-patterns` | `logs:patterns` | Drain log clustering and late-spike corpus |
| `m-shapes` | `metrics:shapes` | Counter reset, gauge gap, and exemplar histogram corpus |
| `m-labels` | `metrics:labels` | Metric label grouping by region corpus |
| `f-attrs` | `attributes:bounded` | Trace and log HTTP method facet corpus |
| `e-burst` | `issues:burst` | Recurring and distinct error fingerprint corpus |
| `e-multi-lang` | `issues:multi_language` | Cross-language issue fingerprint corpus |
| `p-grpc-err` | `protocols:grpc_errors` | Pricing success, validation, and deadline error corpus |
| `p-grpc-stream` | `protocols:grpc_stream` | Streaming RPC per-message event corpus |
| `p-graphql-err` | `protocols:graphql_errors` | GraphQL partial-data and request-error corpus |
| `p-rabbitmq-lag` | `protocols:rabbitmq_lag` | RabbitMQ lag, retry, and dead-letter corpus |
| `j-happy` | `journeys:happy_path` | Successful CLI home-to-checkout journey corpus |
| `j-error` | `journeys:error_path` | Failed CLI checkout journey with widget context |
| `j-outside` | `journeys:outside_screen` | Unattributed CLI journey error corpus |
| `j-reattach` | `journeys:reattach` | Reattached CLI session chain corpus |
| `j-parallel` | `journeys:parallel` | Concurrent CLI invocation isolation corpus |
| `eco-external` | `ecosystem:external_edge` | External client edge without an invented server node |
| `eco-full` | `ecosystem:full` | Complete CLI, browser, and service ecosystem corpus |
| `c1` | `product:issue_context` | Issue context, evidence bundle, and resolution |
| `c2` | `product:invocation_lifecycle` | CLI invocation lifecycle and bundle |
| `c3` | `product:live_tail` | Live logs and traces over SSE |
| `c4` | `product:alerting` | Alert rule, incident, and webhook flow |
| `c5` | `product:saved_state` | Saved dashboard and investigation state |
| `c6` | `product:github_ingest` | GitHub deploy webhook with HMAC verification |
| `c7` | `product:agent_session` | Claude session import and MCP agent context |
| `c8` | `sentry:envelopes` | Real Rust, Java, and JavaScript Sentry envelopes |
| `c9` | `product:lifecycle_ops` | Isolated HOME lifecycle, prune, context, and argument forwarding |
| `c10` | `security:redaction_egress` | Redaction across bundle, MCP, UI, webhook, and Sentry |
| `c11` | `product:ui_agent_verify` | Agent-browser surface smoke plus a real click traversal: issue → occurrence trace → span → surrounding logs → metric exemplar, with back-navigation and empty/error/high-volume states |
| `a30` | `metrics:request_shapes` | Active requests, request latency, and checkout metrics |
| `a31` | `errors:handled_unhandled` | Handled payment decline versus provider-internal failure |

Scenario inputs use only seeded tenants/customers/SKUs for normal paths:

- Acme: `tenant-acme`, `customer-acme-ava`, `WIDGET-1`, `WIDGET-2`,
  `GADGET-1`, `GADGET-2`.
- Nova: `tenant-nova`, `customer-nova-mia`, `NOVA-PACK-20`,
  `NOVA-PACK-30`, `NOVA-LAMP-DESK`, `NOVA-LAMP-FLOOR`.

Synthetic `delay`, `fail`, `leak`, `stampede`, poison, and similar controls
are bounded and explicit. They are not product data or normal success paths.

The real checkout/outbox/fulfillment proof is `commerce:checkout_saga` or
`messaging:checkout_outbox`. The `/order` endpoint is deliberately isolated on
the private `orders.synthetic` exchange; `messaging:batch_fanin`,
`messaging:poison_retry`, `messaging:orphan_consumer`, and the order leg of
`events:typed_business_events` use it only for messaging-shape fixtures. They
do not prove checkout outbox delivery.

## Runtime verification

```bash
mise run verify:commerce_stack
cd web && bun run e2e:compose
mise run verify:commerce_trace
```

The stack verifier proves Compose/service/dependency readiness. The browser
command is the canonical real-Compose journey. Parallax-backed failure
scenarios and the dedicated browser causal assertion remain pending; these
commands do not by themselves complete the DOD.
