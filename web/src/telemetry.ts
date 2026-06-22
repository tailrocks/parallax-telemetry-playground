// Browser OpenTelemetry: WebTracerProvider exporting OTLP/HTTP to a same-origin
// `/v1/traces` proxy (→ Rotel), with fetch + document-load instrumentation that
// propagate W3C traceparent to the backend. ZoneContextManager requires the
// ES2015 build target (see tsconfig) so async context links correctly.
import { WebTracerProvider, BatchSpanProcessor } from "@opentelemetry/sdk-trace-web";
import { OTLPTraceExporter } from "@opentelemetry/exporter-trace-otlp-http";
import { ZoneContextManager } from "@opentelemetry/context-zone";
import { registerInstrumentations } from "@opentelemetry/instrumentation";
import { FetchInstrumentation } from "@opentelemetry/instrumentation-fetch";
import { DocumentLoadInstrumentation } from "@opentelemetry/instrumentation-document-load";

export function initOtel(apiOrigins: (string | RegExp)[]) {
  const provider = new WebTracerProvider({
    spanProcessors: [new BatchSpanProcessor(new OTLPTraceExporter({ url: "/v1/traces" }))],
  });
  provider.register({ contextManager: new ZoneContextManager() });
  registerInstrumentations({
    instrumentations: [
      new DocumentLoadInstrumentation(),
      new FetchInstrumentation({ propagateTraceHeaderCorsUrls: apiOrigins }),
    ],
  });
}
