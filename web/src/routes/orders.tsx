import { Link, createFileRoute } from "@tanstack/react-router";
import { useState } from "react";
import { runTracedStep, tracedFetch, trackStep } from "../rum";
import { APP_SCREEN_NAME, APP_WIDGET_NAME, UI_CLICK, UI_SUBMIT } from "../semconv";

export const Route = createFileRoute("/orders")({
  component: OrdersPage,
});

function OrdersPage() {
  const [lagMs, setLagMs] = useState(250);
  const [batch, setBatch] = useState(false);
  const [status, setStatus] = useState("ready");

  async function submit() {
    const base = import.meta.env.VITE_ORDERS_URL ?? "http://localhost:8092";
    const query = new URLSearchParams({
      lag_ms: String(lagMs),
      batch: batch ? "1" : "0",
    });

    setStatus("submitting...");
    try {
      await runTracedStep(
        UI_SUBMIT,
        {
          [APP_SCREEN_NAME]: "orders",
          [APP_WIDGET_NAME]: "order-form",
        },
        async () => {
          const res = await tracedFetch(`${base}/order?${query}`, { method: "POST" });
          setStatus(`${res.status}: ${await res.text()}`);
        },
      );
    } catch (err) {
      setStatus(`error: ${String(err)}`);
    }
  }

  return (
    <main style={{ fontFamily: "system-ui, sans-serif", padding: 24, maxWidth: 880 }}>
      <nav style={{ display: "flex", gap: 12, marginBottom: 20 }}>
        <Link to="/">home</Link>
        <Link to="/checkout">checkout</Link>
      </nav>
      <h1>Orders</h1>
      <form
        onSubmit={(event) => {
          event.preventDefault();
          void submit();
        }}
        style={{ display: "grid", gap: 12, maxWidth: 360 }}
      >
        <label>
          Consumer lag ms
          <input
            min={0}
            max={5000}
            type="number"
            value={lagMs}
            onChange={(event) => setLagMs(Number(event.target.value))}
          />
        </label>
        <label>
          <input
            checked={batch}
            type="checkbox"
            onChange={(event) => {
              setBatch(event.target.checked);
              void trackStep(UI_CLICK, {
                [APP_SCREEN_NAME]: "orders",
                [APP_WIDGET_NAME]: "batch-toggle",
              });
            }}
          />{" "}
          batch consumer
        </label>
        <button type="submit">submit order</button>
      </form>
      <pre>{status}</pre>
    </main>
  );
}
