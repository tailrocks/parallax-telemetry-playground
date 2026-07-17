# Corner-case matrix (plan 161)

Research date: 2026-07-17. One deterministic scenario per Parallax UI
rendering risk. Ids are stable API for plans 159 (live acceptance) and 160
(UI defect audit): never rename, only add. Run one with
`scenarios/run.sh <id>` (or `scenarios/corner-cases.sh <id>`); run the whole
corpus with `scenarios/corner-cases.sh --all-corner-cases`. Scenarios are
deterministic in SHAPE (counts/structure); ids and timestamps vary per run.
Synthetic shapes export as `service.name=playground-shapes`; journey
scenarios use the `playground console` sim; protocol scenarios ride the real
service paths.

The "known-broken?" column is owned by plan 160 (Parallax repo): it records
surfaces the audit found rendering incorrectly, and empties as fixes land.

| Id | Trigger | Signals emitted (shape) | Target surface | Expected rendering | Known-broken? |
|---|---|---|---|---|---|
| `t-deep` | `shapes t-deep` | 1 trace, 14-span linear chain, alternating tier names | Trace waterfall | Full depth renders with correct nesting; no depth cap truncation | |
| `t-wide` | `shapes t-wide` | 1 trace, 521 spans fanning from one root | Waterfall + minimap | Virtualized list stays responsive; minimap samples but row count says 521 | |
| `t-multiroot` | `shapes t-multiroot` | 1 trace id, 2 root spans | Waterfall | Both roots render; neither is dropped or misparented | |
| `t-orphan` | `shapes t-orphan` | root + child whose parent id never arrives | Waterfall | Detached child renders (flagged/detached), never vanishes | |
| `t-skew` | `shapes t-skew` | CLIENT parent; SERVER child on second service starts 120 ms earlier (crosses the 50 ms cross-service skew threshold) | Waterfall | Non-negative bars; skew flagged; child not clipped | |
| `t-zero` | `shapes t-zero` | zero-duration span + 1 µs span | Waterfall | Visible zero-width markers; no NaN/divide-by-zero layout | |
| `t-links` | `shapes t-links` | 2 traces linked to each other | Trace detail links panel | Both directions navigable | |
| `t-longnames` | `shapes t-longnames` | 1-4 KiB unicode/emoji names, values, keys | Waterfall + span inspector | Truncation with tooltip/copy; no layout break | |
| `t-events` | `shapes t-events` | 1 span, 48 progress events + 3 exception events (Rust/Java/browser stacks) | Span events panel | All 51 events listed; multi-line stacks render preformatted | |
| `l-burst` | `shapes l-burst` | 5,000 logs in seconds across 5 severities | Logs live tail + histogram | Tail caps without stalling; histogram reflects the burst | |
| `l-bodies` | `shapes l-bodies` | JSON body, 32 KiB body, ANSI escapes, blank body, 5 identical-timestamp rows | Logs table + row sheet | Stable ordering for equal timestamps; oversized body truncates; ANSI not raw-rendered | |
| `l-patterns` | `shapes l-patterns` | 20,000 lines: 11 steady templates × 1,200 (ids/ips/durations churn) + one `connection reset…` spike template × 6,800 concentrated in the last fifth of a 5-minute window | Logs patterns toggle (plan 165) | Drain clustering yields ~12 clusters with the documented counts; the spike template ranks first in the late window | |
| `m-shapes` | `shapes m-shapes` | monotonic counter with mid-window reset, gauge with a 2-step gap, `http.server.request.duration` histogram whose exemplar references an exported anchor trace | Dashboards + service latency panel | Rate handles the reset (no negative spike); gauge gap visible; exemplar marker deep-links to the anchor trace | |
| `m-labels` | `shapes m-labels` | one gauge (`shapes.region.load`) + one monotonic sum (`shapes.region.requests_total`) emitted with a `region` label ∈ eu/us/ap at fixed 6/3/1 magnitudes across 4 timestamps | Metrics explorer group-by breakdown (plan 168) | Group-by `region` yields exactly three series with the 6/3/1 split; sum stays monotonic per region | |
| `f-attrs` | `shapes f-attrs` | 100 spans + 100 logs carrying `http.request.method` split exactly 70/20/10 GET/POST/DELETE (plus `shape.case=f-attrs`) | Facet sidebars + where-clause editor (plan 164) | Facet counts read 70/20/10; `http.request.method = "POST"` narrows to exactly 20 | |
| `e-burst` | `shapes e-burst` | 100× one `error.type` + 5 distinct types, one invocation | Issues list + hub errors tab | One grouped issue with count 100 trend; breakdown shows 6 types | |
| `e-multi-lang` | `shapes e-multi-lang` | same logical failure with Rust/Java/browser exception shapes | Issues list | Three distinguishable fingerprints, language-appropriate titles | |
| `p-grpc-err` | real checkout + pricing paths | gRPC OK, INVALID_ARGUMENT, DEADLINE_EXCEEDED (rpc.grpc.status_code=4) | Trace detail RPC panel | Status codes surfaced per attempt span | |
| `p-grpc-stream` | `a7b` path | streaming RPC with SENT/RECEIVED message events, failure + cancel | RPC stream panel | Per-message events ordered; failure/cancel visible | |
| `p-graphql-err` | `a6` path | GraphQL field error with partial data + request-level error | GraphQL operation panel + issues | Partial-data field error distinct from request error | |
| `p-kafka-lag` | orders lag/poison + fulfillment Kafka | delayed CONSUMER, dead-letter path, Java→Rust hop | Trace detail + jobs view | Producer/consumer gap visible; dead-letter attempt shows outcome=failure | |
| `j-happy` | `console --seconds 6` | session pair, 3 screen visits, 3 successful ui.action roots | Invocation hub journey | Chronological narrative; every action links to its trace | |
| `j-error` | `console --fail-at checkout.submit` | forced failure inside the checkout visit with `app.widget.name` | Journey error attribution | Error attributed to the checkout screen and submitting widget | |
| `j-outside` | `console --outside-error` | error log between screen visits | Journey unattributed bucket | Error renders in "outside any screen", never dropped | |
| `j-reattach` | `console --reattach 3` | 3 sessions chained via `session.previous_id` | Sessions tab | Chain of ≥3 sessions navigable via previous-id links | |
| `j-parallel` | 3 concurrent consoles + daemon | 4 distinct invocation ids with interleaved signals | Invocations list + hub isolation | Each hub shows only its own signals; list shows 4 rows | |
| `eco-full` | composite pass over every edge | browser, storefront→catalog/payment, Kafka leg, CLI→checkout | Ecosystem graph | cli/browser/service kinds all present; every edge drawn | |

Regression discipline: every future UI rendering bug gets a scenario id here
BEFORE its fix lands in Parallax.
