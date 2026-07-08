# Parallax Demo Tour

Use this path after `parallax serve` is running in the Parallax repo.

## 1. Baseline Traffic

- Command: `./demo.sh`
- Route: Overview
- See: live span, log, error, and service charts move while the load profile runs.
- Proves: Parallax can ingest the whole playground without per-service setup.

## 2. Checkout Waterfall

- Command: `scenarios/run.sh a1`
- Route: Traces
- See: checkout server span with pricing, inventory, and recommendation children.
- Proves: W3C propagation stitches Rust HTTP and gRPC calls into one trace.

## 3. Error Grouping

- Command: `scenarios/run.sh b-chaos`
- Route: Issues
- See: checkout payment failures grouped with ERROR spans and slow requests.
- Proves: error spans and logs can drive issue-style triage.

## 4. Async Link

- Command: `scenarios/run.sh a3`
- Route: Trace detail
- See: producer span linking to the consumer trace.
- Proves: async work can stay explainable even when it crosses trace roots.

## 5. Release Attribution

- Command: `scenarios/run.sh a13`
- Route: Issues, Services
- See: checkout v2 errors and service release windows.
- Proves: regressions can be tied back to service.version.

## 6. Runtime Lanes

- Command: `scenarios/run.sh a22`, then `scenarios/run.sh b19`
- Route: Services
- See: checkout `tokio.runtime.*` spikes and catalog `jvm.*`/`process.*` lanes.
- Proves: runtime metrics explain latency without leaving the service page.

## 7. DB Spans And Pool Exhaustion

- Command: after Parallax/playground plan 048.
- Route: Traces, Services
- See: DB spans, pool saturation, and correlated latency.
- Proves: storage bottlenecks become trace-visible.

## 8. GraphQL Field Shapes

- Command: after plan 047.
- Route: Trace detail
- See: GraphQL resolver and field-tree shapes.
- Proves: expensive fields can be separated from cheap request envelopes.

## 9. RUM Journey

- Command: `scenarios/run.sh a28`.
- Route: Traces, Services
- See: browser route/user-step spans, `session.id`, web vital spans, a browser
  exception stitched to checkout, and the `?nopropagate=1` broken-continuation
  gap.
- Proves: frontend symptoms can connect to backend causes, and missing
  propagation is visible as an evidence gap.

## 10. Telemetry Quality

- Command: `scenarios/run.sh b22`
- Route: Traces, Logs
- See: roughly 10 percent of checkout traces but full request logs.
- Proves: sampled-out traces are an evidence gap, not missing traffic.

- Command: `scenarios/run.sh b23`
- Route: Logs
- See: `orphan diagnostic without trace context` with no trace chip.
- Proves: uncorrelated logs are visible and teachable.

- Command: `scenarios/run.sh b17b`
- Route: Runs
- See: cron ok/fail/stuck runs, one missing slot, duplicate invocation id.
- Proves: scheduled work needs absence and duplication semantics.

- Command: `scenarios/run.sh a9`
- Route: Logs; Field Explorer after plan 046.
- See: `app_screen_name=workspace-select` dominates the spike window.
- Proves: structured log fields can explain sudden cohort spikes.

## 11. Run Story

- Command: `scenarios/run.sh a12`
- Route: Runs
- See: short-lived CLI run with checkout story and exit code.
- Proves: one-shot tools can be inspected as first-class telemetry.
