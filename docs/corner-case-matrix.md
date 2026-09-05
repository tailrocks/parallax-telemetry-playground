# Corner-case matrix

Stable synthetic rendering corpus plus bounded live-service failure seams.
Run one with `./scenarios/run.sh <id>`. Synthetic shape generation is isolated
to the CLI; normal commerce journeys use seeded PostgreSQL data.

| ID | Trigger | Expected evidence |
|---|---|---|
| `t-deep` / `t-wide` / `t-multiroot` / `t-orphan` | `shapes <id>` | Deep, wide, multi-root, and detached trace trees render without dropping spans. |
| `t-skew` / `t-zero` / `t-links` | `shapes <id>` | Clock skew, zero duration, and bidirectional span-link shapes remain visible. |
| `t-longnames` / `t-events` | `shapes <id>` | Large Unicode values and multi-event exception payloads render safely. |
| `l-burst` / `l-bodies` / `l-patterns` | `shapes <id>` | Log burst, body, ANSI, multiline, equal-timestamp, and clustering behavior is inspectable. |
| `m-shapes` / `m-labels` / `f-attrs` | `shapes <id>` | Counter reset, gauge gap, exemplars, bounded labels, and method facets render correctly. |
| `e-burst` / `e-multi-lang` | `shapes <id>` | Repeated and cross-language error fingerprints group distinctly. |
| `eco-external` | `shapes eco-external` | External client edge has no invented server node. |
| `j-happy` / `j-error` / `j-outside` / `j-reattach` / `j-parallel` | `playground console ...` | Journey/session attribution and concurrent invocation isolation remain visible. |
| `p-grpc-err` | `a1` + `b3b` | Pricing gRPC success, invalid input, deadline, and unavailable outcomes are distinct. |
| `p-grpc-stream` | `a7b` | Stream SENT/RECEIVED events, failure, and cancellation are ordered and visible. |
| `p-graphql-err` | `a6` | Partial Catalog field error is distinct from request-level GraphQL failure. |
| `p-rabbitmq-lag` | `b-async-chaos` + `a4` | RabbitMQ consumer lag, retry, DLQ, and Java→Rust hop are linked. |
| `a-breach-error-rate` | sustained `tok_decline` checkout traffic | Error-rate alert sees explicit provider declines; no flag or fake SKU is required. |
| `a-recover` | sustained `tok_visa` checkout traffic | Healthy traffic resolves the error-rate window. |
| `a14` | flagd `checkoutFlow` flip | `control` and `orchestrated` string variants change behavior without restart. |
| `a25` | inventory reserve knobs | Real PostgreSQL slow query, bounded query fan-out, row lock, and pool pressure. |
| `a26` / `b13` / `b6` | recommendation query knobs | Catalog-backed recommendation, bounded stampede/slow/leak behavior. |
| `b-chaos` / `b2` | checkout/payment/inventory seams | Typed provider decline and inventory failure using real seeded SKUs. |
| `a7` | Catalog `updatePrice` + GraphQL WS | Committed PostgreSQL price change reaches subscribers through database notification. |

The matrix intentionally does not claim that a service runtime or UI passed
until the corresponding executable scenario and gates have been run.
