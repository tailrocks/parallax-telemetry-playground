import {
  Link,
  createFileRoute,
  useParams,
  useRouter,
} from "@tanstack/react-router";
import {
  DEMO_CUSTOMER_ID,
  DEMO_TENANT_ID,
  displayDate,
  errorMessageForUser,
  fetchOrder,
  formatMoney,
  type OrderDetail,
} from "../../commerce";
import {
  LoadingBlock,
  Notice,
  OrderMeta,
  PageFrame,
  StatusPill,
} from "../../components";
import { runTracedStep } from "../../rum";
import { APP_SCREEN_NAME, APP_WIDGET_NAME, UI_CLICK } from "../../semconv";

export const Route = createFileRoute("/orders/$orderId")({
  loader: async ({ params }) => {
    try {
      return {
        order: await fetchOrder(
          params.orderId,
          DEMO_TENANT_ID,
          DEMO_CUSTOMER_ID,
        ),
        error: null,
      } as const;
    } catch (error: unknown) {
      return { order: null, error: errorMessageForUser(error) } as const;
    }
  },
  pendingComponent: () => (
    <PageFrame
      eyebrow="orders / status detail"
      title="Order status"
      description="Reading the durable order projection."
    >
      <LoadingBlock label="Reading order status" />
    </PageFrame>
  ),
  component: OrderDetailPage,
});

type DetailState =
  | Readonly<{ kind: "loading" }>
  | Readonly<{ kind: "ready"; order: OrderDetail }>
  | Readonly<{ kind: "error"; message: string }>;

function OrderDetailPage() {
  const { orderId } = useParams({ from: "/orders/$orderId" });
  const router = useRouter();
  const loaderData = Route.useLoaderData();
  const state: DetailState =
    loaderData.order !== null
      ? { kind: "ready", order: loaderData.order }
      : loaderData.error !== null
        ? { kind: "error", message: loaderData.error }
        : { kind: "loading" };

  async function refresh() {
    await runTracedStep(
      UI_CLICK,
      {
        [APP_SCREEN_NAME]: "order-detail",
        [APP_WIDGET_NAME]: "order-refresh",
        order_id: orderId,
      },
      async () => {
        await router.invalidate();
      },
    );
  }

  return (
    <PageFrame
      eyebrow="orders / status detail"
      title="Order status"
      description={`Durable order ${orderId}. Refresh to observe the asynchronous fulfillment projection.`}
      actions={
        <>
          <Link className="button button-secondary" to="/orders">
            All orders
          </Link>
          <button
            className="button"
            type="button"
            onClick={() => void refresh()}
          >
            Refresh
          </button>
        </>
      }
    >
      {state.kind === "loading" ? (
        <LoadingBlock label="Reading order status" />
      ) : null}
      {state.kind === "error" ? (
        <Notice
          tone="error"
          title="Order detail unavailable"
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
      {state.kind === "ready" ? <OrderDetail order={state.order} /> : null}
    </PageFrame>
  );
}

function OrderDetail({ order }: Readonly<{ order: OrderDetail }>) {
  const status = order.status.toLowerCase();
  const paymentStatus = order.paymentStatus?.toLowerCase() ?? "unknown";
  const cancelled = status.includes("cancel");
  const stage = cancelled
    ? -1
    : status.includes("ship") || status.includes("complete")
      ? 3
      : status.includes("fulfill") ||
          status.includes("process") ||
          status.includes("paid")
        ? 2
      : 1;
  const paymentCaptured = ["captured", "partially_refunded", "refunded"].includes(
    paymentStatus,
  );
  const timeline = [
    {
      label: "Order recorded",
      detail: "Postgres order row committed",
      done: stage >= 1,
    },
    {
      label: "Payment captured",
      detail: `Payment status: ${paymentStatus}`,
      done: paymentCaptured && !cancelled,
    },
    {
      label: "Fulfillment queued",
      detail: "RabbitMQ order event accepted",
      done: stage >= 2 && !cancelled,
    },
    {
      label: "Shipment projection",
      detail: "Fulfillment status becomes visible",
      done: stage >= 3 && !cancelled,
    },
  ];
  return (
    <>
      <section className="surface-card">
        <OrderMeta
          orderNumber={order.orderNumber}
          status={order.status}
          createdAt={order.createdAt}
          totalMinor={order.totalMinor}
          currency={order.currency}
        />
        <div className="detail-rows" style={{ marginTop: "1.25rem" }}>
          <div className="detail-row">
            <span className="muted">Order id</span>
            <span className="trace-chip">{order.id}</span>
          </div>
          <div className="detail-row">
            <span className="muted">Customer</span>
            <strong>{order.customerId ?? "Not returned"}</strong>
          </div>
          <div className="detail-row">
            <span className="muted">Payment status</span>
            <strong>{order.paymentStatus ?? "Not returned"}</strong>
          </div>
          <div className="detail-row">
            <span className="muted">Last created</span>
            <strong>{displayDate(order.createdAt)}</strong>
          </div>
        </div>
      </section>

      <div className="detail-layout" style={{ marginTop: "1.25rem" }}>
        <section className="surface-card" aria-labelledby="timeline-title">
          <div className="section-heading">
            <div>
              <p className="eyebrow">async fulfillment</p>
              <h2 id="timeline-title">Journey timeline</h2>
            </div>
            <StatusPill status={order.status} />
          </div>
          <div className="timeline">
            {timeline.map((item) => (
              <div
                className={`timeline-step${item.done ? " timeline-step-complete" : ""}`}
                key={item.label}
              >
                <span className="timeline-dot" aria-hidden="true">
                  {item.done ? "✓" : "·"}
                </span>
                <div className="timeline-copy">
                  <strong>{item.label}</strong>
                  <span>{item.detail}</span>
                </div>
              </div>
            ))}
          </div>
          {cancelled ? (
            <Notice tone="error" title="Order cancelled">
              The durable order status reports cancellation; no shipment
              completion is shown.
            </Notice>
          ) : null}
        </section>

        <aside className="surface-card summary-card">
          <p className="eyebrow">order totals</p>
          <h2>Settlement</h2>
          <div className="detail-rows">
            <div className="detail-row">
              <span className="muted">Subtotal</span>
              <strong>
                {formatMoney(order.subtotalMinor, order.currency)}
              </strong>
            </div>
            <div className="detail-row">
              <span className="muted">Discount</span>
              <strong>
                {formatMoney(order.discountMinor, order.currency)}
              </strong>
            </div>
            <div className="detail-row">
              <span className="muted">Tax</span>
              <strong>{formatMoney(order.taxMinor, order.currency)}</strong>
            </div>
            <div className="detail-row">
              <span className="muted">Shipping</span>
              <strong>
                {formatMoney(order.shippingMinor, order.currency)}
              </strong>
            </div>
          </div>
          <div className="split-row summary-total">
            <span>Total</span>
            <strong>{formatMoney(order.totalMinor, order.currency)}</strong>
          </div>
        </aside>
      </div>

      <section
        className="surface-card"
        style={{ marginTop: "1.25rem" }}
        aria-labelledby="order-items-title"
      >
        <div className="section-heading">
          <div>
            <p className="eyebrow">server response</p>
            <h2 id="order-items-title">Line items</h2>
          </div>
          <span className="section-kicker">{order.items.length} lines</span>
        </div>
        <div className="cart-lines">
          {order.items.map((item) => (
            <div className="cart-line" key={item.sku}>
              <div className="cart-line-details">
                <strong>{item.productName}</strong>
                <span>
                  {item.sku} · quantity {item.quantity}
                </span>
              </div>
              <span className="muted">
                {formatMoney(item.unitPriceMinor, order.currency)} each
              </span>
              <strong className="price">
                {formatMoney(item.lineTotalMinor, order.currency)}
              </strong>
            </div>
          ))}
        </div>
      </section>
    </>
  );
}
