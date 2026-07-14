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
        }),
        spanProcessors: [
          new SimpleSpanProcessor(new OTLPTraceExporter({ url: this.endpoint })),
        ],
      })
    : undefined;
  private readonly parentContext = parentContextFromEnvironment();

  onTestEnd(test: TestCase, result: TestResult): void {
    if (!this.provider) return;

    const failed = result.status !== "passed";
    const span = this.provider.getTracer(TEST_TRACER).startSpan(
      "test.case",
      {
        startTime: result.startTime,
        attributes: {
          [TEST_CASE_NAME]: test.title,
          [TEST_CASE_RESULT_STATUS]: failed ? "fail" : "pass",
          [TEST_SUITE_NAME]: test.titlePath().slice(0, -1).join(" › "),
          [TEST_SUITE_RUN_STATUS]: failed ? "fail" : "pass",
          [PARALLAX_TEST_ID]: test.id,
          [CICD_PIPELINE_RUN_ID]: process.env.CI_RUN_ID ?? "",
          [CICD_PIPELINE_TASK_TYPE]: "playwright",
          "test.case.retry": result.retry,
          "test.case.duration_ms": result.duration,
        },
      },
      this.parentContext,
    );
    if (failed) {
      const error = result.error;
      const message = error?.message ?? `Playwright test ${result.status}`;
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

function parentContextFromEnvironment(): Context {
  const traceparent = process.env.TRACEPARENT;
  if (!traceparent) return ROOT_CONTEXT;
  return new W3CTraceContextPropagator().extract(
    ROOT_CONTEXT,
    { traceparent },
    { get: (carrier, key) => carrier[key] },
  );
}
