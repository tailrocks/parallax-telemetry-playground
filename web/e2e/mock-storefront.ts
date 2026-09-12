import { spawn } from "node:child_process";
import { once } from "node:events";
import {
  createServer,
  type IncomingMessage,
  type ServerResponse,
} from "node:http";

const HOST = "127.0.0.1";
const WEB_PORT = "4173";

const category = {
  id: "cat-acme-kitchen",
  tenantId: "tenant-acme",
  slug: "kitchen",
  name: "Kitchen",
};

const price = {
  id: "price-acme-widget-1-usd",
  currency: "USD",
  amountMinor: 1999,
  compareAtMinor: 2299,
  validFrom: "2026-01-03T10:00:00Z",
};

const product = {
  id: "prod-acme-widget",
  tenantId: "tenant-acme",
  slug: "everyday-widget",
  sku: "WIDGET-1",
  name: "Everyday Widget",
  description: "A dependable widget for daily tasks.",
  brand: "Acme",
  category,
  priceMinor: 1999,
  price,
  variants: [
    {
      id: "var-acme-widget-1",
      tenantId: "tenant-acme",
      productId: "prod-acme-widget",
      sku: "WIDGET-1",
      name: "Everyday Widget / Standard",
      options: '{"finish":"silver","size":"standard"}',
      price,
    },
  ],
  reviews: [],
  reviewsSlow: [],
  riskScore: null,
};

const productPage = {
  items: [product],
  page: 0,
  size: 20,
  totalElements: 1,
  totalPages: 1,
  hasNext: false,
  experience: "mock",
};

const orderSummary = {
  id: "order-acme-1001",
  order_number: "ACME-1001",
  status: "paid",
  currency: "USD",
  total_minor: 9177,
  created_at: "2026-01-26T08:21:02Z",
};

const analyticsEvent = {
  eventId: "event-1",
  tenantId: "tenant-acme",
  eventKey: "order-completed-1",
  customerId: "customer-acme-ava",
  eventName: "order.completed",
  eventVersion: 1,
  source: "payment",
  entityType: "order",
  entityId: "order-acme-1001",
  occurredAt: "2026-01-26T08:21:02Z",
  traceId: "trace-1",
  spanId: "span-1",
  properties: "{}",
};

const upstream = createServer((request, response) => {
  void handleRequest(request, response).catch(() => {
    response.statusCode = 500;
    response.end("mock storefront failure");
  });
});

async function main(): Promise<void> {
  await listen(upstream);
  const upstreamPort = addressPort(upstream);
  const child = spawn(process.execPath, ["server.ts"], {
    cwd: process.cwd(),
    env: {
      ...process.env,
      HOST,
      PORT: WEB_PORT,
      STOREFRONT_URL: `http://${HOST}:${upstreamPort}/graphql`,
    },
    stdio: "inherit",
  });
  const childExit = once(child, "exit") as Promise<
    [number | null, NodeJS.Signals | null]
  >;
  let stopping = false;
  const stop = (): void => {
    if (stopping) return;
    stopping = true;
    if (child.exitCode === null) child.kill("SIGTERM");
  };
  process.once("SIGINT", stop);
  process.once("SIGTERM", stop);

  const [code] = await childExit;
  await close(upstream);
  process.exitCode = typeof code === "number" ? code : 1;
}

async function handleRequest(
  request: IncomingMessage,
  response: ServerResponse,
): Promise<void> {
  const pathname = new URL(request.url ?? "/", `http://${HOST}`).pathname;
  if (process.env["PLAYGROUND_MOCK_DEBUG"] === "1") {
    console.error(`mock request ${request.method ?? "?"} ${request.url ?? ""}`);
  }
  if (
    request.method === "GET" &&
    (pathname === "/healthz" || pathname === "/readyz")
  ) {
    writeJson(response, 200, { status: "UP" });
    return;
  }
  if (pathname.startsWith("/api/orders")) {
    writeJson(
      response,
      200,
      pathname === "/api/orders"
        ? { tenant_id: "tenant-acme", orders: [orderSummary] }
        : {
            ...orderSummary,
            customer_id: "customer-acme-ava",
            payment_status: "captured",
            subtotal_minor: 9177,
            discount_minor: 0,
            tax_minor: 0,
            shipping_minor: 0,
            items: [
              {
                sku: "WIDGET-1",
                product_name: "Everyday Widget",
                quantity: 1,
                unit_price_minor: 9177,
                discount_minor: 0,
                line_total_minor: 9177,
              },
            ],
          },
    );
    return;
  }
  if (request.method !== "POST" || pathname !== "/graphql") {
    response.statusCode = 404;
    response.end("not found");
    return;
  }

  const payload = parseObject(await readBody(request));
  const operation = stringValue(payload?.["operationName"]);
  if (process.env["PLAYGROUND_MOCK_DEBUG"] === "1") {
    console.error(`mock operation ${operation ?? "<none>"}`);
  }
  switch (operation) {
    case "StorefrontProducts":
      writeJson(response, 200, { data: { products: productPage } });
      return;
    case "StorefrontCategories":
      writeJson(response, 200, { data: { categories: [category] } });
      return;
    case "StorefrontProduct":
      writeJson(response, 200, { data: { product } });
      return;
    case "StorefrontQuote":
      writeJson(response, 200, { data: { quote: quoteFor(payload) } });
      return;
    case "StorefrontCheckout":
      writeJson(response, 200, {
        data: {
          checkout: {
            orderId: "order-acme-1001",
            status: "paid",
            paymentStatus: "captured",
            currency: "USD",
            totalMinor: "2199",
            eventKey: "order-acme-1001:paid",
            featureVariant: "orchestrated",
          },
        },
      });
      return;
    case "StorefrontAnalytics":
      writeJson(response, 200, {
        data: {
          analyticsEvents: [analyticsEvent],
          analyticsSummary: {
            tenantId: "tenant-acme",
            eventName: null,
            eventCount: 1,
            uniqueCustomers: 1,
            firstOccurredAt: analyticsEvent.occurredAt,
            lastOccurredAt: analyticsEvent.occurredAt,
          },
        },
      });
      return;
    case "RecordStorefrontAnalytics":
      writeJson(response, 200, {
        data: {
          recordAnalytics: { eventKey: "mock-event", status: "recorded" },
        },
      });
      return;
    default:
      writeJson(response, 400, {
        errors: [{ message: "unsupported mock Storefront operation" }],
      });
  }
}

function quoteFor(payload: Readonly<Record<string, unknown>> | null) {
  const input = objectValue(objectValue(payload?.["variables"])?.["input"]);
  const items = arrayValue(input?.["items"]);
  const quantity = items.reduce((total, item) => {
    const record = objectValue(item);
    const value = record?.["quantity"];
    return total + (typeof value === "number" && Number.isSafeInteger(value) ? value : 0);
  }, 0);
  const safeQuantity = Math.max(1, quantity);
  const subtotal = 1999 * safeQuantity;
  const tax = Math.round(subtotal * 0.1);
  return {
    quoteId: "quote-mock",
    status: "QUOTE_STATUS_READY",
    lines: [
      {
        sku: "WIDGET-1",
        quantity: safeQuantity,
        unitPrice: { currencyCode: "USD", amountMinor: 1999 },
        lineTotal: { currencyCode: "USD", amountMinor: subtotal },
      },
    ],
    subtotal: { currencyCode: "USD", amountMinor: subtotal },
    discountTotal: { currencyCode: "USD", amountMinor: 0 },
    taxTotal: { currencyCode: "USD", amountMinor: tax },
    grandTotal: { currencyCode: "USD", amountMinor: subtotal + tax },
    validForSeconds: 45,
    pricingVersion: "mock-2026-01",
  };
}

async function readBody(request: IncomingMessage): Promise<unknown> {
  const chunks: Buffer[] = [];
  let length = 0;
  for await (const chunk of request) {
    const value = typeof chunk === "string" ? Buffer.from(chunk) : chunk;
    length += value.length;
    if (length > 1_048_576) throw new Error("request too large");
    chunks.push(value);
  }
  const text = Buffer.concat(chunks).toString("utf8");
  return text.length === 0 ? {} : (JSON.parse(text) as unknown);
}

function writeJson(
  response: ServerResponse,
  status: number,
  value: Readonly<Record<string, unknown>>,
): void {
  response.statusCode = status;
  response.setHeader("content-type", "application/json");
  response.end(JSON.stringify(value));
}

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function parseObject(value: unknown): Readonly<Record<string, unknown>> | null {
  return isObject(value) ? value : null;
}

function objectValue(value: unknown): Readonly<Record<string, unknown>> | null {
  return isObject(value) ? value : null;
}

function arrayValue(value: unknown): readonly unknown[] {
  return Array.isArray(value) ? value : [];
}

function stringValue(value: unknown): string | undefined {
  return typeof value === "string" ? value : undefined;
}

function listen(server: ReturnType<typeof createServer>): Promise<void> {
  return new Promise((resolve, reject) => {
    const onError = (error: Error): void => reject(error);
    server.once("error", onError);
    server.listen(0, HOST, () => {
      server.off("error", onError);
      resolve();
    });
  });
}

function addressPort(server: ReturnType<typeof createServer>): number {
  const address = server.address();
  if (address === null || typeof address === "string") {
    throw new Error("mock Storefront did not expose a TCP port");
  }
  return address.port;
}

function close(server: ReturnType<typeof createServer>): Promise<void> {
  if (!server.listening) return Promise.resolve();
  return new Promise((resolve, reject) => {
    server.close((error) => (error === undefined ? resolve() : reject(error)));
  });
}

void main();
