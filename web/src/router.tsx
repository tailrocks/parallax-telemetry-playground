import { createRouter } from "@tanstack/react-router";
import { routeTree } from "./routeTree.gen";

// Single router factory. TanStack Start calls getRouter() on both server and
// client. Browser telemetry (Sentry + OTel) is wired in instrument.client.ts,
// imported by the client entry, so it initializes once before hydration.
export function getRouter() {
  return createRouter({
    routeTree,
    defaultPreload: "intent",
    scrollRestoration: true,
  });
}

declare module "@tanstack/react-router" {
  interface Register {
    router: ReturnType<typeof getRouter>;
  }
}
