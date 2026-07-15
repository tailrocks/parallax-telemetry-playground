import { createFileRoute } from "@tanstack/react-router";

const rotelBase =
  process.env["ROTEL_OTLP_HTTP_ENDPOINT"]?.replace(/\/+$/, "") ??
  "http://localhost:4318";

export const Route = createFileRoute("/v1/logs")({
  server: {
    handlers: {
      POST: async ({ request }) => {
        const body = await request.arrayBuffer();
        try {
          const upstream = await fetch(`${rotelBase}/v1/logs`, {
            method: "POST",
            headers: {
              "content-type":
                request.headers.get("content-type") ?? "application/x-protobuf",
            },
            body,
          });
          return new Response(await upstream.arrayBuffer(), {
            status: upstream.status,
            headers: {
              "content-type":
                upstream.headers.get("content-type") ?? "application/x-protobuf",
            },
          });
        } catch (err) {
          console.error("[/v1/logs] forward to Rotel failed:", err);
          return new Response(null, { status: 202 });
        }
      },
    },
  },
});
