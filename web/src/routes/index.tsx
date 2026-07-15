import { Link, createFileRoute } from "@tanstack/react-router";
import * as Sentry from "@sentry/tanstackstart-react";
import { useState } from "react";
import { runTracedStep, tracedFetch, trackStep } from "../rum";
import { APP_SCREEN_NAME, APP_WIDGET_NAME, UI_CLICK } from "../semconv";

export const Route = createFileRoute("/")({
  component: Home,
});

function Home() {
  const [out, setOut] = useState("start a browser journey");

  async function breakRum() {
    const base = import.meta.env["VITE_CHECKOUT_URL"] ?? "http://localhost:8088";
    try {
      await runTracedStep(
        UI_CLICK,
        {
          [APP_SCREEN_NAME]: "home",
          [APP_WIDGET_NAME]: "rum-error-button",
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
            void trackStep(UI_CLICK, {
              [APP_SCREEN_NAME]: "home",
              [APP_WIDGET_NAME]: "start-checkout-link",
            })
          }
        >
          checkout journey
        </Link>
        <Link
          to="/orders"
          onClick={() =>
            void trackStep(UI_CLICK, {
              [APP_SCREEN_NAME]: "home",
              [APP_WIDGET_NAME]: "orders-link",
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
          void trackStep(UI_CLICK, {
            [APP_SCREEN_NAME]: "home",
            [APP_WIDGET_NAME]: "promo-button",
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
