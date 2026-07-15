// Browser OpenTelemetry: WebTracerProvider exporting OTLP/HTTP to a same-origin
// `/v1/traces` proxy (→ Rotel → all backends). Fetch + document-load +
// user-interaction instrumentation propagate W3C traceparent to the backend so
// the browser span joins the same distributed trace. ZoneContextManager needs
// an ES2015+ build target so async context links correctly (see vite/tsconfig).
import {
  WebTracerProvider,
  BatchSpanProcessor,
} from "@opentelemetry/sdk-trace-web";
import { OTLPTraceExporter } from "@opentelemetry/exporter-trace-otlp-proto";
import { logs, SeverityNumber, type Logger } from "@opentelemetry/api-logs";
import { LoggerProvider } from "@opentelemetry/sdk-logs";
import { BatchLogRecordProcessor } from "@opentelemetry/sdk-logs";
import { OTLPLogExporter } from "@opentelemetry/exporter-logs-otlp-proto";
import { ZoneContextManager } from "@opentelemetry/context-zone";
import {
  CompositePropagator,
  W3CBaggagePropagator,
  W3CTraceContextPropagator,
} from "@opentelemetry/core";
import { resourceFromAttributes } from "@opentelemetry/resources";
import {
  ATTR_SERVICE_NAME,
  ATTR_SERVICE_VERSION,
} from "@opentelemetry/semantic-conventions";
import {
  SpanStatusCode,
  context,
  propagation,
  trace,
  type Context,
  type Span,
} from "@opentelemetry/api";
import { registerInstrumentations } from "@opentelemetry/instrumentation";
import { FetchInstrumentation } from "@opentelemetry/instrumentation-fetch";
import { DocumentLoadInstrumentation } from "@opentelemetry/instrumentation-document-load";
import { UserInteractionInstrumentation } from "@opentelemetry/instrumentation-user-interaction";
import {
  APP_SCREEN_NAME,
  BROWSER_WEB_VITAL,
  DEFAULT_ENVIRONMENT,
  DEPLOYMENT_ENVIRONMENT_NAME,
  ERROR_TYPE,
  EVENT_NAME,
  SESSION_ID,
  URL_PATH,
  WEB_VITAL_DELTA,
  WEB_VITAL_ID,
  WEB_VITAL_NAME,
  WEB_VITAL_NAVIGATION_TYPE,
  WEB_VITAL_RATING,
  WEB_VITAL_VALUE,
} from "./semconv";

export type RumAttributeValue = string | number | boolean;
export type RumAttributes = Record<string, RumAttributeValue | undefined>;

const SESSION_STORAGE_KEY = "parallax.playground.session_id";
const WEB_TRACER_NAME = "playground.web.rum";

let sessionId: string | undefined;
let providerRef: WebTracerProvider | undefined;
let loggerProviderRef: LoggerProvider | undefined;
let eventLogger: Logger | undefined;
let flushListenersAttached = false;
let vitalsStarted = false;
let currentStepContext: Context | undefined;

export function initOtel() {
  sessionId = getSessionId();
  const resource = resourceFromAttributes({
    [ATTR_SERVICE_NAME]: "web",
    [ATTR_SERVICE_VERSION]: import.meta.env["VITE_RELEASE"] ?? "dev",
    [DEPLOYMENT_ENVIRONMENT_NAME]:
      import.meta.env["VITE_PARALLAX_ENV"] ?? DEFAULT_ENVIRONMENT,
    [SESSION_ID]: sessionId,
  });
  const provider = new WebTracerProvider({
    resource,
    spanProcessors: [
      new BatchSpanProcessor(new OTLPTraceExporter({ url: "/v1/traces" })),
    ],
  });
  providerRef = provider;
  const loggerProvider = new LoggerProvider({
    resource,
    processors: [
      new BatchLogRecordProcessor({
        exporter: new OTLPLogExporter({ url: "/v1/logs" }),
      }),
    ],
  });
  loggerProviderRef = loggerProvider;
  logs.setGlobalLoggerProvider(loggerProvider);
  eventLogger = loggerProvider.getLogger("playground.web.events");
  provider.register({
    contextManager: new ZoneContextManager(),
    propagator: new CompositePropagator({
      propagators: [
        new W3CTraceContextPropagator(),
        new W3CBaggagePropagator(),
      ],
    }),
  });
  registerInstrumentations({
    instrumentations: [
      // Reads <meta name="traceparent"> emitted during SSR (§6 handoff).
      new DocumentLoadInstrumentation(),
      new FetchInstrumentation(),
      new UserInteractionInstrumentation(),
    ],
  });
  void startWebVitals().finally(attachFlushListeners);
  queueMicrotask(() => trackScreen(window.location.pathname));
}

export function getSessionId(): string {
  if (sessionId) return sessionId;
  if (typeof window === "undefined") return "server";

  try {
    const stored = window.sessionStorage.getItem(SESSION_STORAGE_KEY);
    if (stored) {
      sessionId = stored;
      return stored;
    }
    const minted = window.crypto.randomUUID();
    window.sessionStorage.setItem(SESSION_STORAGE_KEY, minted);
    sessionId = minted;
    return minted;
  } catch {
    sessionId = window.crypto.randomUUID();
    return sessionId;
  }
}

export function trackScreen(pathname: string) {
  trackStep(APP_SCREEN_NAME, {
    [APP_SCREEN_NAME]: screenName(pathname),
    [URL_PATH]: pathname,
  });
}

export function trackStep(name: string, attributes: RumAttributes = {}) {
  const span = startRumSpan(name, attributes);
  span.addEvent(name, cleanAttributes(attributes));
  span.end();
}

export async function runTracedStep<T>(
  name: string,
  attributes: RumAttributes,
  fn: () => Promise<T>,
): Promise<T> {
  const span = startRumSpan(name, attributes);
  span.addEvent(name, cleanAttributes(attributes));
  const active = trace.setSpan(sessionContext(), span);
  const previousStepContext = currentStepContext;
  currentStepContext = active;
  try {
    return await context.with(active, fn);
  } catch (err) {
    recordException(span, err);
    throw err;
  } finally {
    currentStepContext = previousStepContext;
    span.end();
  }
}

export async function tracedFetch(
  input: RequestInfo | URL,
  init: RequestInit = {},
): Promise<Response> {
  const headers = new Headers(init.headers);
  propagation.inject(sessionContext(currentStepContext), headers, {
    set(carrier, key, value) {
      carrier.set(key, value);
    },
  });
  return fetch(input, { ...init, headers });
}

export function emitTypedEvent(name: string, attributes: RumAttributes = {}) {
  const logger = eventLogger ?? logs.getLogger("playground.web.events");
  if (!logger.enabled({ severityNumber: SeverityNumber.INFO, eventName: name })) {
    return;
  }
  logger.emit({
    eventName: name,
    severityNumber: SeverityNumber.INFO,
    severityText: "INFO",
    body: name,
    attributes: cleanAttributes({
      [EVENT_NAME]: name,
      ...attributes,
    }),
    context: sessionContext(currentStepContext),
  });
}

function startRumSpan(name: string, attributes: RumAttributes): Span {
  return trace.getTracer(WEB_TRACER_NAME).startSpan(
    name,
    { attributes: cleanAttributes(attributes) },
    sessionContext(),
  );
}

function recordException(span: Span, err: unknown) {
  const error =
    err instanceof Error ? err : new Error(typeof err === "string" ? err : "unknown error");
  span.recordException(error);
  span.setStatus({ code: SpanStatusCode.ERROR, message: error.message });
  span.setAttribute(ERROR_TYPE, error.name);
}

function sessionContext(base: Context = context.active()) {
  const baggage = propagation.createBaggage({
    [SESSION_ID]: { value: getSessionId() },
  });
  return propagation.setBaggage(base, baggage);
}

function cleanAttributes(attributes: RumAttributes) {
  return Object.fromEntries(
    Object.entries(attributes).filter((entry): entry is [string, RumAttributeValue] => {
      const value = entry[1];
      return value !== undefined;
    }),
  );
}

function screenName(pathname: string) {
  if (pathname === "/") return "home";
  if (pathname.startsWith("/checkout")) return "checkout";
  if (pathname.startsWith("/orders")) return "orders";
  return "unknown";
}

function attachFlushListeners() {
  if (flushListenersAttached || typeof document === "undefined") return;
  flushListenersAttached = true;

  const flush = () => {
    void providerRef?.forceFlush().catch((err) => {
      console.debug("[otel] forceFlush failed", err);
    });
    void loggerProviderRef?.forceFlush().catch((err) => {
      console.debug("[otel] log forceFlush failed", err);
    });
  };
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "hidden") flush();
  });
  window.addEventListener("pagehide", flush);
}

async function startWebVitals() {
  if (vitalsStarted || typeof window === "undefined") return;
  vitalsStarted = true;
  const { onCLS, onFCP, onINP, onLCP, onTTFB } = await import("web-vitals");
  const report = (metric: {
    name: string;
    value: number;
    rating: string;
    id: string;
    delta: number;
    navigationType?: string;
  }) => {
    const attrs = {
      [WEB_VITAL_NAME]: metric.name,
      [WEB_VITAL_VALUE]: metric.value,
      [WEB_VITAL_RATING]: metric.rating,
      [WEB_VITAL_ID]: metric.id,
      [WEB_VITAL_DELTA]: metric.delta,
      [WEB_VITAL_NAVIGATION_TYPE]: metric.navigationType,
      [APP_SCREEN_NAME]: screenName(window.location.pathname),
    };
    // Development-status browser convention used by the lab contract.
    const cleaned = cleanAttributes(attrs);
    const span = startRumSpan(BROWSER_WEB_VITAL, cleaned);
    for (const [key, value] of Object.entries(cleaned)) {
      span.setAttribute(key, value);
    }
    span.addEvent(BROWSER_WEB_VITAL, cleaned);
    span.end();
  };
  onCLS(report);
  onFCP(report);
  onINP(report);
  onLCP(report);
  onTTFB(report);
}
