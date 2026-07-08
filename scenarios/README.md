# Scenarios

Run `./scenarios/run.sh` for the catalog or `./scenarios/run.sh a1` for one
scenario. The table lists the existing scripts only; later scenario plans append
rows here and in `run.sh`.

| ID | Script | Drives | Check in Parallax UI |
|---|---|---|---|
| a1 | `a1-checkout.sh` | Checkout through pricing, inventory, and recommendation. | Traces: one checkout waterfall with pricing, inventory, and recommendation children. |
| a3 | `a3-async.sh` | Orders producer/consumer branch. | Trace detail: producer span with link to consumer trace. |
| a4 | `a4-reverse.sh` | Java fulfillment produces to Kafka, consumes, then calls Rust notifications. | Trace detail: Java async span link plus Java -> Rust hop. |
| a12 | `a12-cli-run.sh` | Short-lived Rust CLI checkout driver. | Runs: command row with exit code; run `cargo build` first. `parallax run start -- scenarios/a12-cli-run.sh` is optional when you want run-scoped resource attrs. |
| a13 | `a13-deploy-regression.sh` | Clean checkout, then failed checkout. | Issues: error spike while `RELEASE=v2`; release attribution lands in plan 042. |
| a18 | `a18-canary.sh` | Fake sensitive canary corpus in telemetry. | Issues/Logs: redaction of fake email/token/card/jwt fields. |
| b-async-chaos | `b-async-chaos.sh` | Consumer lag and poison message. | Services/Traces: lag span and dead-letter error branch. |
| b-chaos | `b-chaos.sh` | Payment failure and injected latency. | Issues/Services: checkout error grouping and slow-span rendering. |
| b-checkout-chaos | `b-checkout-chaos.sh` | Retry timeout and N+1 fan-out. | Traces: retry/timeout branch and N+1 waterfall. |
| b-degradation | `b-degradation.sh` | Partial degrade and clock skew. | Traces/Issues: degraded response and skewed span timing. |
| b17 | `b17-cron.sh` | Short-lived Rust CLI cron mode. | Runs: cron success/fail/stuck outcome; run `cargo build` first. `parallax run start -- scenarios/b17-cron.sh` is optional when you want run-scoped resource attrs. |
