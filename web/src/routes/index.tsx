import { createFileRoute } from "@tanstack/react-router";
import * as Sentry from "@sentry/tanstackstart-react";
import { useState } from "react";

export const Route = createFileRoute("/")({
  component: Home,
});

function Home() {
  const [out, setOut] = useState("click to checkout");

  async function checkout() {
    // fetch() is OTel-instrumented: traceparent propagates to the Rust
    // `checkout` backend, stitching browser → backend into one trace.
    const base = import.meta.env.VITE_CHECKOUT_URL ?? "http://localhost:8088";
    try {
      const res = await fetch(`${base}/checkout?sku=WIDGET-1&quantity=2`);
      setOut(await res.text());
    } catch (err) {
      Sentry.captureException(err);
      setOut(`error: ${String(err)}`);
    }
  }

  return (
    <main style={{ fontFamily: "monospace", padding: 24 }}>
      <h1>Telemetry Playground</h1>
      <p>OTel Web SDK (→ /v1/traces proxy → Rotel) + Sentry RUM.</p>
      <button onClick={checkout}>checkout</button>{" "}
      {/* B15: an unresponsive control — rapid repeated clicks register as a
          "rage click" in Sentry Session Replay. */}
      <button
        onClick={() => {
          /* intentionally does nothing (rage-click demo) */
        }}
      >
        apply promo (unresponsive)
      </button>{" "}
      {/* A5: a button that throws so Sentry captures the error + replay (RUM). */}
      <button
        onClick={() => {
          throw new Error("intentional RUM error (A5)");
        }}
      >
        break (RUM error)
      </button>
      <pre>{out}</pre>
    </main>
  );
}
