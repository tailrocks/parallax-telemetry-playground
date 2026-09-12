import handler, { createServerEntry } from "@tanstack/react-start/server-entry";
import {
  propagationHeaders,
  shutdownServerTelemetry,
  traceServerRequest,
} from "./server-telemetry";
import { injectPropagationMeta } from "./traceparent";

export default createServerEntry({
  async fetch(request) {
    const traced = await traceServerRequest(request, async () => handler.fetch(request));
    const { response } = traced;
    const contentType = response.headers.get("content-type") ?? "";
    if (!contentType.includes("text/html") || response.body === null) {
      return response;
    }

    const html = await response.text();
    const headers = new Headers(response.headers);
    headers.delete("content-length");
    return new Response(injectPropagationMeta(html, propagationHeaders(traced.context)), {
      status: response.status,
      statusText: response.statusText,
      headers,
    });
  },
});

// The outer Bun adapter calls this after its HTTP server drains. Keeping the
// export on the generated server entry shuts down the provider instance that
// actually owns SSR spans.
export { shutdownServerTelemetry };
