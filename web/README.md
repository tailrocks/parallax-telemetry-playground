# web (TanStack Start, TypeScript) — scaffold

Frontend + RUM. OTel Web SDK (portable OTLP via a same-origin `/v1/traces` proxy
to Rotel) **and** `@sentry/tanstackstart-react` (replay, web vitals, source maps).
Bun-only per the Parallax repo. Finalize the provider wiring, server middleware
(`<meta traceparent>` SSR handoff), and the proxy per
docs/research/validation/telemetry-playground-sample-project.md §8.
