import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

const W3C_TRACEPARENT = /^00-([0-9a-f]{32})-([0-9a-f]{16})-0[01]$/i;

/**
 * Gives one Playwright test a stable synthetic parent. When a run-level
 * TRACEPARENT exists, its trace ID is retained so every test root joins that
 * Parallax Run; otherwise the test ID deterministically supplies a trace ID.
 */
export function traceparentForTest(testId: string, runTraceparent = process.env.TRACEPARENT): string {
  const digest = createHash("sha256").update(testId).digest("hex");
  const inheritedTraceId = runTraceparent?.match(W3C_TRACEPARENT)?.[1];
  const traceId = inheritedTraceId ?? digest.slice(0, 32);
  const spanId = digest.slice(32, 48);
  return `00-${traceId}-${spanId}-01`;
}

export function testTraceparentPath(testId: string): string {
  const key = createHash("sha256").update(testId).digest("hex");
  return join(testTraceparentDirectory(), key);
}

export function testTraceparentDirectory(): string {
  return join(tmpdir(), "parallax-playwright-traceparents");
}

export async function traceparentForRunningTest(testId: string): Promise<string> {
  if (!process.env.PLAYGROUND_TEST_OTLP_ENDPOINT) return traceparentForTest(testId);

  const path = testTraceparentPath(testId);
  for (let attempt = 0; attempt < 100; attempt += 1) {
    try {
      const traceparent = (await readFile(path, "utf8")).trim();
      if (W3C_TRACEPARENT.test(traceparent)) return traceparent;
      throw new Error(`invalid test traceparent in ${path}`);
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
      await new Promise((resolve) => setTimeout(resolve, 10));
    }
  }
  throw new Error(`timed out waiting for Playwright test traceparent: ${testId}`);
}
