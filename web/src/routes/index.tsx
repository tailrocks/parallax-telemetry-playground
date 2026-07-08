import { Link, createFileRoute } from "@tanstack/react-router";
import * as Sentry from "@sentry/tanstackstart-react";
import { useState } from "react";
import { runTracedStep, tracedFetch, trackStep } from "../rum";

export const Route = createFileRoute("/")({
  component: Home,
});

function Home() {
  const [out, setOut] = useState("start a browser journey");

  async function breakRum() {
    const base = import.meta.env.VITE_CHECKOUT_URL ?? "http://localhost:8088";
    try {
      await runTracedStep(
        "ui.click",
        {
          "app.screen.name": "home",
          "app.widget.name": "rum-error-button",
        },
        async () => {
          const res = await tracedFetch(`${base}/checkout?sku=RUM-ERROR&quantity=1&fail=1`);
          const body = await res.text();
          if (!res.ok) {
            throw new Error(`intentional RUM error after backend ${res.status}: ${body}`);
          }
          throw new Error("intentional RUM error without backend failure");
        },
      );
    } catch (err) {
      Sentry.captureException(err);
      setOut(`error: ${String(err)}`);
    }
  }

  return (
    <main style={{ fontFamily: "system-ui, sans-serif", padding: 24, maxWidth: 880 }}>
      <h1>Telemetry Playground</h1>
      <p>OTel Web SDK to `/v1/traces`, Sentry RUM, browser routes, and backend propagation.</p>
      <nav style={{ display: "flex", gap: 12, marginBottom: 20 }}>
        <Link
          to="/checkout"
          onClick={() =>
            void trackStep("ui.click", {
              "app.screen.name": "home",
              "app.widget.name": "start-checkout-link",
            })
          }
        >
          checkout journey
        </Link>
        <Link
          to="/orders"
          onClick={() =>
            void trackStep("ui.click", {
              "app.screen.name": "home",
              "app.widget.name": "orders-link",
            })
          }
        >
          orders journey
        </Link>
      </nav>
      {/* B15: an unresponsive control; manual ui.click spans make the signal
          visible in OTLP even without session replay. */}
      <button
        onClick={() => {
          void trackStep("ui.click", {
            "app.screen.name": "home",
            "app.widget.name": "promo-button",
          });
        }}
      >
        apply promo (unresponsive)
      </button>{" "}
      <button onClick={() => void breakRum()}>
        break (RUM error)
      </button>
      <pre>{out}</pre>
    </main>
  );
}
