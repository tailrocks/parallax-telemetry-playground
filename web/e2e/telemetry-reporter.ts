import type { FullResult, Reporter, TestCase, TestResult } from "@playwright/test/reporter";
import { mkdirSync, rmSync, unlinkSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";
import { ROOT_CONTEXT, SpanStatusCode, type Context, type Span } from "@opentelemetry/api";
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
  SERVICE_VERSION,
  TEST_ATTEMPT_ORDINAL,
  TEST_CASE_NAME,
  TEST_CASE_FAILURE_KIND,
  TEST_CASE_PARAMETERS,
  TEST_CASE_RESULT_STATUS,
  TEST_CONFIGURATION_BROWSER,
  TEST_CONFIGURATION_ENVIRONMENT,
  TEST_CONFIGURATION_OS,
  TEST_FAILURE_KIND_ASSERTION,
  TEST_FAILURE_KIND_HARNESS,
  TEST_RESULT_STATUS_FAIL,
  TEST_RESULT_STATUS_PASS,
  TEST_ARTIFACT_PATH,
  TEST_SUITE_NAME,
  TEST_SUITE_RUN_STATUS,
  VCS_REF_HEAD_REVISION,
} from "../src/semconv";
import { testTraceparentDirectory, testTraceparentPath } from "./test-trace-context";

const TEST_TRACER = "playground.web.test";

/**
 * Opt-in Playwright-to-OTLP bridge. The reporter deliberately has no browser
 * dependency: Playwright invokes it in Bun, and explicit timestamps retain the
 * runner's measured test duration. Set PLAYGROUND_TEST_OTLP_ENDPOINT to enable
 * exports, normally under `parallax run start -- bun run e2e`.
 */
export default class TelemetryReporter implements Reporter {
  private readonly spans = new Map<string, Span>();
  private readonly endpoint = process.env.PLAYGROUND_TEST_OTLP_ENDPOINT;
  private readonly provider = this.endpoint
    ? new WebTracerProvider({
        resource: resourceFromAttributes({
          [ATTR_SERVICE_NAME]: "playground-web-tests",
          [PARALLAX_RUN_ID]: process.env.PARALLAX_RUN_ID ?? "",
          [SERVICE_VERSION]: process.env.RELEASE ?? "dev",
          [VCS_REF_HEAD_REVISION]: process.env.GITHUB_SHA ?? process.env.VCS_REF ?? "",
        }),
        spanProcessors: [new SimpleSpanProcessor(new OTLPTraceExporter({ url: this.endpoint }))],
      })
    : undefined;
  onBegin(): void {
    if (this.provider) {
      rmSync(testTraceparentDirectory(), { recursive: true, force: true });
    }
  }

  onTestBegin(test: TestCase, result: TestResult): void {
    if (!this.provider) return;

    const titlePath = test.titlePath();
    const span = this.provider.getTracer(TEST_TRACER).startSpan(
      "test.case",
      {
        startTime: result.startTime,
        attributes: {
          [TEST_CASE_NAME]: titlePath.join(" › "),
          [TEST_SUITE_NAME]: titlePath.slice(0, -1).join(" › "),
          [PARALLAX_TEST_ID]: test.id,
          [CICD_PIPELINE_RUN_ID]: process.env.CI_RUN_ID ?? "",
          [CICD_PIPELINE_TASK_TYPE]: "playwright",
          [TEST_ATTEMPT_ORDINAL]: result.retry + 1,
          [TEST_CASE_PARAMETERS]: parametersForTitle(test.title),
          [TEST_CONFIGURATION_BROWSER]: process.env.PLAYWRIGHT_BROWSER ?? "chromium",
          [TEST_CONFIGURATION_ENVIRONMENT]: process.env.PLAYGROUND_ENV ?? "playground",
          [TEST_CONFIGURATION_OS]: process.platform,
        },
      },
      runParentContext(),
    );
    this.spans.set(spanKey(test, result), span);
    const context = span.spanContext();
    const traceFlags = context.traceFlags.toString(16).padStart(2, "0");
    const path = testTraceparentPath(test.id);
    mkdirSync(dirname(path), { recursive: true });
    writeFileSync(path, `00-${context.traceId}-${context.spanId}-${traceFlags}\n`, {
      mode: 0o600,
    });
  }

  onTestEnd(test: TestCase, result: TestResult): void {
    if (!this.provider) return;
    const key = spanKey(test, result);
    const span = this.spans.get(key);
    if (!span) throw new Error(`missing Playwright telemetry span for ${key}`);
    this.spans.delete(key);

    const failed =
      result.status === "failed" || result.status === "timedOut" || result.status === "interrupted";
    const status =
      result.status === "skipped"
        ? "skip"
        : failed
          ? TEST_RESULT_STATUS_FAIL
          : TEST_RESULT_STATUS_PASS;
    span.setAttribute(TEST_CASE_RESULT_STATUS, status);
    span.setAttribute(TEST_SUITE_RUN_STATUS, status);
    span.setAttribute("test.case.duration_ms", result.duration);
    if (failed) {
      const error = result.error;
      const message = error?.message ?? `Playwright test ${result.status}`;
      span.setAttribute(
        TEST_CASE_FAILURE_KIND,
        result.status === "timedOut" || result.status === "interrupted"
          ? TEST_FAILURE_KIND_HARNESS
          : TEST_FAILURE_KIND_ASSERTION,
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
    try {
      unlinkSync(testTraceparentPath(test.id));
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
    }
  }

  async onEnd(_result: FullResult): Promise<void> {
    await this.provider?.forceFlush();
    await this.provider?.shutdown();
  }
}

function spanKey(test: TestCase, result: TestResult): string {
  return `${test.id}:${result.retry}`;
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

function runParentContext(): Context {
  const traceparent = process.env.TRACEPARENT;
  return traceparent ? parentContextFromTraceparent(traceparent) : ROOT_CONTEXT;
}
