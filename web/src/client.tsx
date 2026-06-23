import { StrictMode, startTransition } from "react";
import { hydrateRoot } from "react-dom/client";
import { StartClient } from "@tanstack/react-start/client";
import { initBrowserTelemetry } from "./instrument.client";

// Custom client entry (auto-detected by TanStack Start in place of the default).
// Telemetry initializes BEFORE hydration so the Sentry + OTel providers are
// installed when the first fetch/route-change fires.
initBrowserTelemetry();

startTransition(() => {
  hydrateRoot(
    document,
    <StrictMode>
      <StartClient />
    </StrictMode>,
  );
});
