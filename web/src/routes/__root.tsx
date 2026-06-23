/// <reference types="vite/client" />
import { HeadContent, Scripts, createRootRoute } from "@tanstack/react-router";
import type { ReactNode } from "react";
import { mintTraceparent } from "../traceparent";

// Root document. `head()` runs during SSR and emits a fresh <meta
// name="traceparent">; the browser OTel document-load instrumentation reads it
// so the first-paint span joins the same distributed trace.
export const Route = createRootRoute({
  head: () => ({
    meta: [
      { charSet: "utf-8" },
      { name: "viewport", content: "width=device-width, initial-scale=1" },
      { title: "Telemetry Playground" },
      { name: "traceparent", content: mintTraceparent() },
    ],
  }),
  shellComponent: RootDocument,
});

function RootDocument({ children }: { children: ReactNode }) {
  return (
    <html lang="en">
      <head>
        <HeadContent />
      </head>
      <body>
        {children}
        <Scripts />
      </body>
    </html>
  );
}
