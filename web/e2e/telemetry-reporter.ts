import type {
  FullResult,
  Reporter,
  TestCase,
  TestResult,
} from "@playwright/test/reporter";
import {
  ROOT_CONTEXT,
  SpanStatusCode,
  type Context,
} from "@opentelemetry/api";
import { W3CTraceContextPropagator } from "@opentelemetry/core";
import { OTLPTraceExporter } from "@opentelemetry/exporter-trace-otlp-proto";
import { resourceFromAttributes } from "@opentelemetry/resources";
import { SimpleSpanProcessor, WebTracerProvider } from "@opentelemetry/sdk-trace-web";
import { ATTR_SERVICE_NAME } from "@opentelemetry/semantic-conventions";
import {
  CICD_PIPELINE_RUN_ID,
  CICD_PIPELINE_TASK_TYPE,
  PARALLAX_RUN_ID,
  PARALLAX_TEST_ID,
  TEST_CASE_NAME,
  TEST_CASE_RESULT_STATUS,
  TEST_ARTIFACT_PATH,
  TEST_SUITE_NAME,
  TEST_SUITE_RUN_STATUS,
} from "../src/semconv";
import { traceparentForTest } from "./test-trace-context";

const TEST_TRACER = "playground.web.test";

/**
 * Opt-in Playwright-to-OTLP bridge. The reporter deliberately has no browser
 * dependency: Playwright invokes it in Bun, and explicit timestamps retain the
 * runner's measured test duration. Set PLAYGROUND_TEST_OTLP_ENDPOINT to enable
 * exports, normally under `parallax run start -- bun run e2e`.
 */
export default class TelemetryReporter implements Reporter {
  private readonly endpoint = process.env.PLAYGROUND_TEST_OTLP_ENDPOINT;
  private readonly provider = this.endpoint
    ? new WebTracerProvider({
        resource: resourceFromAttributes({
          [ATTR_SERVICE_NAME]: "playground-web-tests",
          [PARALLAX_RUN_ID]: process.env.PARALLAX_RUN_ID ?? "",
          "service.version": process.env.RELEASE ?? "dev",
          "vcs.ref.head.revision": process.env.GITHUB_SHA ?? process.env.VCS_REF ?? "",
        }),
        spanProcessors: [
          new SimpleSpanProcessor(new OTLPTraceExporter({ url: this.endpoint })),
        ],
      })
    : undefined;
  onTestEnd(test: TestCase, result: TestResult): void {
    if (!this.provider) return;

    const failed = result.status === "failed" || result.status === "timedOut" || result.status === "interrupted";
    const status = result.status === "skipped" ? "skip" : failed ? "fail" : "pass";
    const titlePath = test.titlePath();
    const span = this.provider.getTracer(TEST_TRACER).startSpan(
      "test.case",
      {
        startTime: result.startTime,
        attributes: {
          [TEST_CASE_NAME]: titlePath.join(" › "),
          [TEST_CASE_RESULT_STATUS]: status,
          [TEST_SUITE_NAME]: titlePath.slice(0, -1).join(" › "),
          [TEST_SUITE_RUN_STATUS]: status,
          [PARALLAX_TEST_ID]: test.id,
          [CICD_PIPELINE_RUN_ID]: process.env.CI_RUN_ID ?? "",
          [CICD_PIPELINE_TASK_TYPE]: "playwright",
          "test.attempt.ordinal": result.retry + 1,
          "test.case.duration_ms": result.duration,
          "test.case.parameters": parametersForTitle(test.title),
          "test.configuration.browser": process.env.PLAYWRIGHT_BROWSER ?? "chromium",
          "test.configuration.environment": process.env.PLAYGROUND_ENV ?? "playground",
          "test.configuration.os": process.platform,
        },
      },
      parentContextFromTraceparent(traceparentForTest(test.id)),
    );
    if (failed) {
      const error = result.error;
      const message = error?.message ?? `Playwright test ${result.status}`;
      span.setAttribute(
        "test.case.failure.kind",
        result.status === "timedOut" || result.status === "interrupted"
          ? "harness_error"
          : "assertion_failure",
      );
      span.recordException({
        name: error?.name ?? "PlaywrightTestError",
        message,
        stack: error?.stack,
      });
      span.setStatus({ code: SpanStatusCode.ERROR, message });
    }
    const traceArchive = result.attachments.find((attachment) =>
      attachment.path?.endsWith("trace.zip"),
    );
    if (traceArchive?.path) {
      span.setAttribute(TEST_ARTIFACT_PATH, traceArchive.path);
    }
    span.end(new Date(result.startTime.getTime() + result.duration));
  }

  async onEnd(_result: FullResult): Promise<void> {
    await this.provider?.forceFlush();
    await this.provider?.shutdown();
  }
}

function parametersForTitle(title: string): string {
  const match = title.match(/\[([^\]]+)]/);
  return match?.[1] ?? "";
}

function parentContextFromTraceparent(traceparent: string): Context {
  return new W3CTraceContextPropagator().extract(
    ROOT_CONTEXT,
    { traceparent },
    { get: (carrier, key) => carrier[key] },
  );
}
