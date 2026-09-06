# Handoff

Branch: `fix/current-verification-readiness`

This branch contains the large commerce-telemetry playground migration from
the pasted DOD at:
`/Users/donbeave/.codex-chainargos2/attachments/fccc78a4-9bae-4704-9da1-739f2046b860/pasted-text-1.txt`.

## State at pause

- All current worktree changes are being committed and pushed together in this
  checkpoint. The DOD is not complete.
- Disposable Compose stacks were cleaned up.
- The local Parallax server was stopped.
- The checkpoint replaces the deleted shell scenario/verifier dispatch with a
  Rust CLI plus `mise-scenarios.toml`; it also removes per-service Gradle
  wrappers and shell migration/schema helpers.
- Static inventory found 107 Mise tasks and 89 public scenario names, with a
  task entry for every public scenario name. This is catalog evidence only.
- The latest service/docs edits include the same-origin GraphQL checkout,
  Storefront/Checkout readiness chain, catalog query cleanup, W3C async-context
  fixes, bounded outbox retries, pricing quote metadata/expiry, Storefront
  business-context validation, and fail-closed browser/scenario paths.
- Fresh delegated reviewers were stopped after reporting; no reviewer edits
  remain outstanding. Review and test this checkpoint before treating it as
  integrated.

## Last proven gates

Before the Rust task replacement, the legacy managed verifier passed with cache
telemetry skipped, including clean migration rerun, deterministic PostgreSQL
seed, normal and failure commerce journeys, Rabbit/Redis/ClickHouse outage
recovery, browser commerce, feature flip, and readiness failure/recovery. Rust, Java, web,
Buf, scenario catalog, Compose-config, diff-whitespace, and forbidden
SQLx/Kafka/Redpanda scans also passed in that run.
Those results belong to the deleted shell verifier and are not proof for this
checkpoint. No full current-state DOD gate was run before this pause.

The subsequent required local-Parallax verifier did not reach runtime proof: it
stopped around migration rerun with a shell `command not found` report at line
630 while delegated agents were concurrently editing verifier files. Re-run
from a clean tree after reviewing the verifier.

## Known remaining review items

Fresh read-only audit of this checkpoint found:

1. `verify:commerce_stack` validates Compose configuration plus only five
   health endpoints; it omits most services and infrastructure readiness.
2. `test:observable --acceptance` is fail-open and runs no acceptance checks.
3. Web observable testing is mock Playwright only; it lacks production-build
   and real-Compose browser proof.
4. `check:typescript` only typechecks; it omits web tests and production build.
5. Scenario validation is syntactic. It does not prove distinct behavior,
   runtime assertions, stream messages, or Parallax/MCP/webhook evidence.
6. Several public tasks alias one implementation, including
   `messaging:java_fulfillment_replay`/`messaging:seeded_order_replay` and
   `grpc:pricing_stream`/`protocols:grpc_stream`.
7. Review and test the outbox, pricing, Storefront, CLI, browser, and service
   patches. Verify the single-snapshot pricing invariant, durable
   price-list/segment/tier selection, absolute expiry, and expired-quote
   rejection.
8. Run browser mock/Compose E2E, live payment integration/failure coverage,
   the full local live-Parallax verifier, and a fresh max-model review.
9. Search the final diff and tree for forbidden SQLx/Kafka/Redpanda/fake/
   placeholder/TODO paths, then repeat all DOD gates. Do not call the DOD
   complete until every required gate passes.

## Restart prompt

Read this file, the pasted DOD, and `AGENTS.md`. Continue on the current branch
from the pushed checkpoint; preserve existing changes. Use subagents only with
`model: gpt-5.6-luna` and `reasoning_effort: max`. Start with `git status`,
`git log -1`, `mise run check:scenarios`, and the known review items above.
Then repair the verification gaps, test the changed contracts, run the full
local Compose and Parallax proof, perform a fresh read-only review, and repeat
all DOD gates until proven. The previous line-630 verifier failure is no
longer the only blocker; the current Rust/Mise gates must be strengthened and
validated first.
