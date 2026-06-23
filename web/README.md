# web (TanStack Start, TypeScript)

Frontend + RUM for the playground. A real **TanStack Start** app (file-based
routing, Nitro server) — not a scaffold. Bun-only per the Parallax repo.

Two telemetry paths (spec §8):

- **Portable OTel** — `@opentelemetry/sdk-trace-web` (`src/telemetry.ts`) exports
  OTLP/HTTP to a **same-origin `/v1/traces` proxy** (`src/routes/v1.traces.ts`, a
  Start server route) that forwards to Rotel server-side. Same-origin avoids
  collector CORS, hides the endpoint, and is where ingest auth would live.
  `fetch` + `document-load` + `user-interaction` instrumentation propagate W3C
  `traceparent` to the `checkout` backend, stitching browser → backend.
- **Sentry RUM** — `@sentry/tanstackstart-react` (`src/instrument.client.ts`):
  session replay, web vitals (browser tracing), user feedback, console-logs,
  source maps (Debug IDs via `@sentry/vite-plugin`, gated on `SENTRY_AUTH_TOKEN`).

SSR → browser handoff: `src/routes/__root.tsx` emits `<meta name="traceparent">`
during SSR; the OTel document-load instrumentation reads it so the first-paint
span joins the trace (initial nav has no fetch to inject a header into).

Telemetry inits once in the custom client entry (`src/client.tsx`) before
hydration.

## Run

```bash
bun install
bun run build          # vite build (client + SSR + Nitro) + tsc --noEmit
bun run dev            # dev server on :5173
bun run start          # node .output/server/index.mjs (prod)
```

## Env

| Var | Default | Purpose |
|---|---|---|
| `VITE_SENTRY_DSN` | — | Sentry browser DSN (envelope path) |
| `VITE_CHECKOUT_URL` | `http://localhost:8088` | checkout backend base (trace-propagation target) |
| `VITE_RELEASE` | `dev` | `service.version` / release id |
| `ROTEL_OTLP_HTTP_ENDPOINT` | `http://localhost:4318` | Rotel HTTP receiver the `/v1/traces` proxy forwards to (server-side; `http://rotel:4318` in compose) |
| `SENTRY_AUTH_TOKEN` / `SENTRY_ORG` / `SENTRY_PROJECT` | — | enable source-map upload at build |

Spec: `docs/research/validation/telemetry-playground-sample-project.md` §8.
