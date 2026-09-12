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
const BAGGAGE_KEY_PATTERN =
  /^[a-z0-9][a-z0-9._-]{0,255}(?:@[a-z0-9][a-z0-9._-]{0,13})?$/;
const UUID_PATTERN =
  /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;

type BaggageMember = Readonly<{
  key: string;
  value: string;
  metadata?: string | undefined;
}>;

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
    expect(
      baggageValidationError(baggage),
      `${request.url} baggage`,
    ).toBeUndefined();
  }
}

export function baggageValidationError(
  actual: string | undefined,
  expected: string = E2E_BAGGAGE,
): string | undefined {
  let actualMembers: readonly BaggageMember[];
  let expectedMembers: readonly BaggageMember[];
  try {
    actualMembers = parseBaggage(actual);
    expectedMembers = parseBaggage(expected);
  } catch (error) {
    return error instanceof Error ? error.message : String(error);
  }

  const expectedStaticMembers = expectedMembers.filter(
    ({ key }) => key !== "session.id",
  );
  const actualByKey = new Map(actualMembers.map((member) => [member.key, member]));

  for (const expectedMember of expectedStaticMembers) {
    const actualMember = actualByKey.get(expectedMember.key);
    if (actualMember === undefined) {
      return `missing canonical member ${expectedMember.key}`;
    }
    if (
      actualMember.value !== expectedMember.value ||
      actualMember.metadata !== expectedMember.metadata
    ) {
      return `wrong canonical member ${expectedMember.key}`;
    }
  }

  const expectedKeys = new Set(expectedStaticMembers.map(({ key }) => key));
  for (const actualMember of actualMembers) {
    if (expectedKeys.has(actualMember.key)) continue;
    if (actualMember.key !== "session.id") {
      return `unexpected member ${actualMember.key}`;
    }
    if (actualMember.metadata !== undefined) {
      return "session.id must not have metadata";
    }
    if (!UUID_PATTERN.test(actualMember.value)) {
      return "session.id must be a UUIDv4";
    }
  }

  return undefined;
}

function parseBaggage(value: string | undefined): readonly BaggageMember[] {
  if (value === undefined || value.trim().length === 0) return [];

  const seen = new Set<string>();
  return value.split(",").map((rawMember, index) => {
    const member = rawMember.trim();
    const separator = member.indexOf("=");
    if (separator <= 0) {
      throw new Error(`invalid baggage member at index ${index}`);
    }

    const key = member.slice(0, separator).trim();
    if (!BAGGAGE_KEY_PATTERN.test(key)) {
      throw new Error(`invalid baggage key ${key}`);
    }
    if (seen.has(key)) {
      throw new Error(`duplicate baggage key ${key}`);
    }
    seen.add(key);

    const valueAndMetadata = member.slice(separator + 1).split(";");
    const encodedValue = valueAndMetadata.shift()?.trim() ?? "";
    if (encodedValue.length === 0) {
      throw new Error(`empty baggage value for ${key}`);
    }
    const metadata = valueAndMetadata.join(";").trim();
    if (valueAndMetadata.length > 0 && metadata.length === 0) {
      throw new Error(`invalid baggage metadata for ${key}`);
    }

    return {
      key,
      value: decodeBaggageValue(encodedValue),
      ...(metadata.length > 0 ? { metadata } : {}),
    };
  });
}

function decodeBaggageValue(value: string): string {
  try {
    return decodeURIComponent(value.trim());
  } catch {
    throw new Error(`invalid encoded baggage value ${value}`);
  }
}
