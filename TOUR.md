# Parallax Demo Tour

Narrative walk after `parallax serve` is up and the playground compose stack
is emitting OTLP (+ Sentry envelopes). Each stop names the technology, the
concept, the scenario, what to look at (including a screenshot when one
exists), and why it matters (pain corpus:
`parallax/docs/research/market/competitor-pain-points.md`).

Spine: [`docs/coverage-matrix.md`](docs/coverage-matrix.md). Machine asserts:
`scenarios/run.sh c1` … `c11`. Display shots live in `artifacts/ui/`.

## 1. Whole-system ingest

- Technology: Rotel fan-out + Rust axum/tonic, Java Spring, TanStack web.
- Concept: one OTLP stream, no per-service setup.
- Scenario: `./demo.sh` or `scenarios/run.sh a1`
- See: Overview cards (spans, logs, metric points, error rate). Shot:
  `artifacts/ui/home-1440-dark.png` / `home-1440-light.png`.
- Why: local-dev gap and three-pillar onboarding are root 1 in the corpus.

## 2. Cross-language waterfall

- Technology: Rust checkout → tonic pricing + HTTP inventory/recommendation.
- Concept: W3C `traceparent` stitches SERVER/CLIENT/INTERNAL across languages.
- Scenario: `scenarios/run.sh a1`
- See: an **18-span** checkout waterfall, not a 1-span health scheduler.
  Live teaching id `8cf58d291fb795ef02fb67acff2a3431`. Shot:
  `artifacts/ui/traces-teach-waterfall-1440-dark.png`.
- Why: fragmented pillars hide the hop that actually failed.

## 3. gRPC streams that fail and cancel

- Technology: checkout `quote-stream` → pricing server-streaming.
- Concept: `rpc.message` SENT/RECEIVED, mid-stream fail, client cancel.
- Scenario: `scenarios/run.sh a7b`
- See: clean stream `b4e373608fffe78a` (many `rpc.message`); fail
  `338473de01e58f6f` (`pricing stream failed`). Shots:
  `traces-teach-stream-1440-dark.png`, `traces-teach-stream-fail-1440-dark.png`.
- Why: a unary RED chart cannot explain a stream that died on item 4.

## 4. GraphQL field cost (batch vs N+1)

- Technology: catalog Spring GraphQL + DataLoader.
- Concept: one `catalog.reviews.batch` vs one `reviewsSlow` per product.
- Scenario: `scenarios/run.sh a6`
- See: batch `40fd24943cfbe026` vs N+1 `9a0b6ed9ee5c20ac`. Shots:
  `traces-teach-batch-1440-dark.png`, `traces-teach-nplus1-1440-dark.png`.
- Why: the expensive field is not the HTTP envelope.

## 5. Async links

- Technology: Redpanda + orders producer/consumer.
- Concept: span links across trace roots.
- Scenario: `scenarios/run.sh a3`
- See: `linkedTraces` on `963391462dd5c46b`. Shot:
  `traces-teach-links-1440-dark.png`.
- Why: sampling guilt — the discarded consumer root is the incident.

## 6. Exemplars: metric → the exact trace

- Technology: catalog JVM `trace_based` exemplars.
- Concept: a counter point carries `trace_id`.
- Scenario: `scenarios/run.sh a2` (catalog traffic from a6 also seeds them)
- See: Metrics `catalog.product.queries` → trace `9a3941a829b19628`. Shot:
  `metrics-teach-exemplars-1440-dark.png`.
- Why: a histogram bucket without a joinable trace is another eyeball hunt.

## 7. Errors that group — dual OTLP + Sentry

- Technology: sentry-rust 0.49, sentry-java 8.53, Sentry JS 10.70,
  Parallax `/api/1/envelope/`.
- Concept: real SDK envelopes (not a synthetic `platform:native` POST).
- Scenario: `scenarios/run.sh c8` `a31` `c1`
- See: Issues `error: c8-rust-sdk PaymentError`,
  `IllegalStateException: c8-java-sdk PaymentError`,
  `Error: c8-js-sdk PaymentError`. Sentry Groups: `plat=native`, `plat=java`,
  `plat=node`. Handled 502 vs unhandled panic is a31.
- Why: grouping opacity is a named corpus pain; Sentry is the grouping
  authority for the cross-language PaymentError probe.

## 8. Flaky tests as first-class evidence

- Technology: `parallax invocation start -- scripts/observable-test-session.sh rust --acceptance`
- Concept: fail-then-pass attempt chain, `cli.invocation.id`.
- Scenario: that wrapper, then Tests explorer.
- See: `w4_assertion_failure_passes_on_retry` rollup `FLAKY_PASS`. Shot:
  `tests-teach-flaky-1440-dark.png`.
- Why: test-flakiness blindness is a differentiator none of the researched
  competitors have.

## 9. Logs: correlated, spiked, orphan

- Technology: tracing fields + SSE live tail.
- Concept: severity, structured KV, Query→Live, uncorrelated diagnostic.
- Scenario: `scenarios/run.sh a9` `b23` `c3`
- See: Logs facets; click **Query** (idle) → `?live=true`. Orphan issue
  `orphan diagnostic without trace context`.
- Why: sampled-out traces must not look like missing traffic.

## 10. Alerts that can be proven, including Slack dest

- Technology: error_rate rule + webhook + `slack_webhook`.
- Concept: rule → open incident.
- Scenario: `scenarios/run.sh c4`
- See: Alerts incident + dest kinds. Shot: `alerts-teach-incident-1440-dark.png`.
- Why: alert fatigue is #1 in two Grafana surveys.

## 11. Deploy adjacency and isolated prune

- Technology: GitHub HMAC fixture; CLI contexts; `--otlp-forward off`.
- Concept: change-triggered outages; prune must not eat the operator HOME.
- Scenario: `scenarios/run.sh c6` `c9`
- See: c6 200/401. c9 throwaway `$repo/.isolation/` HOME, `context add c9lab`.
- Why: ~70% of outages are change-triggered; silent quota drops are corpus
  root 1.

## 12. Redaction canary

- Technology: checkout `?canary=1` + redaction-lite-v3.
- Concept: canary tokens never leave on bundle / CLI / MCP / UI GraphQL /
  Sentry ack.
- Scenario: `scenarios/run.sh a18` `c10`
- Why: LLM secret-leak fear (OWASP LLM02).

## 13. Clock skew — honest miss

- Technology: checkout `?skew=1` (b-degradation).
- Concept: child starts before parent.
- Scenario: `scenarios/run.sh b-degradation`
- See: trace `0cc30e4ca53bb7f0` has degrade events. The **Clock skew
  suspected** banner does **not** appear — detector is cross-service.
  Shot: `traces-teach-skew-1440-dark.png`. W5 DISCREPANCY in the inventory.
- Why: a comparison that always favors Parallax is a failure state.

## 14. Display contract

- Technology: agent-browser snapshot/@ref.
- Concept: every coverage-matrix Parallax surface, desktop + phone, light +
  dark where rendering differs.
- Scenario: `scenarios/run.sh c11` plus `artifacts/ui/`.
- Why: a green ingest API with a blank UI is how the local-dev gap returns.

SigNoz is residue only (plan 162). Maple / OpenObserve / Sentry dispositions
live in [`VERIFICATION.md`](VERIFICATION.md).
