import { expect, expectSsrTraceparent, test } from "./fixtures";

const category = {
  id: "cat-acme-kitchen",
  tenantId: "tenant-acme",
  slug: "kitchen",
  name: "Kitchen",
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
  price: {
    id: "price-acme-widget-1-usd",
    currency: "USD",
    amountMinor: 1999,
    compareAtMinor: 2299,
    validFrom: "2026-01-03T10:00:00Z",
  },
  variants: [
    {
      id: "var-acme-widget-1",
      tenantId: "tenant-acme",
      productId: "prod-acme-widget",
      sku: "WIDGET-1",
      name: "Everyday Widget / Standard",
      options: '{"finish":"silver","size":"standard"}',
      price: {
        id: "price-acme-widget-1-usd",
        currency: "USD",
        amountMinor: 1999,
        compareAtMinor: 2299,
        validFrom: "2026-01-03T10:00:00Z",
      },
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
  experience: "standard",
};

test("mock UI contract: home and catalog render live-shaped Catalog data", async ({
  page,
  testTraceparent,
}) => {
  await page.route("**/graphql", async (route) => {
    const query = route.request().postData() ?? "";
    if (query.includes("StorefrontProducts")) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ data: { products: productPage } }),
      });
      return;
    }
    if (query.includes("StorefrontCategories")) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ data: { categories: [category] } }),
      });
      return;
    }
    await route.continue();
  });

  await page.goto("/");
  await expectSsrTraceparent(page, testTraceparent);
  await expect(
    page.getByRole("heading", { name: "Commerce, with a trace attached." }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Everyday Widget" }),
  ).toBeVisible();
  await page.getByRole("link", { name: "Browse catalog" }).first().click();
  await expect(
    page.getByRole("heading", { name: "Browse the real assortment." }),
  ).toBeVisible();
  await expect(page.getByText("WIDGET-1").first()).toBeVisible();
});

test("mock UI contract: catalog controls remain server-owned across sort, category, and pages", async ({
  page,
}) => {
  const productRequests: Array<Readonly<{
    category: unknown;
    sort: unknown;
    page: unknown;
  }>> = [];

  await page.route("**/graphql", async (route) => {
    const body = JSON.parse(route.request().postData() ?? "{}") as {
      operationName?: unknown;
      variables?: Record<string, unknown>;
    };
    if (body.operationName === "StorefrontProducts") {
      const variables = body.variables ?? {};
      productRequests.push({
        category: variables["category"],
        sort: variables["sort"],
        page: variables["page"],
      });
      const currentPage =
        typeof variables["page"] === "number" ? variables["page"] : 0;
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          data: {
            products: {
              ...productPage,
              page: currentPage,
              size: 20,
              totalElements: 41,
              totalPages: 3,
              hasNext: currentPage < 2,
            },
          },
        }),
      });
      return;
    }
    if (body.operationName === "StorefrontCategories") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ data: { categories: [category] } }),
      });
      return;
    }
    await route.continue();
  });

  await page.goto("/catalog");
  await expect(page.getByText("Page 1 of 3", { exact: true })).toBeVisible();
  await expect
    .poll(() => productRequests.at(-1)?.category)
    .toBeNull();

  await page.locator("#catalog-sort").selectOption("PRICE_ASC");
  await expect
    .poll(() => productRequests.at(-1)?.sort)
    .toBe("PRICE_ASC");
  await expect(page.getByText("Page 1 of 3", { exact: true })).toBeVisible();

  await page.getByRole("button", { name: "Kitchen", exact: true }).click();
  await expect
    .poll(() => productRequests.at(-1)?.category)
    .toBe("kitchen");
  await expect
    .poll(() => productRequests.at(-1)?.page)
    .toBe(0);

  await page.getByRole("button", { name: "Next", exact: true }).click();
  await expect
    .poll(() => productRequests.at(-1)?.page)
    .toBe(1);
  await expect(page.getByText("Page 2 of 3", { exact: true })).toBeVisible();
});

test("mock UI contract: catalog to cart to checkout uses the Storefront GraphQL mutation", async ({
  page,
}) => {
  await page.route("**/graphql", async (route) => {
    const query = route.request().postData() ?? "";
    if (query.includes("StorefrontProducts")) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ data: { products: productPage } }),
      });
      return;
    }
    if (query.includes("StorefrontCategories")) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ data: { categories: [category] } }),
      });
      return;
    }
    if (query.includes("RecordStorefrontAnalytics")) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          data: {
            recordAnalytics: { eventKey: "web-event-1", status: "recorded" },
          },
        }),
      });
      return;
    }
    if (query.includes("StorefrontProduct")) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ data: { product } }),
      });
      return;
    }
    if (query.includes("StorefrontQuote")) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          data: {
            quote: {
              quoteId: "quote-1",
              status: "QUOTE_STATUS_READY",
              lines: [
                {
                  sku: "WIDGET-1",
                  quantity: 1,
                  unitPrice: { currencyCode: "USD", amountMinor: 1999 },
                  lineTotal: { currencyCode: "USD", amountMinor: 1999 },
                },
              ],
              subtotal: { currencyCode: "USD", amountMinor: 1999 },
              discountTotal: { currencyCode: "USD", amountMinor: 0 },
              taxTotal: { currencyCode: "USD", amountMinor: 200 },
              grandTotal: { currencyCode: "USD", amountMinor: 2199 },
              validForSeconds: 45,
              pricingVersion: "seed-2026-01",
            },
          },
        }),
      });
      return;
    }
    if (query.includes("StorefrontCheckout")) {
      expect(query).toContain("mutation StorefrontCheckout");
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          data: {
            checkout: {
              orderId: "order-acme-test",
              status: "paid",
              paymentStatus: "captured",
              currency: "USD",
              totalMinor: "2199",
              eventKey: "order-acme-test:paid",
              featureVariant: "orchestrated",
            },
          },
        }),
      });
      return;
    }
    await route.continue();
  });

  await page.goto("/catalog");
  await page.getByRole("button", { name: "Add", exact: true }).click();
  await page.getByRole("link", { name: /^Cart/ }).click();
  await expect(
    page.getByRole("heading", { name: "Your working set." }),
  ).toBeVisible();
  await page.getByRole("link", { name: "Review checkout" }).click();
  await expect(
    page.getByRole("heading", { name: "Review before the boundary." }),
  ).toBeVisible();
  await expect(page.getByText("$21.99")).toBeVisible();
  await page.getByRole("button", { name: "Place order" }).click();
  await expect(
    page.getByText("Order accepted and payment captured"),
  ).toBeVisible();
  await expect(page.getByRole("link", { name: "Track order" })).toHaveAttribute(
    "href",
    "/orders/order-acme-test",
  );
});

test("mock UI contract: orders reads durable status from the storefront REST projection", async ({
  page,
}) => {
  await page.route("**/api/orders?*", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        tenant_id: "tenant-acme",
        orders: [
          {
            id: "order-acme-1001",
            order_number: "ACME-1001",
            status: "paid",
            currency: "USD",
            total_minor: 9177,
            created_at: 1769415600,
          },
        ],
      }),
    });
  });
  await page.goto("/orders");
  await expect(
    page.getByRole("heading", { name: "Follow the order after checkout." }),
  ).toBeVisible();
  await expect(page.getByText("ACME-1001")).toBeVisible();
  await expect(
    page.getByText("Viewing Ava Chen’s tenant-acme orders"),
  ).toBeVisible();
});

test("mock UI contract: analytics reads ClickHouse-backed events through storefront GraphQL", async ({
  page,
}) => {
  await page.route("**/graphql", async (route) => {
    const query = route.request().postData() ?? "";
    if (!query.includes("StorefrontAnalytics")) {
      await route.continue();
      return;
    }
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        data: {
          analyticsEvents: [
            {
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
            },
          ],
          analyticsSummary: {
            tenantId: "tenant-acme",
            eventName: null,
            eventCount: 1,
            uniqueCustomers: 1,
            firstOccurredAt: "2026-01-26T08:21:02Z",
            lastOccurredAt: "2026-01-26T08:21:02Z",
          },
        },
      }),
    });
  });
  await page.goto("/analytics");
  await expect(
    page.getByRole("heading", { name: "See what the journey emitted." }),
  ).toBeVisible();
  await expect(page.getByText("order.completed")).toBeVisible();
  await expect(
    page.getByText("ClickHouse", { exact: false }).first(),
  ).toBeVisible();
});
