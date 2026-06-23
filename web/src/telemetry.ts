// Browser OpenTelemetry: WebTracerProvider exporting OTLP/HTTP to a same-origin
// `/v1/traces` proxy (→ Rotel → all backends). Fetch + document-load +
// user-interaction instrumentation propagate W3C traceparent to the backend so
// the browser span joins the same distributed trace. ZoneContextManager needs
// an ES2015+ build target so async context links correctly (see vite/tsconfig).
import {
  WebTracerProvider,
  BatchSpanProcessor,
} from "@opentelemetry/sdk-trace-web";
import { OTLPTraceExporter } from "@opentelemetry/exporter-trace-otlp-http";
import { ZoneContextManager } from "@opentelemetry/context-zone";
import { W3CTraceContextPropagator } from "@opentelemetry/core";
import { resourceFromAttributes } from "@opentelemetry/resources";
import {
  ATTR_SERVICE_NAME,
  ATTR_SERVICE_VERSION,
} from "@opentelemetry/semantic-conventions";
import { registerInstrumentations } from "@opentelemetry/instrumentation";
import { FetchInstrumentation } from "@opentelemetry/instrumentation-fetch";
import { DocumentLoadInstrumentation } from "@opentelemetry/instrumentation-document-load";
import { UserInteractionInstrumentation } from "@opentelemetry/instrumentation-user-interaction";

export function initOtel(apiOrigins: (string | RegExp)[]) {
  const provider = new WebTracerProvider({
    resource: resourceFromAttributes({
      [ATTR_SERVICE_NAME]: "web",
      [ATTR_SERVICE_VERSION]: import.meta.env.VITE_RELEASE ?? "dev",
      "deployment.environment.name": "playground",
    }),
    spanProcessors: [
      new BatchSpanProcessor(new OTLPTraceExporter({ url: "/v1/traces" })),
    ],
  });
  provider.register({
    contextManager: new ZoneContextManager(),
    propagator: new W3CTraceContextPropagator(),
  });
  registerInstrumentations({
    instrumentations: [
      // Reads <meta name="traceparent"> emitted during SSR (§6 handoff).
      new DocumentLoadInstrumentation(),
      new FetchInstrumentation({ propagateTraceHeaderCorsUrls: apiOrigins }),
      new UserInteractionInstrumentation(),
    ],
  });
}
