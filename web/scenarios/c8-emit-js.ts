// Real @sentry/node envelope for c8 (same Sentry JS SDK family as the
// TanStack Start browser app; Node transport works under Bun).
// Run through `mise run sentry:envelopes`; the Rust task owns Rust, Java, and JavaScript emission.
import * as Sentry from "@sentry/node"

const dsn = process.env["SENTRY_DSN"]
if (!dsn) {
  throw new Error("SENTRY_DSN required")
}

// Official JS SDK 10.70 puts sentry_key in the query string (CORS). Parallax
// only accepts X-Sentry-Auth / Authorization (sentry_http.rs). Keep the real
// SDK envelope; add the header the ingest already understands.
const publicKey = new URL(dsn).username
if (!publicKey) {
  throw new Error("SENTRY_DSN missing public key")
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
  transportOptions: {
    headers: {
      "X-Sentry-Auth": `Sentry sentry_version=7, sentry_client=sentry.javascript.node/10.70.0, sentry_key=${publicKey}`,
    },
  },
})

Sentry.withScope((scope) => {
  scope.setFingerprint(["c8-js-sdk"])
  scope.setTag("c8.sdk", "sentry.javascript")
  Sentry.captureException(new Error("c8-js-sdk PaymentError"))
})
const flushed = await Sentry.flush(8000)
console.log(`c8-js-sdk flushed=${flushed}`)
await Sentry.close(2000)
