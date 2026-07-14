import { createHash } from "node:crypto";

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
