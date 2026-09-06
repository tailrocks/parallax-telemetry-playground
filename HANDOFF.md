# Handoff

Branch: `fix/current-verification-readiness`

Source DOD:
`/Users/donbeave/.codex-chainargos2/attachments/fccc78a4-9bae-4704-9da1-739f2046b860/pasted-text-1.txt`

## State at pause

- DOD is not complete.
- `49bda81` is the final source integration commit and is pushed to
  `origin/fix/current-verification-readiness`.
- Recent pushed commits also include `44f1f48` (Checkout waits for healthy
  RabbitMQ), `dbf036e` (CLI Parallax failure/browser assertions), and
  `54a1a6a` (verification handoff/contracts). Earlier commerce, corpus,
  baggage, pricing, and browser fixes are in the preceding history.
- The tree was clean before this handoff edit. No source changes were left
  intentionally uncommitted.
- The Rust CLI plus `mise-scenarios.toml` owns scenario and verification
  dispatch. Java quality tasks invoke the Gradle wrapper JAR directly;
  service-local `gradlew` and `gradlew.bat` scripts are removed.
- `corpus:all` dispatches all 89 proofs exactly once: 61 A/B/C proofs plus 28
  corner proofs. `check:scenarios` validates the catalog and legacy-wrapper
  scan.

## Proven in the final integration tree

- `git diff --check`: pass before commit.
- `cargo fmt --all -- --check`: pass.
- `cargo check --locked --workspace --all-targets --all-features`: pass.
- `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings`:
  pass.
- `cargo test --locked -p playground-cli`: 95 passed.
- `mise run check:scenarios`: pass; 89 semantic tasks plus `corpus:all`.
- `docker compose -f deploy/docker-compose.yml config --quiet`: pass.
- `cd web && bun run test`: 33 passed.
- `cd web && bun run build`: pass; Vite build and strict TypeScript check pass.
- Earlier on the same integrated working-tree source, the real-Compose browser
  E2E passed. It was not rerun after the final commit.

## Exact unresolved DOD

1. Full Parallax-backed payment/inventory failure runtime proof and the
   dedicated browser-to-backend Parallax causal assertion are not proven in
   this final checkpoint. The CLI assertions exist and unit/compile gates pass;
   runtime proof still requires a live Parallax session.
2. Full clean-volume Compose boot, migration idempotence, deterministic
   failure runs, topology/trace run, durable PostgreSQL/RabbitMQ/ClickHouse
   evidence, and the final forbidden-implementation scan were not rerun in
   this short integration checkpoint.
3. `VERIFY_MANAGE_STACK=1 mise run verify:commerce_stack` now uses an isolated
   project, bounded readiness, and cleanup, but that managed runtime path was
   not rerun here.
4. `demo:stack` and `demo:fresh` still use direct detached Compose startup
   without the verifier's readiness/cleanup contract; this remains separate
   hardening work.
5. Parallax remains an external prerequisite, not a Compose service. Start it
   before runtime proof at `127.0.0.1:4000` with OTLP on `127.0.0.1:4317`.

## Restart prompt

Read this file, the pasted DOD, and `AGENTS.md`. Continue from pushed commit
`49bda81`. Preserve the branch. Use subagents only with
`model: gpt-5.6-luna` and `reasoning_effort: max`.

```bash
git status --short --branch
git log -5 --oneline --decorate
mise run check:scenarios
parallax serve
VERIFY_MANAGE_STACK=1 mise run verify:commerce_stack
mise run verify:commerce_trace
mise run failures:payment_latency
mise run failures:inventory
cd web && bun run e2e:compose
```

Then close every unresolved item above. Do not infer DOD completion from the
stack verifier, CLI tests, or browser E2E alone. Finish with all quality gates,
runtime evidence, fresh read-only review, a clean tree, and
`HEAD == origin/fix/current-verification-readiness`.
