import { createFileRoute } from "@tanstack/react-router";

// Same-origin OTLP/HTTP proxy (spec §8). The browser exporter POSTs spans to
// `/v1/traces`; this server route forwards them to Rotel server-side. Keeping it
// same-origin avoids collector CORS, hides the collector endpoint from the
// browser, and is where ingest auth/headers would be added.
//
// ROTEL_OTLP_HTTP_ENDPOINT is the Rotel HTTP receiver base (no trailing
// `/v1/traces`). Defaults: in Docker the web server reaches Rotel at
// `http://rotel:4318`; for host dev, `http://localhost:4318`.
const rotelBase =
  process.env["ROTEL_OTLP_HTTP_ENDPOINT"]?.replace(/\/+$/, "") ??
  "http://localhost:4318";

export const Route = createFileRoute("/v1/traces")({
  server: {
    handlers: {
      POST: async ({ request }) => {
        const body = await request.arrayBuffer();
        try {
          const upstream = await fetch(`${rotelBase}/v1/traces`, {
            method: "POST",
            headers: {
              "content-type":
                request.headers.get("content-type") ?? "application/json",
            },
            body,
          });
          // Mirror upstream status; OTLP clients only need 2xx vs error.
          return new Response(await upstream.arrayBuffer(), {
            status: upstream.status,
            headers: {
              "content-type":
                upstream.headers.get("content-type") ?? "application/json",
            },
          });
        } catch (err) {
          // Rotel down: drop the batch but don't surface a hard error to the
          // page (telemetry must never break the app).
          console.error("[/v1/traces] forward to Rotel failed:", err);
          return new Response(null, { status: 202 });
        }
      },
    },
  },
});
