import {
  Link,
  Outlet,
  createFileRoute,
  useLocation,
  useRouter,
} from "@tanstack/react-router";
import { useEffect, useState } from "react";
import {
  DEMO_CUSTOMER_ID,
  DEMO_TENANT_ID,
  errorMessageForUser,
  fetchOrders,
  type OrderSummary,
} from "../commerce";
import { Notice, OrderMeta, PageFrame } from "../components";
import { runTracedStep } from "../rum";
import { APP_SCREEN_NAME, APP_WIDGET_NAME, UI_CLICK } from "../semconv";

export const Route = createFileRoute("/orders")({
  loader: async () => {
    try {
      return {
        orders: await fetchOrders({
          tenantId: DEMO_TENANT_ID,
          customerId: DEMO_CUSTOMER_ID,
        }),
        error: null,
      } as const;
    } catch (error: unknown) {
      return { orders: null, error: errorMessageForUser(error) } as const;
    }
  },
  component: OrdersPage,
});

function OrdersPage() {
  const { pathname } = useLocation();
  const router = useRouter();
  const showingDetail = pathname.startsWith("/orders/");
  const loaderData = Route.useLoaderData();
  const [fallbackOrders, setFallbackOrders] = useState<
    readonly OrderSummary[] | null
  >(loaderData.orders);
  useEffect(() => {
    if (loaderData.orders !== null) {
      setFallbackOrders(loaderData.orders);
      return;
    }
    let active = true;
    void fetchOrders({
      tenantId: DEMO_TENANT_ID,
      customerId: DEMO_CUSTOMER_ID,
    }).then(
      (orders) => {
        if (active) setFallbackOrders(orders);
      },
      () => undefined,
    );
    return () => {
      active = false;
    };
  }, [loaderData.orders]);
  const state =
    fallbackOrders !== null
      ? ({ kind: "ready", orders: fallbackOrders } as const)
      : loaderData.error !== null
      ? ({ kind: "error", message: loaderData.error } as const)
        : ({ kind: "loading" } as const);

  if (showingDetail) return <Outlet />;

  async function refresh() {
    await runTracedStep(
      UI_CLICK,
      { [APP_SCREEN_NAME]: "orders", [APP_WIDGET_NAME]: "orders-refresh" },
      async () => {
        await router.invalidate();
      },
    );
  }

  return (
    <PageFrame
      eyebrow="orders / durable status"
      title="Follow the order after checkout."
      description="Orders are read from the checkout service's Postgres-backed REST projection. Fulfillment updates status asynchronously through RabbitMQ."
      actions={
        <button
          className="button button-secondary"
          type="button"
          onClick={() => void refresh()}
        >
          Refresh status
        </button>
      }
    >
      <div className="notice notice-info">
        <div>
          <strong>Viewing Ava Chen’s tenant-acme orders</strong>
          <p>Customer filter: {DEMO_CUSTOMER_ID} · list path: /api/orders</p>
        </div>
        <Link className="button button-small button-secondary" to="/catalog" search={{ search: undefined, category: undefined, sort: undefined, page: undefined }}>
          Shop again
        </Link>
      </div>
      {state.kind === "error" ? (
        <Notice
          tone="error"
          title="Order status unavailable"
          action={
            <button
              className="button button-small button-secondary"
              type="button"
              onClick={() => void router.invalidate()}
            >
              Retry
            </button>
          }
        >
          {state.message}
        </Notice>
      ) : null}
      {state.kind === "ready" && state.orders.length === 0 ? (
        <div className="empty-state">
          <h2>No orders yet</h2>
          <p>
            Complete a checkout with a seeded catalog variant, then return here
            to watch its durable status.
          </p>
          <Link className="button" to="/catalog" search={{ search: undefined, category: undefined, sort: undefined, page: undefined }}>
            Browse catalog
          </Link>
        </div>
      ) : null}
      {state.kind === "ready" && state.orders.length > 0 ? (
        <section className="order-list" aria-label="Orders">
          {state.orders.map((order) => (
            <article className="order-card" key={order.id}>
              <Link
                className="order-card-link"
                to="/orders/$orderId"
                params={{ orderId: order.id }}
              >
                <OrderMeta {...order} />
                <p className="checkout-note">
                  Open order detail for line items, payment totals, and
                  fulfillment timeline.
                </p>
              </Link>
            </article>
          ))}
        </section>
      ) : null}
      <p className="checkout-note">
        Every status refresh creates a browser interaction span and propagates
        W3C context to the storefront REST boundary.
      </p>
    </PageFrame>
  );
}
