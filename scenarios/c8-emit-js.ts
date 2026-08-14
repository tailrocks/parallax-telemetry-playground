// Real @sentry/node envelope for c8 (same Sentry JS SDK family as the
// TanStack Start browser app; Node transport works under Bun).
// Run from web/: SENTRY_DSN=... bun ../scenarios/c8-emit-js.ts
import * as Sentry from "@sentry/node"

const dsn = process.env["SENTRY_DSN"]
if (!dsn) {
  throw new Error("SENTRY_DSN required")
}

Sentry.init({
  dsn,
  release: "c8-js-sdk",
  environment: "playground",
  tracesSampleRate: 0,
})

Sentry.captureException(new Error("c8-js-sdk PaymentError"))
await Sentry.flush(5000)
await Sentry.close()
console.log("c8-js-sdk flushed")
