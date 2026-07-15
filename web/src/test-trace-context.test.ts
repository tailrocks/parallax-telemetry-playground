import { mkdir, rm, writeFile } from "node:fs/promises";
import { dirname } from "node:path";
import { afterEach, expect, test, vi } from "vitest";
import {
  testTraceparentPath,
  traceparentForRunningTest,
  traceparentForTest,
} from "../e2e/test-trace-context";

afterEach(() => {
  vi.unstubAllEnvs();
});

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

test("reads the real reporter span context for an observable run", async () => {
  const testId = "chromium:observable-journey";
  const path = testTraceparentPath(testId);
  const traceparent = "00-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-bbbbbbbbbbbbbbbb-01";
  vi.stubEnv("PLAYGROUND_TEST_OTLP_ENDPOINT", "http://127.0.0.1:4318/v1/traces");
  await mkdir(dirname(path), { recursive: true });
  await writeFile(path, `${traceparent}\n`, { mode: 0o600 });
  try {
    await expect(traceparentForRunningTest(testId)).resolves.toBe(traceparent);
  } finally {
    await rm(path, { force: true });
  }
});
