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
  // Session envelopes are type=session; Parallax only derives issues from
  // type=event. Disable sessions so flush sends the exception item.
  autoSessionTracking: false,
  sendClientReports: false,
})

Sentry.withScope((scope) => {
  scope.setFingerprint(["c8-js-sdk"])
  scope.setTag("c8.sdk", "sentry.javascript")
  Sentry.captureException(new Error("c8-js-sdk PaymentError"))
})
const flushed = await Sentry.flush(8000)
console.log(`c8-js-sdk flushed=${flushed}`)
await Sentry.close(2000)
