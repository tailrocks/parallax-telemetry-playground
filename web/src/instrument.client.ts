// Browser telemetry: Sentry (RUM — replay, web vitals, feedback, source maps)
// **and** the portable OTel Web SDK (OTLP → same-origin /v1/traces proxy →
// Rotel → every backend). See spec §8 (Frontend). Called once from the client
// entry before hydration.
import * as Sentry from "@sentry/tanstackstart-react";
import type { Context } from "@opentelemetry/api";
import { initOtel } from "./telemetry";

// API origins matched by Sentry tracePropagationTargets. OTel app fetches use
// tracedFetch() so they can inject trace context and session baggage explicitly.
const storefrontUrl =
  import.meta.env["VITE_STOREFRONT_URL"] ?? "/__storefront/graphql";
const storefrontOrigin = (() => {
  try {
    return new URL(storefrontUrl).origin;
  } catch {
    return storefrontUrl;
  }
})();
const apiTargets: (string | RegExp)[] = [/^\//, storefrontOrigin];

let started = false;
let browserContext: Context | undefined;

export function initBrowserTelemetry(): Context | undefined {
  if (started || typeof document === "undefined") return browserContext;
  started = true;

  Sentry.init({
    dsn: import.meta.env["VITE_SENTRY_DSN"],
    environment: "playground",
    // Pin per run for lab repeatability (see spec §8 sampling note).
    tracesSampleRate: 1.0,
    replaysSessionSampleRate: 0.1,
    replaysOnErrorSampleRate: 1.0,
    // The SSR entry emits Sentry's `sentry-trace` meta parent alongside W3C
    // metadata. BrowserTracing reads it, so the pageload transaction is a
    // child of the real SSR span instead of a second trace root.
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
  browserContext = initOtel();
  return browserContext;
}
