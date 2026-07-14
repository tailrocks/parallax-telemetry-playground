import { expect, test } from "vitest";
import { traceparentForTest } from "../e2e/test-trace-context";

test("derives a stable test parent under the run trace", () => {
  const runParent = "00-0123456789abcdef0123456789abcdef-0123456789abcdef-01";
  const first = traceparentForTest("chromium:journey", runParent);
  const second = traceparentForTest("chromium:journey", runParent);

  expect(first).toBe(second);
  expect(first).toMatch(/^00-0123456789abcdef0123456789abcdef-[0-9a-f]{16}-01$/);
});

test("uses separate trace IDs when no run parent is provided", () => {
  expect(traceparentForTest("first", undefined)).not.toEqual(
    traceparentForTest("second", undefined),
  );
});
