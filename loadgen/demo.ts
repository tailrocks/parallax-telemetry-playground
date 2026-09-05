// Long-running ambient Storefront traffic. Run by the Compose `demo` profile.
import { sleep } from "k6";
import type { Options } from "k6/options";

// @ts-expect-error k6 resolves TypeScript module paths with the explicit extension.
import { runStorefrontIteration } from "./checkout.ts";

export const options: Options = {
  vus: 2,
  duration: "24h",
  thresholds: { checks: ["rate==1"] },
};

export default function demoLoad(): void {
  sleep(runStorefrontIteration());
}
