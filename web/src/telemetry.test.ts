import {
  ROOT_CONTEXT,
  baggageEntryMetadataFromString,
  propagation,
} from "@opentelemetry/api";
import { describe, expect, test } from "vitest";
import { sessionContext } from "./telemetry";

describe("browser context bootstrap", () => {
  test("preserves every inherited safe baggage entry and its metadata", () => {
    const inherited = propagation.createBaggage({
      "tenant.id": { value: "tenant-a" },
      "user.tier": { value: "pro" },
      "customer.segment": { value: "standard" },
      region: { value: "us-east-1" },
      "request.priority": { value: "normal" },
      "feature.variant": {
        value: "blue",
        metadata: baggageEntryMetadataFromString("source=upstream"),
      },
      "cli.invocation.id": { value: "invocation-1" },
      "secret.token": { value: "must-not-cross" },
    });
    const base = propagation.setBaggage(ROOT_CONTEXT, inherited);

    const result = propagation.getBaggage(sessionContext(base));

    expect(result?.getEntry("tenant.id")?.value).toBe("tenant-a");
    expect(result?.getEntry("user.tier")?.value).toBe("pro");
    expect(result?.getEntry("customer.segment")?.value).toBe("standard");
    expect(result?.getEntry("region")?.value).toBe("us-east-1");
    expect(result?.getEntry("request.priority")?.value).toBe("normal");
    expect(result?.getEntry("feature.variant")?.value).toBe("blue");
    expect(result?.getEntry("feature.variant")?.metadata?.toString()).toBe(
      "source=upstream",
    );
    expect(result?.getEntry("cli.invocation.id")?.value).toBe("invocation-1");
    expect(result?.getEntry("secret.token")).toBeUndefined();
    expect(result?.getEntry("session.id")?.value).toBe("server");
  });
});
