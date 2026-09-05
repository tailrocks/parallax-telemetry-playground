import { type Page } from "@playwright/test";
import { expect, expectSsrTraceparent, test } from "./fixtures";

test.describe("Compose-backed commerce", () => {
  test("browses, quotes, checks out, reads status, and queries analytics", async ({
    page,
    testTraceparent,
  }) => {
    test.setTimeout(120_000);

    await page.goto("/catalog");
    await expectSsrTraceparent(page, testTraceparent);
    await expect(
      page.getByRole("heading", { name: "Browse the real assortment." }),
    ).toBeVisible();
    await expect(page.getByText("Page 1 of 2", { exact: true })).toBeVisible();

    const nextPageResponse = waitForStorefrontOperation(
      page,
      "StorefrontProducts",
    );
    await page.getByRole("button", { name: "Next", exact: true }).click();
    expect((await nextPageResponse).ok()).toBe(true);
    await expect(page.getByText("Page 2 of 2", { exact: true })).toBeVisible();

    const previousPageResponse = waitForStorefrontOperation(
      page,
      "StorefrontProducts",
    );
    await page.getByRole("button", { name: "Previous", exact: true }).click();
    expect((await previousPageResponse).ok()).toBe(true);
    await expect(page.getByText("Page 1 of 2", { exact: true })).toBeVisible();

    await page.reload();
    await expect(
      page.getByRole("heading", { name: "Browse the real assortment." }),
    ).toBeVisible();

    const productCard = page
      .locator(".product-card")
      .filter({ hasText: "WIDGET-1" })
      .first();
    await expect(productCard).toBeVisible();
    await productCard.getByRole("button", { name: "Add", exact: true }).click();

    await page.getByRole("link", { name: /^Cart/ }).click();
    await expect(
      page.getByRole("heading", { name: "Your working set." }),
    ).toBeVisible();

    const quoteResponse = waitForStorefrontOperation(page, "StorefrontQuote");
    await page.getByRole("link", { name: "Review checkout" }).click();
    expect((await quoteResponse).ok()).toBe(true);
    await expect(
      page.getByRole("heading", { name: "Review before the boundary." }),
    ).toBeVisible();
    await expect(
      page.getByRole("heading", { name: "Authoritative quote" }),
    ).toBeVisible();
    await expect(
      page.getByText("QUOTE STATUS READY", { exact: true }),
    ).toBeVisible();
    await expect(page.getByText("WIDGET-1 × 1", { exact: true })).toBeVisible();

    const checkoutResponse = waitForStorefrontOperation(
      page,
      "StorefrontCheckout",
    );
    await page.getByRole("button", { name: "Place order" }).click();
    expect((await checkoutResponse).ok()).toBe(true);
    await expect(
      page.getByText(
        /Order accepted and payment captured|Order is pending payment confirmation/,
      ),
    ).toBeVisible({ timeout: 45_000 });

    const trackOrder = page.getByRole("link", { name: "Track order" });
    await expect(trackOrder).toBeVisible();
    await expect(trackOrder).toHaveAttribute("href", /^\/orders\/[^/]+$/);
    const orderHref = await trackOrder.getAttribute("href");
    const orderId = orderHref?.split("/").at(-1);
    expect(orderId).toBeTruthy();
    const orderNumber = orderId?.replace(/^order-/, "");
    expect(orderNumber).toBeTruthy();

    const orderDetailResponse = page.waitForResponse((response) => {
      const request = response.request();
      return (
        new URL(response.url()).pathname.startsWith("/api/orders/") &&
        request.method() === "GET"
      );
    });
    await trackOrder.click();
    expect((await orderDetailResponse).ok()).toBe(true);
    await expect(page).toHaveURL(/\/orders\/[^/?]+$/);
    await expect(
      page.getByRole("heading", { name: "Order status" }),
    ).toBeVisible();
    await expect(
      page.getByRole("heading", { name: "Journey timeline" }),
    ).toBeVisible();
    await expect(
      page.getByText("Order recorded", { exact: true }),
    ).toBeVisible();
    await expect(
      page.getByText(`acme-${orderNumber}`, { exact: true }),
    ).toBeVisible();
    await expect(
      page.getByText("WIDGET-1 · quantity 1", { exact: true }),
    ).toBeVisible();
    await expect(page.locator(".summary-card")).toContainText("$19.99");

    let observedStatus = "";
    for (let attempt = 0; attempt < 30; attempt += 1) {
      observedStatus =
        (await page.locator(".status-pill").first().textContent())
          ?.trim()
          .toLowerCase() ?? "";
      if (["processing", "shipped", "delivered"].includes(observedStatus)) {
        break;
      }
      const refreshResponse = page.waitForResponse((response) => {
        const request = response.request();
        return (
          new URL(response.url()).pathname === `/api/orders/${orderId}` &&
          request.method() === "GET"
        );
      });
      await page.getByRole("button", { name: "Refresh" }).click();
      expect((await refreshResponse).ok()).toBe(true);
      if (attempt < 29) await page.waitForTimeout(1_000);
    }
    expect(observedStatus).toMatch(/processing|shipped|delivered/);
    await expect(
      page.locator(".timeline-step-complete").filter({
        hasText: "Fulfillment queued",
      }),
    ).toBeVisible();

    const ordersResponse = page.waitForResponse((response) => {
      const request = response.request();
      return (
        new URL(response.url()).pathname === "/api/orders" &&
        request.method() === "GET"
      );
    });
    await page.getByRole("link", { name: "All orders" }).click();
    expect((await ordersResponse).ok()).toBe(true);
    await expect(
      page.getByRole("heading", { name: "Follow the order after checkout." }),
    ).toBeVisible();
    await expect(page.locator(".order-card").first()).toBeVisible();

    const analyticsResponse = waitForStorefrontOperation(
      page,
      "StorefrontAnalytics",
    );
    await page.getByRole("link", { name: "Analytics" }).click();
    expect((await analyticsResponse).ok()).toBe(true);
    await expect(
      page.getByRole("heading", { name: "See what the journey emitted." }),
    ).toBeVisible();
    await expect(
      page.getByText("source: ClickHouse", { exact: false }),
    ).toBeVisible();

    await page.locator("#event-filter").fill("order.paid");
    const filteredAnalyticsResponse = waitForStorefrontOperation(
      page,
      "StorefrontAnalytics",
    );
    await page.getByRole("button", { name: "Apply filter" }).click();
    expect((await filteredAnalyticsResponse).ok()).toBe(true);

    const events = page.locator('[aria-label="Analytics events"]');
    const matchingEvent = events
      .locator(".event-card")
      .filter({ hasText: orderId ?? "" })
      .first();
    for (let attempt = 0; attempt < 30; attempt += 1) {
      if (await events.isVisible()) {
        if (await matchingEvent.isVisible()) break;
      }
      if (attempt < 29) await page.waitForTimeout(1_000);
      const refreshResponse = waitForStorefrontOperation(
        page,
        "StorefrontAnalytics",
      );
      await page.getByRole("button", { name: "Refresh events" }).click();
      expect((await refreshResponse).ok()).toBe(true);
    }
    await expect(events).toBeVisible();
    await expect(matchingEvent).toBeVisible();
    await expect(matchingEvent).toContainText(orderId ?? "");
  });
});

function waitForStorefrontOperation(page: Page, operation: string) {
  return page.waitForResponse((response) => {
    const request = response.request();
    return (
      ["/graphql", "/__storefront/graphql"].includes(
        new URL(response.url()).pathname,
      ) &&
      request.method() === "POST" &&
      (request.postData() ?? "").includes(operation)
    );
  });
}
