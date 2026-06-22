// Browser telemetry: OTel Web SDK (portable, → same-origin /v1/traces proxy →
// Rotel) + Sentry (RUM: replay, web vitals, source maps). Import FIRST in the
// client entry. See docs §8 (Frontend). Scaffold — wire the provider per the doc.
import * as Sentry from "@sentry/tanstackstart-react";

Sentry.init({
  dsn: import.meta.env.VITE_SENTRY_DSN,
  environment: "playground",
  tracesSampleRate: 1.0,
  replaysSessionSampleRate: 0.1,
  replaysOnErrorSampleRate: 1.0,
  integrations: [Sentry.replayIntegration(), Sentry.browserTracingIntegration()],
  // propagateTraceparent is opt-in — enable to share trace_id with the OTLP backends.
});
// OTel WebTracerProvider + fetch/document-load instrumentation: see doc §8.
