import {
  context,
  ROOT_CONTEXT,
  SpanKind,
  SpanStatusCode,
  trace,
  type Context,
  type Span,
  type Tracer,
} from "@opentelemetry/api";
import {
  CompositePropagator,
  W3CBaggagePropagator,
  W3CTraceContextPropagator,
} from "@opentelemetry/core";
import { OTLPTraceExporter } from "@opentelemetry/exporter-trace-otlp-proto";
import { resourceFromAttributes } from "@opentelemetry/resources";
import {
  BatchSpanProcessor,
  NodeTracerProvider,
} from "@opentelemetry/sdk-trace-node";
import {
  sanitizePropagationHeaders,
  type PropagationHeaders,
} from "./traceparent";
import { safeWebError } from "./error-contract";

const SERVER_TRACER_NAME = "playground.web.ssr";
const SERVER_SERVICE_NAME = "playground-web";
const SERVER_PROPAGATOR = new CompositePropagator({
  propagators: [
    new W3CTraceContextPropagator(),
    new W3CBaggagePropagator(),
  ],
});

let provider: NodeTracerProvider | undefined;
let tracer: Tracer | undefined;

/**
 * Install the real Node provider once. The server always creates/export spans;
 * an unavailable collector only affects export, never trace correctness.
 */
export function serverTracer(): Tracer {
  if (tracer !== undefined) return tracer;

  provider = new NodeTracerProvider({
    resource: resourceFromAttributes({
      "service.name": SERVER_SERVICE_NAME,
      "service.namespace": "parallax",
      "service.version": process.env["RELEASE"] ?? "dev",
      "deployment.environment.name": process.env["PARALLAX_ENV"] ?? "playground",
      "vcs.ref.head.revision": process.env["GIT_SHA"] ?? "local",
    }),
    spanProcessors: [
      new BatchSpanProcessor(
        new OTLPTraceExporter({
          url: traceEndpoint(),
          timeoutMillis: 5_000,
        }),
        { scheduledDelayMillis: 1_000, exportTimeoutMillis: 5_000 },
      ),
    ],
  });
  provider.register({
    propagator: SERVER_PROPAGATOR,
  });
  tracer = provider.getTracer(SERVER_TRACER_NAME);
  return tracer;
}

function traceEndpoint(): string {
  const explicit = firstNonEmpty([
    process.env["OTEL_EXPORTER_OTLP_TRACES_ENDPOINT"],
    process.env["PARALLAX_OTLP_HTTP_TRACES_ENDPOINT"],
  ]);
  if (explicit !== undefined) return explicit;

  const base =
    firstNonEmpty([
      process.env["ROTEL_OTLP_HTTP_ENDPOINT"],
      process.env["OTEL_EXPORTER_OTLP_ENDPOINT"],
    ]) ?? "http://localhost:4318";
  return /\/v1\/traces$/i.test(base)
    ? base
    : `${base.replace(/\/+$/, "")}/v1/traces`;
}

function firstNonEmpty(values: readonly (string | undefined)[]): string | undefined {
  return values
    .map((value) => value?.trim())
    .find((value): value is string => value !== undefined && value.length > 0);
}

/** Extract only validated W3C context and bounded business baggage. */
export function inboundServerContext(request: Request): Context {
  const headers: PropagationHeaders = {
    traceparent: request.headers.get("traceparent") ?? undefined,
    tracestate: request.headers.get("tracestate") ?? undefined,
    baggage: request.headers.get("baggage") ?? undefined,
  };
  const carrier = sanitizePropagationHeaders(headers);
  const values: Record<string, string> = {};
  for (const [key, value] of Object.entries(carrier)) {
    if (value !== undefined) values[key] = value;
  }
  return SERVER_PROPAGATOR.extract(ROOT_CONTEXT, values, {
    get: (current, key) => current[key.toLowerCase()],
    keys: (current) => Object.keys(current),
  });
}

/**
 * Trace one incoming SSR request. The returned context contains the genuine
 * server span and is used to bootstrap the browser document.
 */
export async function traceServerRequest(
  request: Request,
  handler: () => Promise<Response>,
): Promise<Readonly<{ response: Response; context: Context }>> {
  const parent = inboundServerContext(request);
  const span = serverTracer().startSpan(
    "web.ssr.request",
    {
      kind: SpanKind.SERVER,
      attributes: {
        "http.request.method": request.method,
        "url.path": new URL(request.url).pathname,
        "server.address": new URL(request.url).hostname,
      },
    },
    parent,
  );
  const active = trace.setSpan(parent, span);

  try {
    const response = await context.with(active, handler);
    span.setAttribute("http.response.status_code", response.status);
    if (response.status >= 500) {
      span.setStatus({ code: SpanStatusCode.ERROR, message: `HTTP ${response.status}` });
    }
    return { response, context: active };
  } catch (error: unknown) {
    recordServerException(span, error);
    throw error;
  } finally {
    span.end();
  }
}

/** Inject the genuine SSR span plus inherited tracestate/baggage. */
export function propagationHeaders(contextToInject: Context): PropagationHeaders {
  const carrier: Record<string, string> = {};
  SERVER_PROPAGATOR.inject(contextToInject, carrier, {
    set: (current, key, value) => {
      current[key] = value;
    },
  });
  return sanitizePropagationHeaders(carrier);
}

/** Best-effort shutdown hook for tests and graceful process termination. */
export async function shutdownServerTelemetry(): Promise<void> {
  const current = provider;
  provider = undefined;
  tracer = undefined;
  await current?.shutdown();
}

function recordServerException(span: Span, error: unknown): void {
  const normalized = safeWebError(error);
  span.recordException(normalized);
  span.setStatus({ code: SpanStatusCode.ERROR, message: normalized.message });
}
