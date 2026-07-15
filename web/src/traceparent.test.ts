import { describe, expect, test } from "vitest";
import { mintTraceparent } from "./traceparent";

describe("mintTraceparent", () => {
  test("creates unique W3C traceparent values", () => {
    const first = mintTraceparent();
    const second = mintTraceparent();
    expect(first).toMatch(/^00-[0-9a-f]{32}-[0-9a-f]{16}-01$/);
    expect(second).toMatch(/^00-[0-9a-f]{32}-[0-9a-f]{16}-01$/);
    expect(first).not.toBe(second);
  });
});
