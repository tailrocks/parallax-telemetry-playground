# Parallax Demo Tour

Narrative walk after `parallax serve` is up and the playground compose stack
is emitting OTLP (+ Sentry envelopes). Each stop names the technology, the
concept, the scenario, what to look at, and why it matters (pain corpus:
`parallax/docs/research/market/competitor-pain-points.md`).

Spine: [`docs/coverage-matrix.md`](docs/coverage-matrix.md). Machine asserts:
`scenarios/run.sh c1` … `c11`. Display pass: `artifacts/ui/` + c11.

## 1. Whole-system ingest

- Technology: Rotel fan-out + every playground service (Rust axum/tonic,
  Java Spring, TanStack web).
- Concept: one OTLP stream, no per-service setup.
- Scenario: `./demo.sh` or `scenarios/run.sh a1`
- See: Overview cards for spans, logs, metric points, error rate; Services
  lists checkout/catalog/payment/…
- Why: local-dev observability gap and three-pillar onboarding complexity
  are the first structural root in the pain corpus.

## 2. Cross-language waterfall

- Technology: Rust checkout → tonic pricing + HTTP inventory/recommendation
  + Java payment gRPC.
- Concept: W3C `traceparent` stitches span kinds across languages.
- Scenario: `scenarios/run.sh a1` then `a23`
- See: Traces waterfall — checkout SERVER with CLIENT/INTERNAL children;
  payment `Pricing/Quote` SERVER in the same tree.
- Why: fragmented pillars hide the hop that actually failed.

## 3. GraphQL field cost

- Technology: catalog Spring GraphQL + DataLoader.
- Concept: batched vs N+1 vs partial error vs operation-name policy.
- Scenario: `scenarios/run.sh a6`
- See: Trace detail field tree; N+1 is one `reviewsSlow` span per product.
- Why: the expensive field is not the HTTP envelope — eyeball dashboards
  miss it.

## 4. Async links and poison

- Technology: Redpanda + Rust orders + Java fulfillment.
- Concept: producer/consumer span links, batch fan-in, dead-letter.
- Scenario: `scenarios/run.sh a3` `a20` `b-async-chaos` `b21`
- See: Trace detail links; orphan consumer is a new root with
  `messaging.orphan=true`.
- Why: async work crossing trace roots is how "the discarded trace is the
  one you needed" (sampling guilt) shows up.

## 5. Errors that group — and ones that must not

- Technology: checkout `PaymentError` + Sentry SDK 0.49 + Parallax
  fingerprints.
- Concept: handled 502 vs unhandled panic; dual OTLP+Sentry; grouping.
- Scenario: `scenarios/run.sh b-chaos` `a31` `c1` `c8`
- See: Issues — PaymentError group vs panic 500; Sentry `verify.sh` A15/A16
  `times_seen`; GraphQL `bundle` hash on c1.
- Why: grouping opacity (Sentry over/under-group) and alerts-that-are-
  questions. Bundle is the evidence primitive the corpus says is missing.

## 6. Release and deploy adjacency

- Technology: `service.version` + GitHub deploy webhook fixture.
- Concept: v1→v2 regression; HMAC deploy ingest.
- Scenario: `scenarios/run.sh a13` `c6`
- See: Issues + Services release strip; deploy fixture HTTP 200 / bad HMAC
  401.
- Why: ~70% of outages are change-triggered; "what changed?" is still
  hunted by hand in the corpus.

## 7. Runtime, DB, cache

- Technology: tokio metrics, JVM agent, sqlx/Postgres, recommendation cache.
- Concept: gauges, pool wait, `db.query.text`, cache hit/miss/stampede.
- Scenario: `scenarios/run.sh a22` `b19` `a25` `a26`
- See: Services → checkout Runtime `tokio.runtime.*`; catalog `jvm.*`;
  inventory DB spans + `db.client.connection.*`; Metrics `cache_*`.
- Why: latency without a runtime lane becomes another eyeball hunt.

## 8. Metric shapes (teaching)

- Technology: checkout `playground-telemetry` up-down + bounded counter.
- Concept: up-down `http.server.active_requests`; histogram
  `http.server.request.duration`; `playground.cardinality.events` with
  `demo.bucket` ∈ 0..15; default CUMULATIVE temporality.
- Scenario: `scenarios/run.sh a30` `a1`
- See: Metrics catalog `http_server_active_requests` and
  `playground_cardinality_events_total`; workbench on
  `http_server_request_duration_seconds`.
- Why: cardinality anxiety and silent metric-math bugs (Uptrace 5×) are
  corpus roots; the playground teaches the safe shape, not an unbounded
  label.

## 9. Logs: correlated, spiked, orphan

- Technology: tracing fields + logback MDC + SSE live tail.
- Concept: severity ladder, structured KV, trace chip, uncorrelated
  diagnostic, live Query/Live toggle.
- Scenario: `scenarios/run.sh a9` `a29` `b23` `c3`
- See: Logs table + facets; Event column `checkout.completed`; orphan row
  with no chip; click **Query** → `?live=true`.
- Why: sampled-out traces must not look like missing traffic; live tail is
  how on-call stops refreshing.

## 10. RUM and broken continuation

- Technology: TanStack Start + `@sentry/tanstackstart-react` + browser OTLP.
- Concept: `session.id`, web-vitals, rage-click, `nopropagate`.
- Scenario: `scenarios/run.sh a28` `a5` `b15`
- See: Traces — browser route spans stitched to checkout; disconnected
  pair when `?nopropagate=1`.
- Why: frontend symptoms without a backend join are the three-pillars
  failure mode in a browser.

## 11. Feature flags and retry storms

- Technology: flagd + tonic deadlines.
- Concept: `feature_flag.evaluation` events; `rpc.grpc.status_code=4`.
- Scenario: `scenarios/run.sh a14` `b3b`
- See: Trace events; sibling `pricing.attempt` ERROR spans.
- Why: a flip and a deadline look like "the app is down" unless the
  evaluation and status code sit on the same waterfall.

## 12. Sampling guilt, made visible

- Technology: checkout `PLAYGROUND_SAMPLE_RATIO=0.1`.
- Concept: 10% traces, 100% logs.
- Scenario: `scenarios/run.sh b22`
- See: Traces thinner than Logs for the same window; dangling trace chips.
- Why: the corpus names sampling guilt explicitly — the discarded trace is
  the incident.

## 13. CLI Apps, tests, agent story

- Technology: `parallax invocation` + JUnit/nextest bridge + `import-claude`.
- Concept: `cli.invocation.id`, flaky fail-then-pass, agent-session import.
- Scenario: `scenarios/run.sh a12` `c2` `c7` and `observable-test-session`
- See: CLI Apps hub; Tests explorer (seed with `test-verify`); Story after
  c7 `import_id`.
- Why: test-flakiness blindness is a named differentiator; agent sessions
  need the same evidence primitive as human issues.

## 14. Saved workspace and SQL

- Technology: Turso metadata + Greptime SQL.
- Concept: dashboard / investigation / one SQL surface.
- Scenario: `scenarios/run.sh c5` then open `/sql`
- See: Dashboards `c5-dash`, Investigations `c5-case`, SQL `SELECT 1`.
- Why: query-language proliferation is root 1; one SQL surface is the
  counter.

## 15. Alerts that can be proven

- Technology: error_rate rule + webhook destination.
- Concept: rule → open incident after a seed.
- Scenario: `scenarios/run.sh c4`
- See: Alerts `c4-high-errors` + incident id.
- Why: alert fatigue is #1 in two Grafana surveys; a rule that cannot be
  driven end-to-end is theatre.

## 16. Redaction canary and prune

- Technology: redaction-lite-v3 + isolated-HOME doctor/prune.
- Concept: canary tokens never leave on bundle/webhook/log egress; prune
  is pin-aware and dry-run here.
- Scenario: `scenarios/run.sh a18` `c10` `c9`
- Why: LLM secret-leak fear (OWASP LLM02) and silent quota drops.

## 17. Display contract

- Technology: agent-browser v0.34 snapshot/@ref.
- Concept: every coverage-matrix Parallax surface, desktop 1440 + phone
  390, light + dark.
- Scenario: `scenarios/run.sh c11` (smoke) and the walk in
  `artifacts/ui/`.
- See: Overview not blank while `/health` is green; theme pills; ⌘K;
  Issues row → detail; Logs Query → Live.
- Why: a green ingest API with a blank UI is how "local-dev gap" returns.

SigNoz is residue only (plan 162, Foundry-only compose). Comparison arms
for Maple / OpenObserve / Sentry live in [`VERIFICATION.md`](VERIFICATION.md).
