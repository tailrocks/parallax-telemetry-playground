import { test as base } from "@playwright/test";
import { traceparentForRunningTest } from "./test-trace-context";

export const test = base.extend({
  page: async ({ page }, use, testInfo) => {
    const traceparent = await traceparentForRunningTest(testInfo.testId);
    await page.setExtraHTTPHeaders({ traceparent });
    await page.addInitScript((parent) => {
      const install = () => {
        const meta = document.querySelector<HTMLMetaElement>('meta[name="traceparent"]');
        if (!meta) return false;
        meta.content = parent;
        return true;
      };
      if (install()) return;
      const observer = new MutationObserver(() => {
        if (install()) observer.disconnect();
      });
      observer.observe(document, { childList: true, subtree: true });
    }, traceparent);
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
  },
});

export { expect } from "@playwright/test";
