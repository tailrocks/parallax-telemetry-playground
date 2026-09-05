import { Link } from "@tanstack/react-router";
import type { ReactNode } from "react";
import { useCart } from "./cart";
import { displayDate, formatMoney, type Product } from "./commerce";

export function SiteHeader() {
  const { itemCount } = useCart();
  return (
    <header className="site-header">
      <div className="header-inner">
        <Link className="brand" to="/">
          <span className="brand-mark" aria-hidden="true">
            ◒
          </span>
          <span>
            <span className="brand-name">Parallax</span>
            <span className="brand-subtitle">Commerce lab</span>
          </span>
        </Link>
        <nav className="primary-nav" aria-label="Primary navigation">
          <Link
            to="/catalog"
            search={{ search: undefined, category: undefined, sort: undefined, page: undefined }}
            activeProps={{ className: "nav-link nav-link-active" }}
            className="nav-link"
          >
            Catalog
          </Link>
          <Link
            to="/cart"
            activeProps={{ className: "nav-link nav-link-active" }}
            className="nav-link nav-cart-link"
          >
            Cart <span className="cart-count">{itemCount}</span>
          </Link>
          <Link
            to="/orders"
            activeProps={{ className: "nav-link nav-link-active" }}
            className="nav-link"
          >
            Orders
          </Link>
          <Link
            to="/analytics"
            search={{ eventName: undefined }}
            activeProps={{ className: "nav-link nav-link-active" }}
            className="nav-link"
          >
            Analytics
          </Link>
        </nav>
        <div
          className="telemetry-badge"
          title="Browser OpenTelemetry and Sentry RUM are active"
        >
          <span className="pulse-dot" aria-hidden="true" />
          <span>Telemetry live</span>
        </div>
      </div>
    </header>
  );
}

export function CartStorageNotice() {
  const { storageError } = useCart();
  return storageError ? (
    <div className="page-frame" style={{ paddingBottom: 0 }}>
      <Notice tone="error" title="Cart persistence is unavailable">
        {storageError} No checkout action will use an unpersisted cart.
      </Notice>
    </div>
  ) : null;
}

export function PageFrame({
  eyebrow,
  title,
  description,
  actions,
  children,
}: Readonly<{
  eyebrow?: string;
  title: string;
  description?: string;
  actions?: ReactNode;
  children: ReactNode;
}>) {
  return (
    <main className="page-frame">
      <div className="page-heading">
        <div>
          {eyebrow ? <p className="eyebrow">{eyebrow}</p> : null}
          <h1>{title}</h1>
          {description ? (
            <p className="page-description">{description}</p>
          ) : null}
        </div>
        {actions ? <div className="page-actions">{actions}</div> : null}
      </div>
      {children}
    </main>
  );
}

export function Notice({
  tone,
  title,
  children,
  action,
}: Readonly<{
  tone: "info" | "error" | "success";
  title: string;
  children?: ReactNode;
  action?: ReactNode;
}>) {
  return (
    <section
      className={`notice notice-${tone}`}
      role={tone === "error" ? "alert" : "status"}
    >
      <div>
        <strong>{title}</strong>
        {children ? <p>{children}</p> : null}
      </div>
      {action ? <div className="notice-action">{action}</div> : null}
    </section>
  );
}

export function LoadingBlock({
  label = "Loading commerce data",
}: Readonly<{ label?: string }>) {
  return (
    <div className="loading-block" role="status" aria-label={label}>
      <span className="spinner" aria-hidden="true" />
      <span>{label}</span>
    </div>
  );
}

export function ProductCard({
  product,
  onAdd,
  added,
}: Readonly<{
  product: Product;
  onAdd: (sku: string) => void;
  added: boolean;
}>) {
  const price = product.price;
  const amount = price?.amountMinor ?? product.priceMinor;
  const compareAt = price?.compareAtMinor;
  const rating = product.reviews.length
    ? product.reviews.reduce((sum, review) => sum + review.stars, 0) /
      product.reviews.length
    : null;
  return (
    <article className="product-card">
      <Link
        className={`product-art ${artTone(product.category.slug)}`}
        to="/products/$sku"
        params={{ sku: product.sku }}
        aria-label={`View ${product.name}`}
      >
        <span className="product-art-kicker">{product.category.name}</span>
        <span className="product-art-sku">{product.sku}</span>
      </Link>
      <div className="product-card-body">
        <div className="product-card-meta">
          <span>{product.brand ?? "Catalog item"}</span>
          <span>{rating === null ? "New" : `${rating.toFixed(1)} ★`}</span>
        </div>
        <h2>
          <Link
            className="product-title-link"
            to="/products/$sku"
            params={{ sku: product.sku }}
          >
            {product.name}
          </Link>
        </h2>
        <p className="product-description">{product.description}</p>
        <div className="product-card-footer">
          <div>
            <strong className="price">
              {formatMoney(amount, price?.currency)}
            </strong>
            {compareAt !== null && compareAt !== undefined ? (
              <span className="compare-price">
                {formatMoney(compareAt, price?.currency)}
              </span>
            ) : null}
          </div>
          <button
            className="button button-small"
            type="button"
            onClick={() => onAdd(product.sku)}
          >
            {added ? "Added" : "Add"}
          </button>
        </div>
      </div>
    </article>
  );
}

export function QuantityStepper({
  quantity,
  onChange,
  label,
}: Readonly<{
  quantity: number;
  onChange: (quantity: number) => void;
  label: string;
}>) {
  return (
    <div className="quantity-stepper" aria-label={label}>
      <button
        type="button"
        aria-label={`Decrease ${label}`}
        onClick={() => onChange(quantity - 1)}
      >
        −
      </button>
      <span aria-live="polite">{quantity}</span>
      <button
        type="button"
        aria-label={`Increase ${label}`}
        onClick={() => onChange(quantity + 1)}
      >
        +
      </button>
    </div>
  );
}

export function StatusPill({ status }: Readonly<{ status: string }>) {
  const normalized = status.toLowerCase().replaceAll("_", "-");
  const tone =
    normalized.includes("cancel") || normalized.includes("fail")
      ? "bad"
      : normalized.includes("paid") ||
          normalized.includes("ship") ||
          normalized.includes("complete")
        ? "good"
        : "neutral";
  return (
    <span className={`status-pill status-pill-${tone}`}>
      {status.replaceAll("_", " ")}
    </span>
  );
}

export function OrderMeta({
  orderNumber,
  status,
  createdAt,
  totalMinor,
  currency,
}: Readonly<{
  orderNumber: string;
  status: string;
  createdAt: string | null;
  totalMinor: number;
  currency: string;
}>) {
  return (
    <div className="order-meta">
      <div>
        <span className="label">Order</span>
        <strong>{orderNumber}</strong>
      </div>
      <StatusPill status={status} />
      <div className="order-meta-total">
        <span className="label">Total</span>
        <strong>{formatMoney(totalMinor, currency)}</strong>
      </div>
      <span className="muted">{displayDate(createdAt)}</span>
    </div>
  );
}

function artTone(slug: string): string {
  if (slug.includes("electronic")) return "product-art-blue";
  if (slug.includes("kitchen")) return "product-art-coral";
  if (slug.includes("pack")) return "product-art-forest";
  if (slug.includes("light")) return "product-art-amber";
  return "product-art-violet";
}
