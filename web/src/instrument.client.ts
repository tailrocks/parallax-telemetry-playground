// Browser telemetry: Sentry (RUM — replay, web vitals, feedback, source maps)
// **and** the portable OTel Web SDK (OTLP → same-origin /v1/traces proxy →
// Rotel → every backend). See spec §8 (Frontend). Called once from the client
// entry before hydration.
import * as Sentry from "@sentry/tanstackstart-react";
import { initOtel } from "./telemetry";

// API origins matched by Sentry tracePropagationTargets. OTel app fetches use
// tracedFetch() so they can inject trace context and session baggage explicitly.
const checkoutUrl = import.meta.env["VITE_CHECKOUT_URL"] ?? "http://localhost:8088";
const ordersUrl = import.meta.env["VITE_ORDERS_URL"] ?? "http://localhost:8092";
const apiTargets: (string | RegExp)[] = [/^\//, checkoutUrl, ordersUrl];

let started = false;

export function initBrowserTelemetry() {
  if (started || typeof document === "undefined") return;
  started = true;

  Sentry.init({
    dsn: import.meta.env["VITE_SENTRY_DSN"],
    environment: "playground",
    // Pin per run for lab repeatability (see spec §8 sampling note).
    tracesSampleRate: 1.0,
    replaysSessionSampleRate: 0.1,
    replaysOnErrorSampleRate: 1.0,
    // Share W3C traceparent so the Sentry transaction tree and the OTLP trace
    // carry the same trace_id (emission is opt-in; §6).
    tracePropagationTargets: apiTargets,
    // Sentry logs (browser OTel logs are still experimental — §8).
    enableLogs: true,
    integrations: [
      Sentry.replayIntegration(),
      // LCP/CLS/INP/FCP/TTFB web vitals + browser distributed tracing.
      Sentry.browserTracingIntegration(),
      Sentry.feedbackIntegration({ colorScheme: "system" }),
      Sentry.consoleLoggingIntegration(),
    ],
  });

  // Portable path: OTel WebTracerProvider → /v1/traces proxy. Fetch +
  // document-load + user-interaction instrumentation propagate traceparent to
  // the backend so the browser span joins the same distributed trace.
  initOtel();
}
