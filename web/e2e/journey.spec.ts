import { expect, test } from "./fixtures";

test("home to checkout journey preserves the user-facing flow", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Telemetry Playground" })).toBeVisible();
  await page.getByRole("link", { name: "checkout journey" }).click();
  await expect(page.getByRole("heading", { name: "Checkout" })).toBeVisible();
  await expect(page.getByRole("button", { name: "submit checkout" })).toBeVisible();
});

test("propagation-break journey explains its intentional disconnected trace", async ({ page }) => {
  await page.goto("/checkout?nopropagate=1");
  await expect(page.getByText("Propagation break mode:")).toBeVisible();
  await expect(page.getByRole("link", { name: "open propagation-break variant" })).toBeVisible();
});

test("home to orders journey submits a batched order [batch]", async ({ page }) => {
  await page.route("**/order?**", async (route) => {
    await route.fulfill({
      status: 202,
      contentType: "application/json",
      body: JSON.stringify({ status: "queued" }),
    });
  });
  await page.goto("/");
  await page.getByRole("link", { name: "orders journey" }).click();
  await expect(page.getByRole("heading", { name: "Orders" })).toBeVisible();
  await page.getByRole("checkbox", { name: "batch consumer" }).check();
  await page.getByRole("button", { name: "submit order" }).click();
  await expect(page.getByText('202: {"status":"queued"}')).toBeVisible();
});

test("forced RUM error preserves its backend failure", async ({ page }) => {
  await page.route("**/checkout?sku=RUM-ERROR&quantity=1&fail=1", async (route) => {
    await route.fulfill({ status: 502, body: "payment failed" });
  });
  await page.goto("/");
  await page.getByRole("button", { name: "break (RUM error)" }).click();
  await expect(page.getByText(/intentional RUM error after backend 502/)).toBeVisible();
});
