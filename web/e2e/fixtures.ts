import { test as base } from "@playwright/test";
import { traceparentForTest } from "./test-trace-context";

export const test = base.extend({
  page: async ({ page }, use, testInfo) => {
    const traceparent = traceparentForTest(testInfo.testId);
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
  },
});

export { expect } from "@playwright/test";
