import { StrictMode, startTransition } from "react";
import { hydrateRoot } from "react-dom/client";
import { StartClient } from "@tanstack/react-start/client";
import { context } from "@opentelemetry/api";
import { initBrowserTelemetry } from "./instrument.client";

// Custom client entry (auto-detected by TanStack Start in place of the default).
// Telemetry initializes BEFORE hydration so the Sentry + OTel providers are
// installed when the first fetch/route-change fires.
const browserContext = initBrowserTelemetry();

startTransition(() => {
  const hydrate = () =>
    hydrateRoot(
      document,
      <StrictMode>
        <StartClient />
      </StrictMode>,
    );
  if (browserContext === undefined) {
    hydrate();
  } else {
    context.with(browserContext, hydrate);
  }
});
