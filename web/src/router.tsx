import { createRouter } from "@tanstack/react-router";
import { routeTree } from "./routeTree.gen";

// Single router factory. TanStack Start calls getRouter() on both server and
// client. Browser telemetry (Sentry + OTel) is wired in instrument.client.ts,
// imported by the client entry, so it initializes once before hydration.
export function getRouter() {
  const router = createRouter({
    routeTree,
    defaultPreload: "intent",
    scrollRestoration: true,
  });
  if (typeof document !== "undefined") {
    let lastPathname = "";
    router.subscribe("onResolved", () => {
      const pathname = router.state.location.pathname;
      if (pathname === lastPathname) return;
      lastPathname = pathname;
      void import("./telemetry").then(({ trackScreen }) => trackScreen(pathname));
    });
  }
  return router;
}

declare module "@tanstack/react-router" {
  interface Register {
    router: ReturnType<typeof getRouter>;
  }
}
