import { Link, createFileRoute } from "@tanstack/react-router";
import { useEffect, useMemo, useState } from "react";
import { useCart } from "../cart";
import {
  DEMO_TENANT_ID,
  errorMessageForUser,
  fetchProduct,
  formatMoney,
  type Product,
} from "../commerce";
import {
  LoadingBlock,
  Notice,
  PageFrame,
  QuantityStepper,
} from "../components";
import { runTracedStep, trackStep } from "../rum";
import {
  APP_SCREEN_NAME,
  APP_WIDGET_NAME,
  UI_ACTION_CART_ADD,
  UI_CLICK,
} from "../semconv";

export const Route = createFileRoute("/cart")({
  component: CartPage,
});

type CartLoadState =
  | Readonly<{ kind: "loading" }>
  | Readonly<{ kind: "ready"; products: ReadonlyMap<string, Product | null> }>
  | Readonly<{ kind: "error"; message: string }>;

function CartPage() {
  const { items, itemCount, setQuantity, removeItem, clear } = useCart();
  const [state, setState] = useState<CartLoadState>({ kind: "loading" });
  const [promoCode, setPromoCode] = useState("");
  const itemKey = items.map((item) => `${item.sku}:${item.quantity}`).join("|");

  useEffect(() => {
    let active = true;
    if (items.length === 0) {
      setState({ kind: "ready", products: new Map() });
      return () => {
        active = false;
      };
    }
    setState({ kind: "loading" });
    void Promise.all(
      items.map(
        async (item) =>
          [item.sku, await fetchProduct(item.sku, DEMO_TENANT_ID)] as const,
      ),
    ).then(
      (entries) => {
        if (active) setState({ kind: "ready", products: new Map(entries) });
      },
      (error: unknown) => {
        if (active)
          setState({
            kind: "error",
            message: errorMessageForUser(error),
          });
      },
    );
    return () => {
      active = false;
    };
  }, [itemKey, items]);

  const lineTotal = useMemo(() => {
    if (state.kind !== "ready") return null;
    return items.reduce((sum, item) => {
      const product = state.products.get(item.sku);
      const amount = product?.price?.amountMinor ?? product?.priceMinor;
      return amount === null || amount === undefined
        ? sum
        : sum + amount * item.quantity;
    }, 0);
  }, [items, state]);
  const catalogComplete =
    state.kind === "ready" &&
    items.every((item) => {
      const product = state.products.get(item.sku);
      return product !== undefined && product !== null;
    });

  async function changeQuantity(sku: string, quantity: number) {
    await runTracedStep(
      UI_CLICK,
      {
        [APP_SCREEN_NAME]: "cart",
        [APP_WIDGET_NAME]: UI_ACTION_CART_ADD,
        sku,
        quantity,
      },
      async () => {
        setQuantity(sku, quantity);
      },
    );
  }

  function removeFromCart(sku: string) {
    removeItem(sku);
    void trackStep(UI_CLICK, {
      [APP_SCREEN_NAME]: "cart",
      [APP_WIDGET_NAME]: "cart-remove",
      sku,
    });
  }

  return (
    <PageFrame
      eyebrow="cart / browser session"
      title="Your working set."
      description="The browser stores only SKU, quantity, and a stable cart identity. Product identity and pricing are re-read from Catalog, and Checkout materializes that cart in Postgres before payment."
      actions={
        <Link className="button button-secondary" to="/catalog" search={{ search: undefined, category: undefined, sort: undefined, page: undefined }}>
          Continue shopping
        </Link>
      }
    >
      {state.kind === "error" ? (
        <Notice tone="error" title="Could not refresh cart products">
          {state.message}
        </Notice>
      ) : null}
      {state.kind === "loading" ? (
        <LoadingBlock label="Refreshing cart products" />
      ) : null}
      {state.kind === "ready" && items.length === 0 ? (
        <div className="empty-state">
          <h2>Cart is clear</h2>
          <p>
            Browse the live Catalog to add a seeded product variant. No
            placeholder products are kept here.
          </p>
          <Link className="button" to="/catalog" search={{ search: undefined, category: undefined, sort: undefined, page: undefined }}>
            Browse catalog
          </Link>
        </div>
      ) : null}
      {state.kind === "ready" && items.length > 0 ? (
        <div className="checkout-layout">
          <section className="surface-card" aria-labelledby="cart-lines-title">
            <div className="section-heading">
              <div>
                <p className="eyebrow">{itemCount} units</p>
                <h2 id="cart-lines-title">Cart lines</h2>
              </div>
              <button
                className="button button-small button-quiet"
                type="button"
                onClick={clear}
              >
                Clear cart
              </button>
            </div>
            <div className="cart-lines">
              {items.map((item) => {
                const product = state.products.get(item.sku);
                const amount =
                  product?.price?.amountMinor ?? product?.priceMinor;
                return (
                  <div className="cart-line" key={item.sku}>
                    <div className="cart-line-details">
                      <strong>
                        {product?.name ?? "Catalog product unavailable"}
                      </strong>
                      <span>
                        {item.sku}
                        {product
                          ? ` · ${product.category.name}`
                          : " · refresh required"}
                      </span>
                    </div>
                    <QuantityStepper
                      label={`${item.sku} quantity`}
                      quantity={item.quantity}
                      onChange={(quantity) =>
                        void changeQuantity(
                          item.sku,
                          Math.max(0, Math.min(99, quantity)),
                        )
                      }
                    />
                    <div>
                      <strong className="price">
                        {formatMoney(amount, product?.price?.currency)}
                      </strong>
                      <button
                        className="button button-small button-quiet"
                        type="button"
                        onClick={() => removeFromCart(item.sku)}
                      >
                        Remove
                      </button>
                    </div>
                  </div>
                );
              })}
            </div>
          </section>

          <aside className="surface-card summary-card">
            <p className="eyebrow">fresh quote at checkout</p>
            <h2>Cart summary</h2>
            <div className="detail-rows">
              <div className="detail-row">
                <span className="muted">Units</span>
                <strong>{itemCount}</strong>
              </div>
              <div className="detail-row">
                <span className="muted">Catalog preview</span>
                <strong>
                  {lineTotal === null ? "Unavailable" : formatMoney(lineTotal)}
                </strong>
              </div>
            </div>
            <label
              className="form-grid"
              htmlFor="promo-code"
              style={{ marginTop: "1.25rem" }}
            >
              <span>Promotion code (optional)</span>
              <input
                className="field"
                id="promo-code"
                value={promoCode}
                onChange={(event) => setPromoCode(event.target.value)}
                placeholder="Try ACME10"
              />
            </label>
            {catalogComplete ? (
              <Link
                className="button"
                to="/checkout"
                search={{ promo: promoCode }}
                style={{ width: "100%", marginTop: "1rem" }}
              >
                Review checkout
              </Link>
            ) : (
              <button
                className="button button-secondary"
                type="button"
                disabled
                style={{ width: "100%", marginTop: "1rem" }}
              >
                Refresh catalog to continue
              </button>
            )}
            <p className="checkout-note">
              Preview only. Checkout owns the authoritative price, inventory,
              payment, and order transition.
            </p>
          </aside>
        </div>
      ) : null}
    </PageFrame>
  );
}
