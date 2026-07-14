import { afterEach, describe, expect, test, vi } from "vitest";
import { emitTypedEvent, runTracedStep, tracedFetch, trackStep } from "./rum";

afterEach(() => {
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
});
