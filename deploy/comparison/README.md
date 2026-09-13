# Competitor lab: same workload, pinned versions, honest status

Fan-out harness for GOAL.md §11. One OTLP stream from the playground goes to
Parallax **and** to self-hostable competitors, so capability/UX comparisons use
exactly the same workload instead of prose.

## Pinned versions (verified 2026-09-13)

| Role | Image | Latest stable | Digest |
|---|---|---|---|
| Fan-out | `otel/opentelemetry-collector-contrib` | `0.160.0` (2026-09-02) | `sha256:799dc6cf12c96192af37b5bdba804da8c10b3bc563b43cb90c3f3c58d9572ad6` |
| Traces | `jaegertracing/jaeger` | `2.20.0` (2026-07-20) | `sha256:46a886260e04002d8f45e213fc39063fa11a50446048fdaa64786fc0840cb9f8` |
| Logs/metrics/traces | `openobserve/openobserve` | `v1.0.0` (2026-09-11) | `sha256:d581789cb03b5f061ed56a3e864b5e4bbc86bbf6d317ec863372e98ee49b30d5` |

Tags are GitHub `releases/latest` per repo; digests are the resolved
multi-arch manifest digests (`docker buildx imagetools inspect`). Never float
these tags: re-verify and re-pin on a schedule, recording the date above.

## Run

```bash
# 1. Start a normal `parallax serve` (OTLP gRPC on 127.0.0.1:4317).
parallax serve

# 2. Start the comparison backends (isolated project, no playground ports).
docker compose -f deploy/comparison/docker-compose.comparison.yml --project-name parallax-comparison up -d

# 3. Point the playground at the fan-out collector instead of Parallax.
export OTEL_EXPORTER_OTLP_ENDPOINT=http://127.0.0.1:24317
export OTEL_EXPORTER_OTLP_HTTP_ENDPOINT=http://127.0.0.1:24318

# 4. Run the same deterministic workload every backend receives.
mise run commerce:checkout_saga
mise run metrics:cardinality_stress
mise run propagation:malformed
```

Surfaces:

- Parallax: <http://127.0.0.1:4000> (operator's own instance)
- Jaeger UI: <http://127.0.0.1:36686>
- OpenObserve UI: <http://127.0.0.1:5080> (`root@example.com` / `Complexpass#123`)
- Collector zPages: <http://127.0.0.1:55679/debug/servicez>

## Fan-out proof

`mise run check:fanout` (Rust, `playground check-fanout`) emits one
deterministic `traces:deep` trace through the collector and asserts all 14
spans arrive in Parallax, Jaeger, and OpenObserve. It is a standalone live
check, not a registered scenario, so the scenario-corpus gate is unaffected
and the repo stays free of legacy shell wrappers.

```bash
PARALLAX_URL=http://127.0.0.1:4000 mise run check:fanout
```

`PARALLAX_URL` must point at a token-free instance whose OTLP ports are the
standard 4317/4318 the collector targets. `FANOUT_OTLP_HTTP`, `JAEGER_URL`,
`OO_URL`, and `FANOUT_TIMEOUT_SECS` override the remaining endpoints.

## Status (honest, 2026-09-13)

| Check | Result |
|---|---|
| Compose config validates | pass (`docker compose config --quiet`) |
| Pinned images pull | pass (all three digests resolve and pull) |
| Collector boots, Jaeger + OpenObserve healthy | pass (isolated `parallax-comparison` project) |
| Same trace reaches Jaeger | pass (`traces:deep` trace `af226d30…0001`: 1 trace, 14/14 spans via `/api/traces`) |
| Same trace reaches OpenObserve | pass (14/14 spans in stream `default`, `?type=traces` search) |
| Parallax leg of the fan-out | pass 2026-09-13 (trace `b27530ab…0001`, 14/14 spans in Parallax, Jaeger, and OpenObserve; Parallax was a token-free serve with standard OTLP ports 4317/4318; proven by the `check-fanout.sh` predecessor, since migrated to `mise run check:fanout`) |
| Same-workload UI comparison | **pending** (needs agent-browser passes over the fanned-out data) |
| SigNoz / Grafana stack / Uptrace | **not scaffolded** (heavier; next expansion) |

Query notes: OTLP traces land in the OpenObserve stream literally named
`default` (via the `stream-name` header) with stream type `traces`; search
needs `POST /api/default/_search?type=traces` plus `start_time`/`end_time`
epoch-micros, not the bare `_search` logs default.

## Failure log

Append every integration failure here with date, versions, symptom, and cause.
Do not silently downgrade a pinned version because the newest is harder to run:
record why, then decide.

- 2026-09-13: `openobserve/openobserve:v1.0.0` is distroless (no `/bin/sh`),
  so the in-container `wget` healthcheck could never pass. Fixed by disabling
  the Compose healthcheck and polling `/healthz` from the host. No version
  change.
- 2026-09-13: `otel/opentelemetry-collector-contrib:0.160.0` deprecates the
  `otlphttp` exporter alias (`use "otlp_http" instead`). Fixed by renaming in
  `otelcol-comparison.yaml`. No version change.
