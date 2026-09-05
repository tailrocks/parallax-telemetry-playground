import { Link, createFileRoute } from "@tanstack/react-router";
import { useEffect, useRef, useState, type FormEvent } from "react";
import { useCart } from "../cart";
import {
  DEMO_CUSTOMER_ID,
  DEMO_CURRENCY,
  DEMO_PAYMENT_TOKEN,
  DEMO_TENANT_ID,
  errorMessageForUser,
  fetchQuote,
  formatMoney,
  submitCheckout,
  type Quote,
} from "../commerce";
import { LoadingBlock, Notice, PageFrame, StatusPill } from "../components";
import { emitTypedEvent, runTracedStep, trackStep } from "../rum";
import {
  APP_SCREEN_NAME,
  APP_WIDGET_NAME,
  UI_SUBMIT,
  WEB_CHECKOUT_SUBMITTED,
} from "../semconv";

export const Route = createFileRoute("/checkout")({
  validateSearch: (search: Record<string, unknown>) => ({
    promo: typeof search["promo"] === "string" ? search["promo"] : "",
  }),
  component: CheckoutPage,
});

type QuoteState =
  | Readonly<{ kind: "empty" }>
  | Readonly<{ kind: "loading" }>
  | Readonly<{ kind: "ready"; quote: Quote }>
  | Readonly<{ kind: "error"; message: string }>;

type SubmitState =
  | Readonly<{ kind: "idle" }>
  | Readonly<{ kind: "submitting" }>
  | Readonly<{ kind: "success"; orderId: string; orderNumber: string | null }>
  | Readonly<{
      kind: "pending";
      orderId: string;
      orderNumber: string | null;
      message: string;
    }>
  | Readonly<{ kind: "error"; message: string }>;

function CheckoutPage() {
  const { items, cartId, itemCount, clear } = useCart();
  const { promo } = Route.useSearch();
  const [promotionCode, setPromotionCode] = useState(promo);
  const [paymentToken, setPaymentToken] = useState<string>(
    DEMO_PAYMENT_TOKEN,
  );
  const [paymentMethodType, setPaymentMethodType] = useState("card");
  const [quoteReload, setQuoteReload] = useState(0);
  const [quoteState, setQuoteState] = useState<QuoteState>(
    items.length === 0 ? { kind: "empty" } : { kind: "loading" },
  );
  const [submitState, setSubmitState] = useState<SubmitState>({ kind: "idle" });
  const requestId = useRef<string | null>(null);
  const requestFingerprint = useRef<string | null>(null);

  useEffect(() => {
    let active = true;
    if (items.length === 0) {
      setQuoteState({ kind: "empty" });
      return () => {
        active = false;
      };
    }
    setQuoteState({ kind: "loading" });
    void fetchQuote({
      items,
      promotionCode,
      customerId: DEMO_CUSTOMER_ID,
      currencyCode: DEMO_CURRENCY,
      paymentMethodType,
    }).then(
      (quote) => {
        if (active) setQuoteState({ kind: "ready", quote });
      },
      (error: unknown) => {
        if (active)
          setQuoteState({ kind: "error", message: errorMessageForUser(error) });
      },
    );
    return () => {
      active = false;
    };
  }, [items, paymentMethodType, promotionCode, quoteReload]);

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (items.length === 0 || quoteState.kind !== "ready") return;
    if (paymentToken.trim().length === 0) {
      setSubmitState({ kind: "error", message: "Payment token is required." });
      return;
    }
    const fingerprint = JSON.stringify({
      items,
      cartId,
      promotionCode,
      paymentMethodType,
      paymentToken: paymentToken.trim(),
    });
    if (requestFingerprint.current !== fingerprint) {
      requestFingerprint.current = fingerprint;
      requestId.current = `web-checkout-${crypto.randomUUID()}`;
    }
    const activeRequestId = requestId.current;
    if (activeRequestId === null) {
      setSubmitState({ kind: "error", message: "Could not create checkout identity." });
      return;
    }
    setSubmitState({ kind: "submitting" });
    try {
      await runTracedStep(
        UI_SUBMIT,
        {
          [APP_SCREEN_NAME]: "checkout",
          [APP_WIDGET_NAME]: "checkout-form",
          item_count: itemCount,
          quote_id: quoteState.quote.quoteId,
        },
        async () => {
          await emitTypedEvent(WEB_CHECKOUT_SUBMITTED, {
            item_count: itemCount,
            quote_id: quoteState.quote.quoteId,
          });
          const receipt = await submitCheckout({
            items,
            cartId,
            tenantId: DEMO_TENANT_ID,
            customerId: DEMO_CUSTOMER_ID,
            currencyCode: DEMO_CURRENCY,
            promotionCode,
            paymentMethodToken: paymentToken.trim(),
            paymentMethodType,
            requestId: activeRequestId,
          });
          if (receipt.orderId === null)
            throw new Error("Checkout completed without an order id.");
          if (receipt.status !== "paid") {
            setSubmitState({
              kind: "pending",
              orderId: receipt.orderId,
              orderNumber: receipt.orderNumber,
              message:
                receipt.status === "payment_pending"
                  ? "Payment is still being confirmed. No capture is reported yet."
                  : "Checkout was accepted for follow-up, but payment is not confirmed.",
            });
            return;
          }
          setSubmitState({
            kind: "success",
            orderId: receipt.orderId,
            orderNumber: receipt.orderNumber,
          });
          requestId.current = null;
          requestFingerprint.current = null;
          clear();
        },
      );
    } catch (error: unknown) {
      const message = errorMessageForUser(error);
      setSubmitState({ kind: "error", message });
      void trackStep("web.checkout.failed", {
        [APP_SCREEN_NAME]: "checkout",
        [APP_WIDGET_NAME]: "checkout-form",
        error: error instanceof Error ? error.name : "unknown",
      });
    }
  }

  return (
    <PageFrame
      eyebrow="checkout / durable orchestration"
      title="Review before the boundary."
      description="A fresh itemized quote is required before the Rust checkout service validates Catalog, reserves Inventory, authorizes and captures Payment, writes Postgres, and publishes the outbox event."
      actions={
        <Link className="button button-secondary" to="/cart">
          Back to cart
        </Link>
      }
    >
      {submitState.kind === "success" ? (
        <Notice
          tone="success"
          title="Order accepted and payment captured"
          action={
            <Link
              className="button button-small"
              to="/orders/$orderId"
              params={{ orderId: submitState.orderId }}
            >
              Track order
            </Link>
          }
        >
          {submitState.orderNumber
            ? `Order ${submitState.orderNumber} is moving through fulfillment.`
            : "Your order is moving through fulfillment."}
        </Notice>
      ) : null}
      {submitState.kind === "pending" ? (
        <Notice
          tone="info"
          title="Order is pending payment confirmation"
          action={
            <Link
              className="button button-small"
              to="/orders/$orderId"
              params={{ orderId: submitState.orderId }}
            >
              Track order
            </Link>
          }
        >
          {submitState.orderNumber
            ? `Order ${submitState.orderNumber}: ${submitState.message}`
            : submitState.message}
        </Notice>
      ) : null}
      {submitState.kind === "error" ? (
        <Notice tone="error" title="Checkout did not complete">
          {submitState.message}
        </Notice>
      ) : null}
      {quoteState.kind === "empty" ? (
        <div className="empty-state">
          <h2>Your cart is empty</h2>
          <p>
            Checkout needs at least one catalog variant. Return to the live
            catalog to begin.
          </p>
          <Link className="button" to="/catalog" search={{ search: undefined, category: undefined, sort: undefined, page: undefined }}>
            Browse catalog
          </Link>
        </div>
      ) : null}
      {quoteState.kind !== "empty" ? (
        <div className="checkout-layout">
          <form
            className="surface-card form-grid"
            onSubmit={(event) => void submit(event)}
          >
            <div>
              <p className="eyebrow">payment boundary</p>
              <h2>Payment details</h2>
              <p className="section-kicker">
                This playground uses provider test tokens. The token is sent to
                the real Payment gRPC lifecycle.
              </p>
            </div>
            <label htmlFor="payment-token">
              Payment token
              <input
                className="field"
                id="payment-token"
                value={paymentToken}
                onChange={(event) => setPaymentToken(event.target.value)}
                autoComplete="off"
              />
            </label>
            <label htmlFor="payment-method">
              Payment method
              <select
                className="field"
                id="payment-method"
                value={paymentMethodType}
                onChange={(event) => setPaymentMethodType(event.target.value)}
              >
                <option value="card">Card</option>
                <option value="bank_account">Bank account</option>
                <option value="wallet">Wallet</option>
              </select>
            </label>
            <label htmlFor="promotion-code">
              Promotion code
              <input
                className="field"
                id="promotion-code"
                value={promotionCode}
                onChange={(event) => setPromotionCode(event.target.value)}
                placeholder="Optional"
              />
            </label>
            <button
              className="button"
              type="submit"
              disabled={
                quoteState.kind !== "ready" ||
                (submitState.kind !== "idle" && submitState.kind !== "error")
              }
            >
              {submitState.kind === "submitting"
                ? "Running checkout…"
                : "Place order"}
            </button>
            <p className="checkout-note">
              Default success token: <code>tok_visa</code>. Payment failure
              tokens are intentionally provider-owned; no SKU is treated as a
              failure switch.
            </p>
          </form>

          <aside className="surface-card summary-card" aria-live="polite">
            <p className="eyebrow">pricing gRPC → GraphQL</p>
            <h2>Authoritative quote</h2>
            {quoteState.kind === "loading" ? (
              <LoadingBlock label="Refreshing quote" />
            ) : null}
            {quoteState.kind === "error" ? (
              <Notice
                tone="error"
                title="Quote unavailable"
                action={
                  <button
                    className="button button-small button-secondary"
                    type="button"
                    onClick={() => setQuoteReload((value) => value + 1)}
                  >
                    Retry
                  </button>
                }
              >
                {quoteState.message}
              </Notice>
            ) : null}
            {quoteState.kind === "ready" ? (
              <QuoteSummary quote={quoteState.quote} />
            ) : null}
          </aside>
        </div>
      ) : null}
    </PageFrame>
  );
}

function QuoteSummary({ quote }: Readonly<{ quote: Quote }>) {
  return (
    <>
      <div className="split-row">
        <span className="muted">Status</span>
        <StatusPill status={quote.status} />
      </div>
      <div className="detail-rows" style={{ marginTop: "1rem" }}>
        {quote.lines.map((line) => (
          <div className="detail-row" key={line.sku}>
            <span className="muted">
              {line.sku} × {line.quantity}
            </span>
            <strong>
              {formatMoney(
                line.lineTotal?.amountMinor,
                line.lineTotal?.currencyCode,
              )}
            </strong>
          </div>
        ))}
        <div className="detail-row">
          <span className="muted">Subtotal</span>
          <strong>
            {formatMoney(
              quote.subtotal?.amountMinor,
              quote.subtotal?.currencyCode,
            )}
          </strong>
        </div>
        <div className="detail-row">
          <span className="muted">Discount</span>
          <strong>
            {formatMoney(
              quote.discountTotal?.amountMinor,
              quote.discountTotal?.currencyCode,
            )}
          </strong>
        </div>
        <div className="detail-row">
          <span className="muted">Tax</span>
          <strong>
            {formatMoney(
              quote.taxTotal?.amountMinor,
              quote.taxTotal?.currencyCode,
            )}
          </strong>
        </div>
      </div>
      <div className="split-row summary-total">
        <span>Total</span>
        <strong>
          {formatMoney(
            quote.grandTotal?.amountMinor,
            quote.grandTotal?.currencyCode,
          )}
        </strong>
      </div>
      <p className="checkout-note">
        Quote {quote.quoteId} · valid {quote.validForSeconds}s · pricing{" "}
        {quote.pricingVersion}
      </p>
    </>
  );
}
