# Corner-case matrix

Stable synthetic rendering corpus plus bounded live-service failure seams.
Run one with `mise run <task>`. IDs below are internal fixture references only;
they are not public task names and have no aliases. Synthetic shape generation is
isolated to the CLI; normal commerce journeys use seeded PostgreSQL data.

| ID (internal) | Mise task | Expected evidence |
|---|---|---|
| `t-deep` / `t-wide` / `t-multiroot` / `t-orphan` | `traces:deep`, `traces:wide`, `traces:multi_root`, `traces:orphan` | Deep, wide, multi-root, and detached trace trees render without dropping spans. |
| `t-skew` / `t-zero` / `t-links` | `traces:clock_skew`, `traces:zero_duration`, `traces:cross_links` | Clock skew, zero duration, and bidirectional span-link shapes remain visible. |
| `t-longnames` / `t-events` | `traces:long_names`, `traces:events` | Large Unicode values and multi-event exception payloads render safely. |
| `l-burst` / `l-bodies` / `l-patterns` | `logs:burst`, `logs:bodies`, `logs:patterns` | Log burst, body, ANSI, multiline, equal-timestamp, and clustering behavior is inspectable. |
| `m-shapes` / `m-labels` / `f-attrs` | `metrics:shapes`, `metrics:labels`, `attributes:bounded` | Counter reset, gauge gap, exemplars, bounded labels, and method facets render correctly. |
| `e-burst` / `e-multi-lang` | `issues:burst`, `issues:multi_language` | Repeated and cross-language error fingerprints group distinctly. |
| `eco-external` | `ecosystem:external_edge` | External client edge has no invented server node. |
| `eco-service-map` | `ecosystem:service_map` | Completed CLI, browser, database, queue, and healthy/error external dependency identities expose service-map nodes, call counts, and error counts. |
| `j-happy` / `j-error` / `j-outside` / `j-reattach` / `j-parallel` | `journeys:happy_path`, `journeys:error_path`, `journeys:outside_screen`, `journeys:reattach`, `journeys:parallel` | Journey/session attribution and concurrent invocation isolation remain visible. |
| `p-grpc-err` | `protocols:grpc_errors` | Pricing gRPC success, invalid input, deadline, and unavailable outcomes are distinct. |
| `p-grpc-stream` | `protocols:grpc_stream` | Stream SENT/RECEIVED events, failure, and cancellation are ordered and visible. |
| `p-graphql-err` | `protocols:graphql_errors` | Partial Catalog field error is distinct from request-level GraphQL failure. |
| `p-rabbitmq-lag` | `protocols:rabbitmq_lag` | RabbitMQ consumer lag, retry, DLQ, and Java→Rust hop are linked. |
| `a-breach-error-rate` | `alerts:error_rate_breach` | Error-rate alert sees explicit provider declines; no flag or fake SKU is required. |
| `a-recover` | `alerts:recovery` | Healthy traffic resolves the error-rate window. |
| `a14` | `feature_flags:checkout_variants` | `control` and `orchestrated` string variants change behavior without restart. |
| `a25` | `postgres:query_pressure` | Real PostgreSQL slow query, bounded query fan-out, row lock, and pool pressure. |
| `a26` / `b13` / `b6` | `cache:recommendation_stampede`, `recommendation:slow_query`, `memory:cache_leak` | Catalog-backed recommendation, bounded stampede/slow/leak behavior. |
| `b-chaos` / `b2` | `failures:payment_latency`, `failures:inventory` | Typed provider decline and inventory failure using real seeded SKUs. |
| `a7` | `database:price_subscription` | Committed PostgreSQL price change reaches subscribers through database notification. |

The matrix intentionally does not claim that a service runtime or UI passed
until the corresponding executable scenario and gates have been run.
