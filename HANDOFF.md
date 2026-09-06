# Handoff

Branch: `fix/current-verification-readiness`

This branch contains the commerce-telemetry playground migration from the
pasted DOD at:
`/Users/donbeave/.codex-chainargos2/attachments/fccc78a4-9bae-4704-9da1-739f2046b860/pasted-text-1.txt`.

## State at pause

- DOD is not complete.
- The current docs checkpoint records commits `20e929a`, `1f65a00`,
  `3558ba0`, `c535e91`, and `632cbf4`.
- The Rust CLI plus `mise-scenarios.toml` owns scenario and verification
  dispatch. The service-local `gradlew` and `gradlew.bat` scripts are removed;
  Java quality tasks invoke `gradle/wrapper/gradle-wrapper.jar` directly.
- `corpus:all` dispatches all 89 proofs exactly once: 61 A/B/C proofs plus 28
  corner proofs. `check:scenarios` validates the catalog and legacy-wrapper
  scan.
- This docs commit intentionally preserves unrelated dirty source changes in
  `cli/`, `deploy/`, `services/`, and `web/`. Do not reset or discard them.

## Latest proven gates

- `mise run verify:commerce_stack` passed: Compose configuration, all 16
  required running services healthy, all three required one-shot jobs exited
  successfully, ten configured HTTP readiness surfaces, and PostgreSQL,
  Redis, RabbitMQ, ClickHouse, flagd, Pricing gRPC, Payment HTTP/gRPC, and
  Notifications probes.
- `cd web && bun run e2e:compose` passed the canonical real-Compose browser
  journey, including strict W3C carrier checks, commerce flow, browser
  analytics, and refreshed durable orders projection.
- The stack verifier and browser E2E are separate gates. Neither proves the
  full Parallax failure corpus or every browser-to-backend causal assertion.

## Remaining gates

1. Parallax-backed failure scenarios and the dedicated browser causal/Parallax
   assertion work remain pending. Do not call DOD complete.
2. Re-run `mise run verify:commerce_trace` and the full failure/topology gate
   from a clean, integrated tree after the preserved source changes are
   reviewed.
3. Repeat the final DOD review: clean-volume migration/idempotence, all
   language quality gates, canonical journeys and deterministic failures,
   durable PostgreSQL/RabbitMQ/ClickHouse evidence, browser proof, forbidden
   implementation scan, and fresh max-model review.

## Restart prompt

Read this file, the pasted DOD, and `AGENTS.md`. Continue on the current branch
from the latest commit; preserve the dirty source files. Use subagents only with
`model: gpt-5.6-luna` and `reasoning_effort: max`. Start with:

```bash
git status --short
git log -1
mise run check:scenarios
mise run verify:commerce_stack
cd web && bun run e2e:compose
```

Then close the remaining Parallax failure/browser assertion gates, run the full
local Compose and Parallax proof, perform a fresh read-only review, and repeat
all DOD gates until proven. Do not infer DOD completion from the stack verifier
or browser E2E alone.
