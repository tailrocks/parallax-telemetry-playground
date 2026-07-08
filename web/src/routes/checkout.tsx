import { Link, createFileRoute } from "@tanstack/react-router";
import { useEffect, useState } from "react";
import { emitTypedEvent, runTracedStep, tracedFetch, trackStep } from "../rum";

export const Route = createFileRoute("/checkout")({
  component: CheckoutPage,
});

const SKUS = ["WIDGET-1", "WIDGET-2", "RUM-DEMO"];

function CheckoutPage() {
  const [sku, setSku] = useState("WIDGET-1");
  const [quantity, setQuantity] = useState(2);
  const [status, setStatus] = useState("ready");
  const [nopropagate, setNopropagate] = useState(false);

  useEffect(() => {
    setNopropagate(new URLSearchParams(window.location.search).get("nopropagate") === "1");
  }, []);

  async function submit() {
    const normalBase = import.meta.env.VITE_CHECKOUT_URL ?? "http://localhost:8088";
    const noPropBase =
      import.meta.env.VITE_CHECKOUT_URL_NOPROP ?? "http://127.0.0.1:8088";
    const base = nopropagate ? noPropBase : normalBase;
    const query = new URLSearchParams({ sku, quantity: String(quantity) });

    setStatus("submitting...");
    try {
      await runTracedStep(
        "ui.submit",
        {
          "app.screen.name": "checkout",
          "app.widget.name": nopropagate
            ? "checkout-form-nopropagate"
            : "checkout-form",
          "telemetry.propagation.disabled": nopropagate,
        },
        async () => {
          await emitTypedEvent("web.checkout.submitted", {
            sku,
            quantity,
          });
          const res = nopropagate
            ? await fetch(`${base}/checkout?${query}`)
            : await tracedFetch(`${base}/checkout?${query}`);
          const body = await res.text();
          setStatus(`${res.status}: ${body}`);
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
        <Link to="/orders">orders</Link>
      </nav>
      <h1>Checkout</h1>
      {nopropagate ? (
        <p>
          Propagation break mode: browser spans still emit, but checkout uses a
          backend origin outside the propagation allowlist.
        </p>
      ) : null}
      <form
        onSubmit={(event) => {
          event.preventDefault();
          void submit();
        }}
        style={{ display: "grid", gap: 12, maxWidth: 360 }}
      >
        <label>
          SKU
          <select
            value={sku}
            onChange={(event) => {
              setSku(event.target.value);
              void trackStep("ui.click", {
                "app.screen.name": "checkout",
                "app.widget.name": "sku-picker",
              });
            }}
          >
            {SKUS.map((item) => (
              <option key={item} value={item}>
                {item}
              </option>
            ))}
          </select>
        </label>
        <label>
          Quantity
          <input
            min={1}
            max={9}
            type="number"
            value={quantity}
            onChange={(event) => setQuantity(Number(event.target.value))}
          />
        </label>
        <button type="button" onClick={() => void submit()}>
          submit checkout
        </button>
      </form>
      <pre>{status}</pre>
      <p>
        <Link to="/checkout" search={{ nopropagate: "1" }}>
          open propagation-break variant
        </Link>
      </p>
    </main>
  );
}
