import { expect, type Page, type Request, test as base } from "@playwright/test";
import { sanitizePropagationHeaders } from "../src/traceparent";
import { traceparentForRunningTest } from "./test-trace-context";

const DEFAULT_TRACESTATE = "playground=browser";
const DEFAULT_BAGGAGE =
  "tenant.id=tenant-acme,user.tier=standard,customer.segment=standard,region=us-east-1,request.priority=normal";

export const E2E_TRACESTATE = runPropagationHeader(
  "tracestate",
  process.env["TRACESTATE"],
  DEFAULT_TRACESTATE,
);
export const E2E_BAGGAGE = runPropagationHeader(
  "baggage",
  process.env["BAGGAGE"],
  DEFAULT_BAGGAGE,
);

type CapturedRequest = Readonly<{
  url: string;
  headers: Readonly<Record<string, string>>;
}>;

const TRACEPARENT_PATTERN = /^00-[0-9a-f]{32}-[0-9a-f]{16}-[0-9a-f]{2}$/;

export const test = base.extend<{ testTraceparent: string }>({
  testTraceparent: async ({}, use, testInfo) => {
    await use(await traceparentForRunningTest(testInfo.testId));
  },
  page: async ({ page, testTraceparent }, use) => {
    const hydratedRequests: Array<Promise<CapturedRequest>> = [];
    page.on("request", (request) => {
      if (!isHydratedApplicationRequest(request)) return;
      hydratedRequests.push(
        request.allHeaders().then((headers) => ({ url: request.url(), headers })),
      );
    });
    await page.route("**/*", async (route) => {
      if (route.request().resourceType() !== "document") {
        await route.continue();
        return;
      }
      await route.continue({
        headers: {
          ...route.request().headers(),
          traceparent: testTraceparent,
          tracestate: E2E_TRACESTATE,
          baggage: E2E_BAGGAGE,
        },
      });
    });
    await use(page);
    // Await the page's deterministic telemetry flush before teardown closes
    // it; closing skips pagehide in headless automation and drops batches.
    try {
      await page.evaluate(async () => {
        const flush = (window as unknown as Record<string, unknown>)[
          "__playgroundFlushTelemetry"
        ];
        if (typeof flush === "function") await (flush as () => Promise<void>)();
      });
    } catch {
      // Page may already be gone (crash tests); telemetry loss is acceptable there.
    }
    await assertHydratedPropagation(hydratedRequests, testTraceparent);
  },
});

/** Assert that SSR created a child span instead of echoing the test parent. */
export async function expectSsrTraceparent(
  page: Page,
  inboundTraceparent: string,
): Promise<void> {
  const meta = page.locator('meta[name="traceparent"]');
  await expect(meta).toHaveAttribute(
    "content",
    /^00-[0-9a-f]{32}-[0-9a-f]{16}-[0-9a-f]{2}$/,
  );
  const actual = await meta.getAttribute("content");
  expect(actual).not.toBe(inboundTraceparent);
  expect(actual?.split("-").slice(0, 2)).toEqual(
    inboundTraceparent.split("-").slice(0, 2),
  );

  await expect(page.locator('meta[name="tracestate"]')).toHaveAttribute(
    "content",
    E2E_TRACESTATE,
  );
  await expect(page.locator('meta[name="baggage"]')).toHaveAttribute(
    "content",
    E2E_BAGGAGE,
  );
  const [, traceId, spanId, flags] = actual?.split("-") ?? [];
  await expect(page.locator('meta[name="sentry-trace"]')).toHaveAttribute(
    "content",
    `${traceId}-${spanId}-${Number.parseInt(flags ?? "0", 16) & 1}`,
  );
}

export { expect } from "@playwright/test";

function runPropagationHeader(
  header: "tracestate" | "baggage",
  value: string | undefined,
  fallback: string,
): string {
  const sanitized = sanitizePropagationHeaders({ [header]: value?.trim() })[header];
  return sanitized ?? fallback;
}

function isHydratedApplicationRequest(request: Request): boolean {
  if (request.resourceType() !== "fetch" && request.resourceType() !== "xhr") {
    return false;
  }
  const pathname = new URL(request.url()).pathname;
  return (
    pathname === "/graphql" ||
    pathname === "/__storefront/graphql" ||
    pathname.startsWith("/api/")
  );
}

async function assertHydratedPropagation(
  requests: readonly Promise<CapturedRequest>[],
  inboundTraceparent: string,
): Promise<void> {
  const captured = await Promise.all(requests);
  expect(captured.length, "hydrated browser outbound requests captured").toBeGreaterThan(0);
  const expectedTraceId = inboundTraceparent.split("-")[1];

  for (const request of captured) {
    const traceparent = request.headers["traceparent"];
    const tracestate = request.headers["tracestate"];
    const baggage = request.headers["baggage"];
    expect(traceparent, `${request.url} traceparent`).toMatch(TRACEPARENT_PATTERN);
    expect(traceparent?.split("-")[1], `${request.url} trace ID lineage`).toBe(
      expectedTraceId,
    );
    expect(tracestate, `${request.url} tracestate`).toBe(E2E_TRACESTATE);
    expect(normalizeBaggage(baggage), `${request.url} baggage`).toEqual(
      normalizeBaggage(E2E_BAGGAGE),
    );
  }
}

function normalizeBaggage(value: string | undefined): readonly string[] {
  if (value === undefined) return [];
  return value
    .split(",")
    .map((member) => {
      const separator = member.indexOf("=");
      if (separator <= 0) return member.trim();
      const key = member.slice(0, separator).trim();
      const [encodedValue, ...metadata] = member
        .slice(separator + 1)
        .split(";");
      return `${key}=${decodeBaggageValue(encodedValue ?? "")}${metadata
        .map((property) => {
          const propertySeparator = property.indexOf("=");
          if (propertySeparator <= 0) return property;
          return `${property.slice(0, propertySeparator)}=${decodeBaggageValue(
            property.slice(propertySeparator + 1),
          )}`;
        })
        .join(";")}`;
    })
    .sort();
}

function decodeBaggageValue(value: string): string {
  try {
    return decodeURIComponent(value.trim());
  } catch {
    return value.trim();
  }
}
