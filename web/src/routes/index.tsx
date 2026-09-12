import { Link, createFileRoute } from "@tanstack/react-router";
import { useEffect, useState } from "react";
import { useCart } from "../cart";
import {
  DEMO_TENANT_ID,
  DEMO_CUSTOMER_ID,
  errorMessageForUser,
  fetchProducts,
  recordAnalytics,
  type ProductPage,
} from "../commerce";
import { LoadingBlock, Notice, PageFrame, ProductCard } from "../components";
import {
  emitTypedEvent,
  reportHandledError,
  runTracedStep,
  trackStep,
} from "../rum";
import {
  APP_SCREEN_NAME,
  APP_WIDGET_NAME,
  UI_CLICK,
  UI_ACTION_CART_ADD,
} from "../semconv";

export const Route = createFileRoute("/")({
  loader: async () => {
    try {
      return {
        products: await fetchProducts({ tenantId: DEMO_TENANT_ID, size: 4 }),
        error: null,
      } as const;
    } catch (error: unknown) {
      return { products: null, error: errorMessageForUser(error) } as const;
    }
  },
  component: Home,
});

function Home() {
  const { itemCount, addItem } = useCart();
  const loaderData = Route.useLoaderData();
  const [fallbackProducts, setFallbackProducts] = useState<ProductPage | null>(
    loaderData.products,
  );
  const [addedSku, setAddedSku] = useState<string | null>(null);

  useEffect(() => {
    if (loaderData.products !== null) {
      setFallbackProducts(loaderData.products);
      return;
    }
    const controller = new AbortController();
    void fetchProducts({
      tenantId: DEMO_TENANT_ID,
      size: 4,
      signal: controller.signal,
    }).then(
      (products) => {
        if (!controller.signal.aborted) setFallbackProducts(products);
      },
      () => undefined,
    );
    return () => controller.abort();
  }, [loaderData.products]);

  const products = fallbackProducts;

  async function addFeaturedItem(sku: string) {
    await runTracedStep(
      UI_CLICK,
      { [APP_SCREEN_NAME]: "home", [APP_WIDGET_NAME]: UI_ACTION_CART_ADD, sku },
      async () => {
        if (!addItem(sku)) return;
        setAddedSku(sku);
        await emitTypedEvent("web.cart.item_added", { sku, source: "home" });
        await recordBrowseEvent(sku);
      },
    );
  }

  return (
    <PageFrame
      title="Commerce, with a trace attached."
      description="A real browser-facing commerce surface. Browse catalog data, build a cart, price it through gRPC, and follow the order and analytics path end to end."
      actions={
        <Link className="button" to="/catalog" search={{ search: undefined, category: undefined, sort: undefined, page: undefined }}>
          Browse catalog <span aria-hidden="true">↗</span>
        </Link>
      }
    >
      <section className="hero-panel" aria-labelledby="home-hero-title">
        <div className="hero-copy">
          <p className="eyebrow">tenant-acme / standard segment</p>
          <h2 id="home-hero-title">One journey. Every boundary visible.</h2>
          <p>
            Catalog reads, itemized quotes, durable checkout, payment,
            inventory, RabbitMQ fulfillment, and ClickHouse analytics all stay
            connected to the browser trace.
          </p>
          <div className="hero-actions">
            <Link className="button" to="/catalog" search={{ search: undefined, category: undefined, sort: undefined, page: undefined }}>
              Start with products
            </Link>
            <Link className="button button-secondary" to="/analytics" search={{ eventName: undefined }}>
              Inspect signals
            </Link>
          </div>
        </div>
        <div
          className="signal-orbit"
          aria-label="Distributed commerce trace diagram"
        >
          <div className="orbit-ring" />
          <div className="signal-core" aria-hidden="true">
            ⌁
          </div>
          <span className="signal-label">W3C context / active</span>
        </div>
      </section>

      <div className="section-heading">
        <div>
          <p className="eyebrow">live from catalog GraphQL</p>
          <h2>Featured inventory</h2>
        </div>
        <span className="section-kicker">
          {itemCount} cart units · prices are server-owned
        </span>
      </div>

      <div className="product-grid">
        {products === null ? (
          loaderData.error !== null ? (
            <Notice tone="error" title="Featured catalog unavailable">
              {loaderData.error}
            </Notice>
          ) : (
            <LoadingBlock label="Loading featured catalog" />
          )
        ) : (
          products.items.map((product) => (
            <ProductCard
              key={product.id}
              product={product}
              onAdd={(sku) => void addFeaturedItem(sku)}
              added={addedSku === product.sku}
            />
          ))
        )}
      </div>

      <div className="section-heading">
        <div>
          <p className="eyebrow">reference topology</p>
          <h2>Where the signal goes</h2>
        </div>
      </div>
      <div className="service-grid">
        <article className="service-card">
          <span className="service-icon" aria-hidden="true">
            ◌
          </span>
          <strong>Catalog + pricing</strong>
          <p>
            GraphQL delegates product reads to Catalog and quotes to the
            itemized Pricing gRPC contract.
          </p>
        </article>
        <article className="service-card">
          <span className="service-icon" aria-hidden="true">
            ↗
          </span>
          <strong>Checkout + fulfillment</strong>
          <p>
            Postgres order state, payment lifecycle, inventory reservations, and
            durable RabbitMQ events.
          </p>
        </article>
        <article className="service-card">
          <span className="service-icon" aria-hidden="true">
            ⌁
          </span>
          <strong>RUM + analytics</strong>
          <p>
            Browser OTel and Sentry stay on; application events are readable
            from ClickHouse via GraphQL.
          </p>
        </article>
      </div>
    </PageFrame>
  );
}

async function recordBrowseEvent(sku: string): Promise<void> {
  try {
    await recordAnalytics({
      eventKey: `web-browse-${sku}-${crypto.randomUUID()}`,
      eventName: "web.product.added_to_cart",
      entityType: "product_variant",
      entityId: sku,
      tenantId: DEMO_TENANT_ID,
      customerId: DEMO_CUSTOMER_ID,
      properties: { source: "home" },
    });
  } catch (error: unknown) {
    reportHandledError(error, "RecordBrowseAnalytics", {
      [APP_SCREEN_NAME]: "home",
      [APP_WIDGET_NAME]: "analytics-write",
    });
    void trackStep("web.analytics.record_failed", {
      [APP_SCREEN_NAME]: "home",
      [APP_WIDGET_NAME]: "analytics-write",
      error: error instanceof Error ? error.name : "unknown",
    });
  }
}
