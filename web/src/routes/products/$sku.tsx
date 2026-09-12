import { Link, createFileRoute, useParams } from "@tanstack/react-router";
import { useEffect, useState } from "react";
import { useCart } from "../../cart";
import {
  DEMO_CUSTOMER_ID,
  DEMO_TENANT_ID,
  errorMessageForUser,
  fetchProduct,
  formatMoney,
  recordAnalytics,
  type Product,
} from "../../commerce";
import {
  Notice,
  PageFrame,
  QuantityStepper,
} from "../../components";
import {
  emitTypedEvent,
  reportHandledError,
  runTracedStep,
  trackStep,
} from "../../rum";
import {
  APP_SCREEN_NAME,
  APP_WIDGET_NAME,
  UI_ACTION_CART_ADD,
  UI_CLICK,
} from "../../semconv";

export const Route = createFileRoute("/products/$sku")({
  loader: async ({ params }) => {
    try {
      return {
        product: await fetchProduct(params.sku, DEMO_TENANT_ID),
        error: null,
      } as const;
    } catch (error: unknown) {
      return { product: null, error: errorMessageForUser(error) } as const;
    }
  },
  component: ProductDetailPage,
});

function ProductDetailPage() {
  const { sku } = useParams({ from: "/products/$sku" });
  const { addItem } = useCart();
  const loaderData = Route.useLoaderData();
  const [quantity, setQuantity] = useState(1);
  const [added, setAdded] = useState(false);
  const state =
    loaderData.error !== null
      ? ({ kind: "error", message: loaderData.error } as const)
      : loaderData.product === null
        ? ({ kind: "not-found" } as const)
        : ({ kind: "ready", product: loaderData.product } as const);

  useEffect(() => {
    if (loaderData.product !== null) void recordProductView(loaderData.product);
  }, [loaderData.product]);

  async function addToCart() {
    if (state.kind !== "ready") return;
    await runTracedStep(
      UI_CLICK,
      {
        [APP_SCREEN_NAME]: "product",
        [APP_WIDGET_NAME]: UI_ACTION_CART_ADD,
        sku,
        quantity,
      },
      async () => {
        if (!addItem(sku, quantity)) return;
        setAdded(true);
        await emitTypedEvent("web.cart.item_added", {
          sku,
          quantity,
          source: "product",
        });
        await recordProductEvent(state.product, quantity);
      },
    );
  }

  return (
    <PageFrame
      eyebrow="catalog / product detail"
      title={state.kind === "ready" ? state.product.name : sku}
      description={
        state.kind === "ready"
          ? state.product.description
          : "A server-owned product record from Catalog."
      }
      actions={
        <Link className="button button-secondary" to="/catalog" search={{ search: undefined, category: undefined, sort: undefined, page: undefined }}>
          Back to catalog
        </Link>
      }
    >
      {state.kind === "error" ? (
        <Notice tone="error" title="Product read failed">
          {state.message}
        </Notice>
      ) : null}
      {state.kind === "not-found" ? (
        <div className="empty-state">
          <h2>Product not found</h2>
          <p>
            Catalog returned no active product for {sku}. No fallback product
            was inserted.
          </p>
          <Link className="button" to="/catalog" search={{ search: undefined, category: undefined, sort: undefined, page: undefined }}>
            Return to catalog
          </Link>
        </div>
      ) : null}
      {state.kind === "ready" ? (
        <ProductDetail
          product={state.product}
          quantity={quantity}
          setQuantity={setQuantity}
          added={added}
          onAdd={() => void addToCart()}
        />
      ) : null}
    </PageFrame>
  );
}

function ProductDetail({
  product,
  quantity,
  setQuantity,
  added,
  onAdd,
}: Readonly<{
  product: Product;
  quantity: number;
  setQuantity: (value: number) => void;
  added: boolean;
  onAdd: () => void;
}>) {
  const price = product.price;
  const amount = price?.amountMinor ?? product.priceMinor;
  const rating = product.reviews.length
    ? product.reviews.reduce((sum, review) => sum + review.stars, 0) /
      product.reviews.length
    : null;
  return (
    <div className="detail-layout">
      <section>
        <div
          className="product-art product-art-violet detail-art"
          aria-label={`${product.category.name} product artwork`}
        >
          <span className="product-art-kicker">{product.category.name}</span>
          <span className="product-art-sku">{product.sku}</span>
        </div>
        <div className="section-heading">
          <div>
            <p className="eyebrow">available variants</p>
            <h2>Choose the catalog variant</h2>
          </div>
          <span className="section-kicker">
            {product.variants.length} variants
          </span>
        </div>
        <div className="surface-card variant-list">
          {product.variants.map((variant) => (
            <div className="variant-row" key={variant.id}>
              <div>
                <strong>{variant.name}</strong>
                <span className="muted">
                  {variant.sku} · {variant.options}
                </span>
              </div>
              <span className="price">
                {formatMoney(
                  variant.price?.amountMinor,
                  variant.price?.currency,
                )}
              </span>
            </div>
          ))}
        </div>
        <div className="section-heading">
          <div>
            <p className="eyebrow">catalog reviews</p>
            <h2>What buyers said</h2>
          </div>
          <span className="section-kicker">
            {rating === null ? "No reviews" : `${rating.toFixed(1)} average`}
          </span>
        </div>
        <div className="review-list">
          {product.reviews.length === 0 ? (
            <div className="empty-state">
              <p>No reviews returned by Catalog.</p>
            </div>
          ) : null}
          {product.reviews.map((review) => (
            <article className="surface-card" key={review.id}>
              <div className="split-row">
                <strong>{review.title}</strong>
                <span className="muted">
                  {"★".repeat(Math.min(5, Math.max(0, review.stars)))}
                </span>
              </div>
              <p>{review.text}</p>
              <span className="muted">
                {review.verifiedPurchase
                  ? "Verified purchase"
                  : "Catalog review"}
              </span>
            </article>
          ))}
        </div>
      </section>

      <aside className="surface-card summary-card">
        <p className="eyebrow">{product.brand ?? "Catalog item"}</p>
        <h2>{product.name}</h2>
        <div className="detail-rows">
          <div className="detail-row">
            <span className="muted">SKU</span>
            <strong>{product.sku}</strong>
          </div>
          <div className="detail-row">
            <span className="muted">Category</span>
            <strong>{product.category.name}</strong>
          </div>
          <div className="detail-row">
            <span className="muted">Current price</span>
            <strong>{formatMoney(amount, price?.currency)}</strong>
          </div>
          <div className="detail-row">
            <span className="muted">Risk signal</span>
            <strong>
              {product.riskScore === null
                ? "Not returned"
                : product.riskScore.toFixed(2)}
            </strong>
          </div>
        </div>
        <div className="form-grid" style={{ marginTop: "1.25rem" }}>
          <span className="label">Quantity</span>
          <QuantityStepper
            label={`${product.name} quantity`}
            quantity={quantity}
            onChange={(value) => setQuantity(Math.max(1, Math.min(99, value)))}
          />
          <button className="button" type="button" onClick={onAdd}>
            {added ? "Added to cart" : "Add to cart"}
          </button>
        </div>
        <p className="checkout-note">
          The cart stores SKU and quantity only. The checkout quote re-reads
          current server pricing before payment.
        </p>
        <Link
          className="button button-secondary"
          to="/cart"
          style={{ width: "100%", marginTop: "0.5rem" }}
        >
          Open cart
        </Link>
      </aside>
    </div>
  );
}

async function recordProductView(product: Product): Promise<void> {
  await emitTypedEvent("web.product.viewed", {
    sku: product.sku,
    source: "product",
  });
  try {
    await recordAnalytics({
      eventKey: `web-view-${product.sku}-${crypto.randomUUID()}`,
      eventName: "web.product.viewed",
      entityType: "product",
      entityId: product.id,
      tenantId: DEMO_TENANT_ID,
      customerId: DEMO_CUSTOMER_ID,
      properties: { sku: product.sku },
    });
  } catch (error: unknown) {
    reportHandledError(error, "RecordProductViewAnalytics", {
      [APP_SCREEN_NAME]: "product",
      [APP_WIDGET_NAME]: "analytics-write",
    });
    void trackStep("web.analytics.record_failed", {
      [APP_SCREEN_NAME]: "product",
      [APP_WIDGET_NAME]: "analytics-write",
      error: error instanceof Error ? error.name : "unknown",
    });
  }
}

async function recordProductEvent(
  product: Product,
  quantity: number,
): Promise<void> {
  try {
    await recordAnalytics({
      eventKey: `web-add-${product.sku}-${crypto.randomUUID()}`,
      eventName: "web.product.added_to_cart",
      entityType: "product_variant",
      entityId: product.sku,
      tenantId: DEMO_TENANT_ID,
      customerId: DEMO_CUSTOMER_ID,
      properties: { quantity, source: "product" },
    });
  } catch (error: unknown) {
    reportHandledError(error, "RecordProductAnalytics", {
      [APP_SCREEN_NAME]: "product",
      [APP_WIDGET_NAME]: "analytics-write",
    });
    void trackStep("web.analytics.record_failed", {
      [APP_SCREEN_NAME]: "product",
      [APP_WIDGET_NAME]: "analytics-write",
      error: error instanceof Error ? error.name : "unknown",
    });
  }
}
