import { expect, test } from "@playwright/test";

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
