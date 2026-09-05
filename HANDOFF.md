# Handoff

Branch: `fix/current-verification-readiness`

This branch contains the large commerce-telemetry playground migration from
the pasted DOD at:
`/Users/donbeave/.codex-chainargos2/attachments/fccc78a4-9bae-4704-9da1-739f2046b860/pasted-text-1.txt`.

## State at pause

- All current worktree changes are being committed and pushed together.
- Disposable Compose stacks were cleaned up.
- The local Parallax server was stopped.
- This is a second pause after a resumed implementation pass. Delegated
  workers were terminated after their final writes settled; review and test
  their changes before treating them as integrated.
- Latest local edits: docs now describe same-origin GraphQL checkout; Storefront
  readiness probes Checkout `/readyz`; web `/healthz` probes Storefront
  readiness; normal Storefront/web catalog queries no longer request the
  deliberate `reviewsSlow` N+1 field.
- Pricing and Orders inbound W3C context now remains current across async work;
  focused tests passed.
- The CLI verifier received a late auth-separation, ClickHouse/Rabbit event-key,
  analytics event-id, checkout-root, and tracestate-bound patch immediately
  after the main commit. It is included in the follow-up commit below but was
  not re-tested before this pause.

Since resuming, additional patches landed:

- Checkout outbox claiming is atomic and transient retries stop at three
  attempts; the worker reported 23 checkout tests and strict Clippy passing.
- Pricing now carries absolute quote expiry and price-list metadata; inspect
  whether the durable price-list/snapshot implementation is complete.
- Storefront now maps segment, tier, region, priority, pricing strategy,
  promotion, and payment method into one validated context and GraphQL error
  extensions; inspect and test the contract.
- CLI verifier changes add separate Parallax/Storefront request paths,
  event-identity checks, browser topology coverage, and W3C bounds. A focused
  worker reported 77 CLI tests, but the final landed hunks still need an
  integrated rerun.
- Browser/scenario changes add explicit Playwright mode and fail-closed checks;
  inspect the mock server and affected scripts before relying on them.

## Last proven gates

Before the pause, the managed verifier passed with cache telemetry skipped,
including clean migration rerun, deterministic PostgreSQL seed, normal and
failure commerce journeys, Rabbit/Redis/ClickHouse outage recovery, browser
commerce, feature flip, and readiness failure/recovery. Rust, Java, web,
Buf, shell, scenario-map, Compose-config, diff-whitespace, and forbidden
SQLx/Kafka/Redpanda scans also passed in that run.

The subsequent required local-Parallax verifier did not reach runtime proof: it
stopped around migration rerun with a shell `command not found` report at line
630 while delegated agents were concurrently editing verifier files. Re-run
from a clean tree after reviewing the verifier.

## Known remaining review items

Fresh read-only audits identified these items; they were not completed before
this handoff:

1. Review and test the newly landed outbox, pricing, Storefront, CLI, browser,
   and scenario patches; resolve compile, behavior, and contract fallout.
2. Verify the full pricing invariant: one snapshot for version and line reads,
   durable price-list/segment/tier selection, absolute expiry, and rejection of
   expired quotes at consumption.
3. Run browser mock/Compose E2E and affected fail-closed scenarios; validate
   actual stream messages and required Parallax/MCP/webhook evidence.
4. Strengthen live payment integration/failure coverage where required by the
   DOD.
5. Re-run the full local live-Parallax verifier and a fresh max-model review
   after fixes. Do not call the DOD complete until all gates pass.

## Restart prompt

Read this file, the pasted DOD, and `AGENTS.md`. Continue on the current branch.
Preserve all existing changes. Use subagents only with
`model: gpt-5.6-luna` and `reasoning_effort: max`. Start by checking the
verifier line-630 failure and the six review items above; then implement,
test, run the full local Parallax proof, perform a fresh read-only review, and
repeat gates until proven.
