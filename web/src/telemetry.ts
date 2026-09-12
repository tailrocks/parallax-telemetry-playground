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
import * as Sentry from "@sentry/tanstackstart-react";
import {
  CompositePropagator,
  W3CBaggagePropagator,
  W3CTraceContextPropagator,
} from "@opentelemetry/core";
import {
  defaultResource,
  resourceFromAttributes,
} from "@opentelemetry/resources";
import {
  SpanStatusCode,
  ROOT_CONTEXT,
  context,
  isSpanContextValid,
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
import { webResourceAttributes } from "./resource";
import {
  boundedBaggageEntries,
  sanitizePropagationHeaders,
} from "./traceparent";
import {
  safeWebAttributes,
  safeWebError,
  safeWebOperation,
} from "./error-contract";
import {
  boundedGraphqlErrorDetails,
  type GraphqlPathPart,
} from "./graphql-errors";

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
let browserRootContext: Context | undefined;
const reportedHandledErrorKeys = new Set<string>();
const MAX_REPORTED_HANDLED_ERRORS = 128;

export function initOtel(): Context {
  sessionId = getSessionId();
  // Merge onto the SDK default resource: replacing it drops
  // telemetry.sdk.language=webjs, the generic signal observability tools use
  // to classify this service as a browser.
  const resource = defaultResource().merge(
    resourceFromAttributes(
      webResourceAttributes({
        release: import.meta.env["VITE_RELEASE"],
        environment: import.meta.env["VITE_PARALLAX_ENV"],
        gitSha:
          import.meta.env["VITE_GIT_SHA"] ??
          (globalThis as { __PLAYGROUND_GIT_SHA__?: string })
            .__PLAYGROUND_GIT_SHA__,
        sessionId,
      }),
    ),
  );
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
  const propagator = new CompositePropagator({
    propagators: [
      new W3CTraceContextPropagator(),
      new W3CBaggagePropagator(),
    ],
  });
  provider.register({
    contextManager: new ZoneContextManager(),
    propagator,
  });
  browserRootContext = sessionContext(extractDocumentContext(propagator));
  context.with(browserRootContext, () => {
    registerInstrumentations({
      instrumentations: [
        // The server injects its genuine request span. Without an inbound
        // parent, document-load remains a real browser root span.
        new DocumentLoadInstrumentation(),
        new FetchInstrumentation(),
        new UserInteractionInstrumentation(),
      ],
    });
  });
  void startWebVitals().finally(attachFlushListeners);
  queueMicrotask(() =>
    context.with(browserRootContext ?? context.active(), () =>
      trackScreen(window.location.pathname),
    ),
  );
  return browserRootContext;
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
  try {
    return await context.with(active, fn);
  } catch (err) {
    recordException(span, err);
    throw err;
  } finally {
    span.end();
  }
}

export async function tracedFetch(
  input: RequestInfo | URL,
  init: RequestInit = {},
): Promise<Response> {
  const requestContext = sessionContext();
  // FetchInstrumentation is the sole W3C propagation owner. It creates the
  // browser HTTP span and injects that span's context; manual injection here
  // races it and can preserve a foreign active trace.
  return context.with(requestContext, () => fetch(input, init));
}

export function emitTypedEvent(name: string, attributes: RumAttributes = {}) {
  const logger = eventLogger ?? logs.getLogger("playground.web.events");
  if (
    !logger.enabled({ severityNumber: SeverityNumber.INFO, eventName: name })
  ) {
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
    context: sessionContext(),
  });
}

function startRumSpan(name: string, attributes: RumAttributes): Span {
  return trace
    .getTracer(WEB_TRACER_NAME)
    .startSpan(
      name,
      { attributes: cleanAttributes(attributes) },
      sessionContext(),
    );
}

function recordException(span: Span, err: unknown) {
  const error = safeWebError(err);
  span.recordException(error);
  span.setStatus({ code: SpanStatusCode.ERROR, message: error.message });
  span.setAttribute(ERROR_TYPE, error.name);
}

export function sessionContext(base: Context = context.active()): Context {
  const effectiveBase = baseWithBrowserRoot(base);
  const entries = boundedBaggageEntries(propagation.getBaggage(effectiveBase));
  const baggage = propagation.createBaggage({
    ...entries,
    [SESSION_ID]: { value: getSessionId() },
  });
  return propagation.setBaggage(effectiveBase, baggage);
}

function baseWithBrowserRoot(base: Context): Context {
  if (browserRootContext === undefined) return base;
  const activeSpan = trace.getSpan(base);
  const browserRootSpan = trace.getSpan(browserRootContext);
  if (
    activeSpan !== undefined &&
    isSpanContextValid(activeSpan.spanContext()) &&
    (browserRootSpan === undefined ||
      !isSpanContextValid(browserRootSpan.spanContext()) ||
      activeSpan.spanContext().traceId === browserRootSpan.spanContext().traceId)
  ) {
    return base;
  }

  const entries = {
    ...boundedBaggageEntries(propagation.getBaggage(browserRootContext)),
    ...boundedBaggageEntries(propagation.getBaggage(base)),
  };
  return propagation.setBaggage(browserRootContext, propagation.createBaggage(entries));
}

function extractDocumentContext(propagator: CompositePropagator): Context {
  const raw = {
    traceparent: document
      .querySelector<HTMLMetaElement>('meta[name="traceparent"]')
      ?.content,
    tracestate: document
      .querySelector<HTMLMetaElement>('meta[name="tracestate"]')
      ?.content,
    baggage: document
      .querySelector<HTMLMetaElement>('meta[name="baggage"]')
      ?.content,
  };
  const sanitized = sanitizePropagationHeaders(raw);
  const carrier: Record<string, string> = {};
  for (const [key, value] of Object.entries(sanitized)) {
    if (value !== undefined) carrier[key] = value;
  }
  return propagator.extract(ROOT_CONTEXT, carrier, {
    get: (current, key) => current[key.toLowerCase()],
    keys: (current) => Object.keys(current),
  });
}

/** Capture handled UI failures without sending request secrets to Sentry. */
export function reportHandledError(
  error: unknown,
  operation: string,
  attributes: RumAttributes = {},
): void {
  if (typeof window === "undefined") return;
  const normalized = safeWebError(error);
  const safeOperation = safeWebOperation(operation);
  const safeAttributes = safeWebAttributes(attributes);
  const fingerprint = `${safeOperation}|${normalized.name}|${normalized.message}`;
  if (reportedHandledErrorKeys.has(fingerprint)) return;
  reportedHandledErrorKeys.add(fingerprint);
  if (reportedHandledErrorKeys.size > MAX_REPORTED_HANDLED_ERRORS) {
    const oldest = reportedHandledErrorKeys.values().next().value;
    if (oldest !== undefined) reportedHandledErrorKeys.delete(oldest);
  }

  emitTypedEvent("web.error.handled", {
    operation: safeOperation,
    error_code: normalized.name,
    error_type: normalized.name,
  });
  const errorSpan = startRumSpan("web.error.handled", {
    operation: safeOperation,
    error_code: normalized.name,
    error_type: normalized.name,
    ...safeAttributes,
  });
  errorSpan.recordException(normalized);
  errorSpan.setStatus({ code: SpanStatusCode.ERROR, message: normalized.message });
  errorSpan.end();
  Sentry.withScope((scope) => {
    scope.setTag("error.handled", "true");
    scope.setTag("operation", safeOperation);
    scope.setTag("error.code", normalized.name);
    scope.setTag("error.type", normalized.name);
    for (const [key, value] of Object.entries(safeAttributes)) {
      scope.setTag(key, value);
    }
    const graphqlErrors = graphqlErrorDetails(error);
    if (graphqlErrors.length > 0) {
      scope.setContext("graphql", {
        error_count: graphqlErrors.length,
        errors: graphqlErrors,
      });
    }
    Sentry.captureException(normalized);
  });
}

function graphqlErrorDetails(error: unknown): readonly string[] {
  if (!isRecord(error) || !Array.isArray(error["graphqlErrors"])) return [];
  return boundedGraphqlErrorDetails(error["graphqlErrors"].flatMap((value) => {
    if (!isRecord(value) || value["code"] !== "graphql_field_error") return [];
    const path = Array.isArray(value["path"])
      ? value["path"].filter(
          (part): part is GraphqlPathPart =>
            typeof part === "string" ||
            (typeof part === "number" && Number.isSafeInteger(part)),
        )
      : [];
    return [{ path }];
  }));
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function cleanAttributes(attributes: RumAttributes) {
  return Object.fromEntries(
    Object.entries(attributes).filter(
      (entry): entry is [string, RumAttributeValue] => {
        const value = entry[1];
        return value !== undefined;
      },
    ),
  );
}

function screenName(pathname: string) {
  if (pathname === "/") return "home";
  if (pathname === "/catalog" || pathname.startsWith("/products/"))
    return "catalog";
  if (pathname === "/cart") return "cart";
  if (pathname.startsWith("/checkout")) return "checkout";
  if (pathname.startsWith("/orders")) return "orders";
  if (pathname.startsWith("/analytics")) return "analytics";
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
  // Deterministic flush hook: automated browsers (Playwright) close pages
  // without reliably firing pagehide, so test teardown awaits this instead.
  (window as unknown as Record<string, unknown>)["__playgroundFlushTelemetry"] =
    async () => {
      await providerRef?.forceFlush();
      await loggerProviderRef?.forceFlush();
    };
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
