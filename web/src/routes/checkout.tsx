import { Link, createFileRoute } from "@tanstack/react-router";
import { useEffect, useState } from "react";
import { emitTypedEvent, runTracedStep, tracedFetch, trackStep } from "../rum";
import {
  APP_SCREEN_NAME,
  APP_WIDGET_NAME,
  TELEMETRY_PROPAGATION_DISABLED,
  UI_CLICK,
  UI_SUBMIT,
  WEB_CHECKOUT_SUBMITTED,
} from "../semconv";

export const Route = createFileRoute("/checkout")({
  component: CheckoutPage,
});

const SKUS = ["WIDGET-1", "WIDGET-2", "RUM-DEMO"];

type SubmissionStatus =
  | { readonly kind: "ready"; readonly message: "ready" }
  | { readonly kind: "submitting"; readonly message: "submitting..." }
  | { readonly kind: "success"; readonly message: string }
  | { readonly kind: "http-error"; readonly message: string }
  | { readonly kind: "network-error"; readonly message: string };

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function CheckoutPage() {
  const [sku, setSku] = useState("WIDGET-1");
  const [quantity, setQuantity] = useState(2);
  const [status, setStatus] = useState<SubmissionStatus>({
    kind: "ready",
    message: "ready",
  });
  const [nopropagate, setNopropagate] = useState(false);

  useEffect(() => {
    setNopropagate(new URLSearchParams(window.location.search).get("nopropagate") === "1");
  }, []);

  async function submit() {
    const normalBase = import.meta.env["VITE_CHECKOUT_URL"] ?? "http://localhost:8088";
    const noPropBase =
      import.meta.env["VITE_CHECKOUT_URL_NOPROP"] ?? "http://127.0.0.1:8088";
    const base = nopropagate ? noPropBase : normalBase;
    const query = new URLSearchParams({ sku, quantity: String(quantity) });

    setStatus({ kind: "submitting", message: "submitting..." });
    try {
      await runTracedStep(
        UI_SUBMIT,
        {
          [APP_SCREEN_NAME]: "checkout",
          [APP_WIDGET_NAME]: nopropagate
            ? "checkout-form-nopropagate"
            : "checkout-form",
          [TELEMETRY_PROPAGATION_DISABLED]: nopropagate,
        },
        async () => {
          await emitTypedEvent(WEB_CHECKOUT_SUBMITTED, {
            sku,
            quantity,
          });
          const res = nopropagate
            ? await fetch(`${base}/checkout?${query}`)
            : await tracedFetch(`${base}/checkout?${query}`);
          const body = await res.text();
          setStatus({
            kind: res.ok ? "success" : "http-error",
            message: `${res.ok ? "Success" : "HTTP error"}: ${res.status}: ${body}`,
          });
        },
      );
    } catch (err) {
      setStatus({ kind: "network-error", message: `Network error: ${errorMessage(err)}` });
    }
  }

  return (
    <main style={{ fontFamily: "system-ui, sans-serif", padding: 24, maxWidth: 880 }}>
      <nav style={{ display: "flex", gap: 12, marginBottom: 20 }}>
        <Link to="/">home</Link>
        <Link to="/orders">orders</Link>
      </nav>
      <h1>{nopropagate ? "Checkout — propagation-break test" : "Checkout"}</h1>
      {nopropagate ? (
        <aside aria-label="Intentional propagation-break test" role="note">
          <h2>Intentional propagation-break test</h2>
          <p>
            Propagation break mode: browser spans still emit, but checkout uses a
            backend origin outside the propagation allowlist.
          </p>
        </aside>
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
              void trackStep(UI_CLICK, {
                [APP_SCREEN_NAME]: "checkout",
                [APP_WIDGET_NAME]: "sku-picker",
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
      <div aria-atomic="true" aria-live="polite" role="status">
        {status.message}
      </div>
      <p>
        <Link to="/checkout" search={{ nopropagate: "1" }}>
          open intentional propagation-break test
        </Link>
      </p>
    </main>
  );
}
