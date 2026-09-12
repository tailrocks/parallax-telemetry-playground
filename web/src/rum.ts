import {
  context,
  propagation,
  SpanStatusCode,
  trace,
  type Span,
} from "@opentelemetry/api";
import type { RumAttributes } from "./telemetry";
import { sanitizePropagationHeadersInPlace } from "./traceparent";
import {
  safeWebAttributes,
  safeWebError,
  safeWebOperation,
} from "./error-contract";

export async function trackStep(
  name: string,
  attributes: RumAttributes = {},
) {
  if (typeof window === "undefined") {
    const span = startServerSpan(name, attributes);
    span.addEvent(name, cleanAttributes(attributes));
    span.end();
    return;
  }
  const telemetry = await import("./telemetry");
  telemetry.trackStep(name, attributes);
}

export async function runTracedStep<T>(
  name: string,
  attributes: RumAttributes,
  fn: () => Promise<T>,
): Promise<T> {
  if (typeof window === "undefined") return runServerTracedStep(name, attributes, fn);
  const telemetry = await import("./telemetry");
  return telemetry.runTracedStep(name, attributes, fn);
}

export async function tracedFetch(
  input: RequestInfo | URL,
  init: RequestInit = {},
): Promise<Response> {
  if (typeof window === "undefined") return fetchWithActiveContext(input, init);
  const telemetry = await import("./telemetry");
  return telemetry.tracedFetch(input, init);
}

export async function emitTypedEvent(
  name: string,
  attributes: RumAttributes = {},
) {
  if (typeof window === "undefined") {
    const span = startServerSpan(name, attributes);
    span.addEvent(name, cleanAttributes(attributes));
    span.end();
    return;
  }
  const telemetry = await import("./telemetry");
  telemetry.emitTypedEvent(name, attributes);
}

export function reportHandledError(
  error: unknown,
  operation: string,
  attributes: RumAttributes = {},
): void {
  if (typeof window === "undefined") {
    const span = trace.getSpan(context.active());
    if (span === undefined) return;
    const normalized = safeWebError(error);
    const safeOperation = safeWebOperation(operation, "web.ssr");
    span.setAttribute("web.degraded", true);
    span.addEvent("web.error.handled", {
      operation: safeOperation,
      "error.code": normalized.name,
      "error.type": normalized.name,
      "error.message": normalized.message,
      ...safeWebAttributes(attributes),
    });
    return;
  }
  void import("./telemetry").then((telemetry) => {
    telemetry.reportHandledError(error, operation, attributes);
  });
}

function startServerSpan(name: string, attributes: RumAttributes): Span {
  return trace.getTracer("playground.web.ssr").startSpan(name, {
    attributes: cleanAttributes(attributes),
  });
}

async function runServerTracedStep<T>(
  name: string,
  attributes: RumAttributes,
  fn: () => Promise<T>,
): Promise<T> {
  const span = startServerSpan(name, attributes);
  span.addEvent(name, cleanAttributes(attributes));
  const active = trace.setSpan(context.active(), span);
  try {
    return await context.with(active, fn);
  } catch (error: unknown) {
    const normalized = safeWebError(error);
    span.recordException(normalized);
    span.setStatus({ code: SpanStatusCode.ERROR, message: normalized.message });
    throw error;
  } finally {
    span.end();
  }
}

function fetchWithActiveContext(
  input: RequestInfo | URL,
  init: RequestInit,
): Promise<Response> {
  const headers = new Headers(init.headers);
  propagation.inject(context.active(), headers, {
    set: (carrier, key, value) => carrier.set(key, value),
  });
  sanitizePropagationHeadersInPlace(headers);
  return fetch(input, { ...init, headers });
}

function cleanAttributes(attributes: RumAttributes): Record<string, string | number | boolean> {
  return Object.fromEntries(
    Object.entries(attributes).filter(
      (entry): entry is [string, string | number | boolean] =>
        entry[1] !== undefined,
    ),
  );
}
