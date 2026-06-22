// Frontend entry. Sentry first (RUM: replay, web vitals, source maps), then the
// OTel browser provider (portable OTLP → Rotel). One distributed trace forms
// when fetch() to the checkout API propagates traceparent.
import * as Sentry from "@sentry/tanstackstart-react";
import React from "react";
import { createRoot } from "react-dom/client";
import { initOtel } from "./telemetry";

Sentry.init({
  dsn: import.meta.env.VITE_SENTRY_DSN,
  environment: "playground",
  tracesSampleRate: 1.0,
  replaysSessionSampleRate: 0.1,
  replaysOnErrorSampleRate: 1.0,
  integrations: [Sentry.replayIntegration(), Sentry.browserTracingIntegration()],
});

initOtel([/\/api\//, /localhost:8088/]);

function App() {
  const [out, setOut] = React.useState("click to checkout");
  async function checkout() {
    const base = import.meta.env.VITE_CHECKOUT_URL ?? "http://localhost:8088";
    const res = await fetch(`${base}/checkout?sku=WIDGET-1&quantity=2`);
    setOut(await res.text());
  }
  return (
    <main style={{ fontFamily: "monospace", padding: 24 }}>
      <h1>Telemetry Playground</h1>
      <button onClick={checkout}>checkout</button>
      {/* B15: an unresponsive control — rapid repeated clicks register as a
          "rage click" in Sentry Session Replay. */}
      <button onClick={() => { /* intentionally does nothing (rage-click demo) */ }}>
        apply promo (unresponsive)
      </button>
      {/* A5: a button that throws so Sentry captures the error + replay (RUM). */}
      <button onClick={() => { throw new Error("intentional RUM error (A5)"); }}>
        break (RUM error)
      </button>
      <pre>{out}</pre>
    </main>
  );
}

createRoot(document.getElementById("root")!).render(<App />);
