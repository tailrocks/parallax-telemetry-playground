import { describe, expect, test } from "vitest";
import {
  injectPropagationMeta,
  sanitizeBaggageHeader,
  sentryTraceFromTraceparent,
  validTraceparent,
  validTracestate,
} from "./traceparent";

const parent = "00-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-bbbbbbbbbbbbbbbb-01";

describe("SSR traceparent handoff", () => {
  test("accepts only a strict lowercase inbound parent", () => {
    expect(validTraceparent(parent)).toBe(parent);
    expect(sentryTraceFromTraceparent(parent)).toBe(
      "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-bbbbbbbbbbbbbbbb-1",
    );
    expect(validTraceparent(parent.toUpperCase())).toBeUndefined();
    expect(validTraceparent(` ${parent} `)).toBeUndefined();
    expect(validTraceparent(null)).toBeUndefined();
    expect(
      validTraceparent(
        "00-00000000000000000000000000000000-bbbbbbbbbbbbbbbb-01",
      ),
    ).toBeUndefined();
    expect(
      validTraceparent("00-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-0000000000000000-01"),
    ).toBeUndefined();
    expect(validTraceparent("not-a-traceparent")).toBeUndefined();
  });

  test("injects one optional parent into an SSR document", () => {
    const document = "<html><head><title>Lab</title></head><body /></html>";
    const withParent = injectPropagationMeta(document, { traceparent: parent });

    expect(withParent).toContain(`<meta name="traceparent" content="${parent}">`);
    expect(withParent).toContain(
      `<meta name="sentry-trace" content="aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-bbbbbbbbbbbbbbbb-1">`,
    );
    expect(injectPropagationMeta(withParent, { traceparent: parent })).toBe(
      withParent,
    );
    expect(injectPropagationMeta(document, { traceparent: "invalid" })).toBe(
      document,
    );
  });

  test("validates tracestate and keeps only bounded safe baggage", () => {
    expect(validTracestate("vendor=value, rojo=00f067aa0ba902b7")).toBe(
      "vendor=value,rojo=00f067aa0ba902b7",
    );
    expect(
      validTracestate(" \tvendor=value \t, \trojo=00f067aa0ba902b7 \t"),
    ).toBe("vendor=value,rojo=00f067aa0ba902b7");
    expect(validTracestate("vendor=value,\trojo=00f067aa0ba902b7")).toBe(
      "vendor=value,rojo=00f067aa0ba902b7",
    );
    expect(validTracestate("vendor=value with legal spaces")).toBe(
      "vendor=value with legal spaces",
    );
    expect(validTracestate("vendor=value,,rojo=00f067aa0ba902b7")).toBe(
      undefined,
    );
    expect(validTracestate("vendor=value, \t,rojo=00f067aa0ba902b7")).toBe(
      undefined,
    );
    expect(validTracestate("vendor= ")).toBeUndefined();
    expect(validTracestate("Vendor=value")).toBeUndefined();
    expect(validTracestate("vendor=value,vendor=other")).toBeUndefined();
    const maxValue = "v".repeat(256);
    expect(new TextEncoder().encode(maxValue).byteLength).toBe(256);
    expect(validTracestate(`vendor=${maxValue}`)).toBe(`vendor=${maxValue}`);
    expect(validTracestate(`vendor=${maxValue}v`)).toBeUndefined();
    expect(validTracestate("vendor=value\nforged=header")).toBeUndefined();
    expect(validTracestate("vendor=")).toBeUndefined();
    expect(validTracestate(`a${"b".repeat(240)}@vendor=value`)).toBe(
      `a${"b".repeat(240)}@vendor=value`,
    );
    expect(validTracestate(`a${"b".repeat(241)}@vendor=value`)).toBeUndefined();

    expect(
      sanitizeBaggageHeader(
        "tenant.id=tenant-a;trusted=true,feature.variant=blue,secret.token=leak",
      ),
    ).toBe("tenant.id=tenant-a;trusted=true,feature.variant=blue");
    expect(sanitizeBaggageHeader("tenant.id=tenant%2Da")).toBe(
      "tenant.id=tenant%2Da",
    );
    expect(sanitizeBaggageHeader("tenant.id=tenant a")).toBeUndefined();
    expect(sanitizeBaggageHeader("tenant.id=tenant-a;not-a-property")).toBe(
      undefined,
    );
  });

  test("bootstraps trace state and inherited baggage without exposing secrets", () => {
    const document = "<html><head></head><body /></html>";
    const bootstrapped = injectPropagationMeta(document, {
      traceparent: parent,
      tracestate: "vendor=value",
      baggage: "tenant.id=tenant-a,customer.segment=standard,secret.token=leak",
    });

    expect(bootstrapped).toContain(
      `<meta name="traceparent" content="${parent}">`,
    );
    expect(bootstrapped).toContain(
      `<meta name="tracestate" content="vendor=value">`,
    );
    expect(bootstrapped).toContain(
      `<meta name="baggage" content="tenant.id=tenant-a,customer.segment=standard">`,
    );
    expect(bootstrapped).not.toContain("secret.token");
  });
});
