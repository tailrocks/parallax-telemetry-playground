import { createServer } from "node:http";
import {
  ROOT_CONTEXT,
  TraceFlags,
  context,
  createTraceState,
  propagation,
  trace,
} from "@opentelemetry/api";
import { afterEach, describe, expect, test, vi } from "vitest";
import {
  inboundServerContext,
  propagationHeaders,
  shutdownServerTelemetry,
  traceServerRequest,
} from "./server-telemetry";

const traceId = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const spanId = "bbbbbbbbbbbbbbbb";

afterEach(() => {
  vi.unstubAllGlobals();
  vi.unstubAllEnvs();
});

describe("SSR W3C context handoff", () => {
  test("extracts sanitized tracestate and all safe baggage from a request", () => {
    const request = new Request("https://web.test/", {
      headers: {
        traceparent: `00-${traceId}-${spanId}-01`,
        tracestate: "vendor=value",
        baggage:
          "tenant.id=tenant-a,customer.segment=standard,feature.variant=blue,secret.token=leak",
      },
    });

    const extracted = inboundServerContext(request);
    const spanContext = trace.getSpanContext(extracted);

    expect(spanContext?.traceId).toBe(traceId);
    expect(spanContext?.spanId).toBe(spanId);
    expect(spanContext?.traceState?.serialize()).toBe("vendor=value");
    expect(propagation.getBaggage(extracted)?.getEntry("tenant.id")?.value).toBe(
      "tenant-a",
    );
    expect(
      propagation.getBaggage(extracted)?.getEntry("customer.segment")?.value,
    ).toBe("standard");
    expect(
      propagation.getBaggage(extracted)?.getEntry("feature.variant")?.value,
    ).toBe("blue");
    expect(propagation.getBaggage(extracted)?.getEntry("secret.token")).toBeUndefined();
  });

  test("injects the genuine parent context without minting a trace id", () => {
    const context = propagation.setBaggage(
      trace.setSpanContext(ROOT_CONTEXT, {
        traceId,
        spanId,
        traceFlags: TraceFlags.SAMPLED,
        traceState: createTraceState("vendor=value"),
        isRemote: true,
      }),
      propagation.createBaggage({
        "tenant.id": { value: "tenant-a" },
        "feature.variant": { value: "blue" },
        "secret.token": { value: "leak" },
      }),
    );

    expect(propagationHeaders(context)).toEqual({
      traceparent: `00-${traceId}-${spanId}-01`,
      tracestate: "vendor=value",
      baggage: "tenant.id=tenant-a,feature.variant=blue",
    });
  });

  test("creates a real SSR span and carries it with inherited context", async () => {
    let exported = 0;
    const collector = createServer((_request, response) => {
      exported += 1;
      response.statusCode = 200;
      response.end();
    });
    await new Promise<void>((resolve, reject) => {
      collector.once("error", reject);
      collector.listen(0, "127.0.0.1", resolve);
    });
    const address = collector.address();
    if (address === null || typeof address === "string") {
      collector.close();
      throw new Error("test collector did not bind to a TCP port");
    }
    vi.stubEnv(
      "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
      `http://127.0.0.1:${address.port}/v1/traces`,
    );
    const request = new Request("https://web.test/", {
      headers: {
        traceparent: `00-${traceId}-${spanId}-01`,
        tracestate: "vendor=value",
        baggage: "tenant.id=tenant-a,feature.variant=blue",
      },
    });

    try {
      const traced = await traceServerRequest(request, async () => {
        const active = trace.getSpanContext(context.active());
        expect(active?.traceId).toBe(traceId);
        expect(active?.spanId).not.toBe(spanId);
        expect(active?.traceState?.serialize()).toBe("vendor=value");
        expect(
          propagation.getBaggage(context.active())?.getEntry("tenant.id")
            ?.value,
        ).toBe("tenant-a");
        return new Response("ok");
      });

      const headers = propagationHeaders(traced.context);
      expect(headers.traceparent?.startsWith(`00-${traceId}-`)).toBe(true);
      expect(headers.traceparent).not.toBe(`00-${traceId}-${spanId}-01`);
      expect(headers.tracestate).toBe("vendor=value");
      expect(headers.baggage).toBe("tenant.id=tenant-a,feature.variant=blue");
    } finally {
      await shutdownServerTelemetry();
      await new Promise<void>((resolve, reject) => {
        collector.close((error) => (error === undefined ? resolve() : reject(error)));
      });
    }
    expect(exported).toBeGreaterThan(0);
  });
});
