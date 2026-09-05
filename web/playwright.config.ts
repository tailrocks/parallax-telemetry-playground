import { defineConfig, devices } from "@playwright/test";

const composeMode = process.env["PLAYGROUND_COMPOSE_E2E"] === "1";
const mockMode = process.env["PLAYGROUND_MOCK_E2E"] === "1";
const composeBaseURL =
  process.env["PLAYGROUND_COMPOSE_BASE_URL"]?.trim() || "http://localhost:5173";

if (composeMode === mockMode) {
  throw new Error(
    "Choose exactly one explicit browser gate: `PLAYGROUND_COMPOSE_E2E=1` for Compose or `PLAYGROUND_MOCK_E2E=1` for the mocked UI contract suite.",
  );
}

export default defineConfig({
  testDir: "./e2e",
  testMatch: composeMode ? "**/compose.smoke.spec.ts" : "**/journey.spec.ts",
  testIgnore: composeMode ? undefined : ["**/compose.smoke.spec.ts"],
  timeout: 30_000,
  retries: Number(process.env.PLAYWRIGHT_RETRIES ?? (process.env.CI ? 1 : 0)),
  reporter: [
    ["list"],
    ["html", { open: "never" }],
    ["./e2e/telemetry-reporter.ts"],
  ],
  use: {
    baseURL: composeMode ? composeBaseURL : "http://127.0.0.1:4173",
    trace: "on-first-retry",
  },
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],
  // Compose owns the browser server in explicit smoke mode. Normal E2E keeps
  // its isolated local build/server lifecycle.
  webServer: composeMode
    ? undefined
    : {
        command: "bun run build && bun e2e/mock-storefront.ts",
        url: "http://127.0.0.1:4173/healthz",
        timeout: 120_000,
        reuseExistingServer: false,
      },
});
