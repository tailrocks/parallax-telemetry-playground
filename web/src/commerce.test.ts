import { afterEach, describe, expect, test, vi } from "vitest";
import {
  SAFE_WEB_ERROR_MESSAGES,
  safeWebError,
} from "./error-contract";
import {
  CommerceApiError,
  errorMessageForUser,
  fetchProduct,
  fetchProducts,
  fetchOrders,
  fetchQuote,
  formatMoney,
  submitCheckout,
} from "./commerce";

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("commerce API boundary", () => {
  test("formats server-owned minor units without inventing a price", () => {
    expect(formatMoney(1999, "USD")).toBe("$19.99");
    expect(formatMoney(null, "USD")).toBe("Price unavailable");
  });

  test("parses the storefront REST order projection", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(
        async () =>
          new Response(
            JSON.stringify({
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
            { status: 200, headers: { "content-type": "application/json" } },
          ),
      ),
    );

    await expect(
      fetchOrders({ tenantId: "tenant-acme", customerId: "customer-acme-ava" }),
    ).resolves.toEqual([
      expect.objectContaining({ id: "order-acme-1001", totalMinor: 9177 }),
    ]);
  });

  test("parses the itemized GraphQL quote contract", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(
        async () =>
          new Response(
            JSON.stringify({
              data: {
                quote: {
                  quoteId: "quote-1",
                  status: "QUOTE_STATUS_READY",
                  lines: [
                    {
                      sku: "WIDGET-1",
                      quantity: 2,
                      unitPrice: { currencyCode: "USD", amountMinor: 1999 },
                      lineTotal: { currencyCode: "USD", amountMinor: 3998 },
                    },
                  ],
                  subtotal: { currencyCode: "USD", amountMinor: 3998 },
                  discountTotal: { currencyCode: "USD", amountMinor: 0 },
                  taxTotal: { currencyCode: "USD", amountMinor: 400 },
                  grandTotal: { currencyCode: "USD", amountMinor: 4398 },
                  validForSeconds: 45,
                  pricingVersion: "seed-2026-01",
                },
              },
            }),
            { status: 200, headers: { "content-type": "application/json" } },
          ),
      ),
    );

    await expect(
      fetchQuote({
        items: [{ sku: "WIDGET-1", quantity: 2 }],
        tenantId: "tenant-acme",
        customerId: "customer-acme-ava",
        currencyCode: "USD",
      }),
    ).resolves.toEqual(
      expect.objectContaining({
        quoteId: "quote-1",
        grandTotal: { currencyCode: "USD", amountMinor: 4398 },
      }),
    );
  });

  test("submits checkout through the Storefront GraphQL mutation", async () => {
    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      expect(new URL(input.toString()).pathname).toBe("/graphql");
      expect(init?.method).toBe("POST");
      const request = JSON.parse(String(init?.body)) as {
        operationName?: unknown;
        query?: unknown;
        variables?: { input?: Record<string, unknown> };
      };
      expect(request.operationName).toBe("StorefrontCheckout");
      expect(request.query).toContain("mutation StorefrontCheckout");
      expect(request.variables?.input).toMatchObject({
        tenantId: "tenant-acme",
        customerId: "customer-acme-ava",
        items: [{ sku: "WIDGET-1", quantity: 1 }],
      });
      return new Response(
        JSON.stringify({
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
        }),
        { status: 200, headers: { "content-type": "application/json" } },
      );
    });
    vi.stubGlobal("fetch", fetchMock);

    await expect(
      submitCheckout({
        items: [{ sku: "WIDGET-1", quantity: 1 }],
        tenantId: "tenant-acme",
        customerId: "customer-acme-ava",
        currencyCode: "USD",
        paymentMethodToken: "tok_visa",
      }),
    ).resolves.toEqual(
      expect.objectContaining({
        status: "paid",
        orderId: "order-acme-1001",
        paymentStatus: "captured",
        totalMinor: 2199,
        featureVariant: "orchestrated",
      }),
    );
    expect(fetchMock).toHaveBeenCalledOnce();
  });

  test("rejects a GraphQL response with no data", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(
        async () =>
          new Response(
            JSON.stringify({
              data: null,
              errors: [{ message: "catalog down" }],
            }),
            {
              status: 200,
              headers: { "content-type": "application/json" },
            },
          ),
      ),
    );

    await expect(
      fetchQuote({
        items: [{ sku: "WIDGET-1", quantity: 1 }],
        tenantId: "tenant-acme",
        customerId: "customer-acme-ava",
        currencyCode: "USD",
      }),
    ).rejects.toMatchObject({
      code: "graphql_failure",
      message: SAFE_WEB_ERROR_MESSAGES.graphql_failure,
    });
  });

  test("rejects partial GraphQL data instead of decoding a null result", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(
        async () =>
          new Response(
            JSON.stringify({
              data: { quote: null },
              errors: [{ message: "pricing unavailable" }],
            }),
            {
              status: 200,
              headers: { "content-type": "application/json" },
            },
          ),
      ),
    );

    await expect(
      fetchQuote({
        items: [{ sku: "WIDGET-1", quantity: 1 }],
        tenantId: "tenant-acme",
        customerId: "customer-acme-ava",
        currencyCode: "USD",
      }),
    ).rejects.toMatchObject({
      code: "response_invalid",
      message: SAFE_WEB_ERROR_MESSAGES.response_invalid,
    });
  });

  test("preserves a valid nullable product result when GraphQL reports an error", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(
        async () =>
          new Response(
            JSON.stringify({
              data: { product: null },
              errors: [{ message: "product lookup failed" }],
            }),
            { status: 200, headers: { "content-type": "application/json" } },
          ),
      ),
    );

    await expect(fetchProduct("MISSING-SKU", "tenant-acme")).resolves.toBeNull();
  });

  test("preserves valid partial GraphQL data while retaining degraded errors", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(
        async () =>
          new Response(
            JSON.stringify({
              data: {
                product: {
                  id: "product-1",
                  tenantId: "tenant-acme",
                  slug: "widget-1",
                  sku: "WIDGET-1",
                  name: "Widget One",
                  description: "A test widget",
                  brand: null,
                  category: {
                    id: "category-1",
                    tenantId: "tenant-acme",
                    slug: "widgets",
                    name: "Widgets",
                  },
                  priceMinor: 1999,
                  price: null,
                  variants: [],
                  reviews: [],
                  reviewsSlow: [],
                  riskScore: null,
                },
              },
              errors: [
                {
                  message: "reviews service degraded",
                  path: ["product", "reviewsSlow"],
                },
              ],
            }),
            { status: 200, headers: { "content-type": "application/json" } },
          ),
      ),
    );

    await expect(fetchProduct("WIDGET-1", "tenant-acme")).resolves.toMatchObject({
      sku: "WIDGET-1",
      name: "Widget One",
    });
  });

  test("maps upstream text to stable safe errors", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(
        async () =>
          new Response(
            JSON.stringify({
              message: "SQL failed for card=4111111111111111 email=ava@example.com",
            }),
            { status: 502, headers: { "content-type": "application/json" } },
          ),
      ),
    );

    await expect(
      fetchOrders({ tenantId: "tenant-acme", customerId: "customer-acme-ava" }),
    ).rejects.toMatchObject({
      code: "http_failure",
      message: SAFE_WEB_ERROR_MESSAGES.http_failure,
    });
    expect(errorMessageForUser(new Error("provider token leaked"))).toBe(
      SAFE_WEB_ERROR_MESSAGES.unexpected,
    );
    expect(safeWebError(new Error("raw SQL/card/email"))).toMatchObject({
      name: "unexpected",
      message: SAFE_WEB_ERROR_MESSAGES.unexpected,
    });
    expect(new CommerceApiError("http", "test", "http_failure").message).toBe(
      SAFE_WEB_ERROR_MESSAGES.http_failure,
    );
  });

  test("sends category, typed sort, and page controls to the server", async () => {
    const fetchMock = vi.fn(
      async (input: RequestInfo | URL, init?: RequestInit) => {
        expect(new URL(input.toString()).pathname).toBe("/graphql");
        const request = JSON.parse(String(init?.body)) as {
          query?: unknown;
          variables?: Record<string, unknown>;
        };
        expect(request.query).toContain("$sort: ProductSort");
        expect(request.query).toContain("category: $category");
        expect(request.query).toContain("sort: $sort");
        expect(request.variables).toMatchObject({
          category: "kitchen",
          sort: "PRICE_ASC",
          page: 1,
          size: 20,
        });
        return new Response(
          JSON.stringify({
            data: {
              products: {
                items: [],
                page: 1,
                size: 20,
                totalElements: 21,
                totalPages: 2,
                hasNext: false,
                experience: "standard",
              },
            },
          }),
          { status: 200, headers: { "content-type": "application/json" } },
        );
      },
    );
    vi.stubGlobal("fetch", fetchMock);

    await expect(
      fetchProducts({
        tenantId: "tenant-acme",
        category: "kitchen",
        sort: "PRICE_ASC",
        page: 1,
        size: 20,
      }),
    ).resolves.toMatchObject({
      page: 1,
      totalElements: 21,
      totalPages: 2,
    });
  });
});
