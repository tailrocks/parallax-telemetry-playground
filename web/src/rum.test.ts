import {
  ROOT_CONTEXT,
  TraceFlags,
  context,
  createTraceState,
  propagation,
  trace,
  type Span,
} from "@opentelemetry/api";
import {
  CompositePropagator,
  W3CBaggagePropagator,
  W3CTraceContextPropagator,
} from "@opentelemetry/core";
import { afterEach, describe, expect, test, vi } from "vitest";
import {
  emitTypedEvent,
  reportHandledError,
  runTracedStep,
  tracedFetch,
  trackStep,
} from "./rum";
import { serverTracer, shutdownServerTelemetry } from "./server-telemetry";

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("SSR RUM boundary", () => {
  test("executes traced work without loading browser telemetry", async () => {
    await expect(runTracedStep("server", {}, async () => "ok")).resolves.toBe("ok");
  });

  test("delegates fetch without browser telemetry", async () => {
    const response = new Response("ok");
    const fetchMock = vi.fn(async () => response);
    vi.stubGlobal("fetch", fetchMock);
    await expect(tracedFetch("https://example.invalid/health")).resolves.toBe(response);
    expect(fetchMock).toHaveBeenCalledOnce();
  });

  test("does not emit browser-only events during SSR", async () => {
    await expect(trackStep("server", {})).resolves.toBeUndefined();
    await expect(emitTypedEvent("server", {})).resolves.toBeUndefined();
  });

  test("annotates handled SSR failures without exporting secrets", () => {
    const addEvent = vi.fn();
    const setAttribute = vi.fn();
    const span = {
      addEvent,
      setAttribute,
    } as unknown as Span;
    vi.spyOn(trace, "getSpan").mockReturnValue(span);

    reportHandledError(
      new Error("payment_token=secret-value"),
      "StorefrontCheckout",
      { payment_token: "secret-value", safe_field: "checkout" },
    );

    expect(setAttribute).toHaveBeenCalledWith("web.degraded", true);
    const event = addEvent.mock.calls[0]?.[1] as Record<string, unknown>;
    expect(event["error.code"]).toBe("unexpected");
    expect(event["error.message"]).toBe(
      "Something unexpected happened. Retry.",
    );
    expect(event["payment_token"]).toBeUndefined();
    expect(event["safe_field"]).toBeUndefined();
  });

  test("forwards the active server context and only bounded baggage", async () => {
    serverTracer();
    propagation.setGlobalPropagator(
      new CompositePropagator({
        propagators: [
          new W3CTraceContextPropagator(),
          new W3CBaggagePropagator(),
        ],
      }),
    );
    const active = propagation.setBaggage(
      trace.setSpanContext(ROOT_CONTEXT, {
        traceId: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        spanId: "bbbbbbbbbbbbbbbb",
        traceFlags: TraceFlags.SAMPLED,
        traceState: createTraceState("vendor=value"),
        isRemote: true,
      }),
      propagation.createBaggage({
        "tenant.id": { value: "tenant-a" },
        "feature.variant": { value: "blue" },
        "secret.token": { value: "must-not-cross" },
      }),
    );
    const fetchMock = vi.fn(
      async (_input: RequestInfo | URL, init?: RequestInit) => {
        const headers = new Headers(init?.headers);
        expect(headers.get("traceparent")).toBe(
          "00-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-bbbbbbbbbbbbbbbb-01",
        );
        expect(headers.get("tracestate")).toBe("vendor=value");
        expect(headers.get("baggage")).toBe(
          "tenant.id=tenant-a,feature.variant=blue",
        );
        return new Response("ok");
      },
    );
    vi.stubGlobal("fetch", fetchMock);

    try {
      await context.with(active, () => tracedFetch("https://example.invalid/api"));
      expect(fetchMock).toHaveBeenCalledOnce();
    } finally {
      await shutdownServerTelemetry();
    }
  });
});
