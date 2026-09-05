# Parallax Commerce Lab web

TanStack Start + React + strict TypeScript frontend for the commerce telemetry
playground. Bun owns the web toolchain.

The UI uses only server-owned catalog and order data:

- `/` — commerce/telemetry overview with live featured Catalog products.
- `/catalog` — server-owned Catalog GraphQL search, category filters, typed
  sorting, and page-based pagination across the full assortment.
- `/products/:sku` — Catalog product, variants, reviews, and current price.
- `/cart` — browser-session SKU/quantity working set; no product or price fixtures.
- `/checkout` — fresh Pricing quote, then the Storefront GraphQL `checkout` mutation.
- `/orders` — customer-filtered `GET /api/orders` projection.
- `/orders/:orderId` — `GET /api/orders/:order_id` detail and status timeline.
- `/analytics` — ClickHouse-backed `analyticsEvents` and `analyticsSummary` GraphQL reads.

The browser keeps OpenTelemetry and Sentry RUM initialized before hydration.
Commerce requests use `tracedFetch`, carrying W3C trace context and session
baggage to the storefront boundary. UI actions emit typed RUM spans/events;
application analytics writes are best-effort through `recordAnalytics` and never
invent a successful commerce result. SSR creates a real request span and emits
bounded `traceparent`, `tracestate`, and `baggage` metadata for browser
bootstrap; an absent inbound parent produces an honest new trace root.

## Run

```bash
bun install
bun run build
bun run dev
bun run test
bun run e2e:mock
bun run e2e
```

`bun run e2e:mock` runs the mocked UI contract suite. It is not an
end-to-end acceptance test. `bun run e2e` is the required Compose-backed gate;
start the disposable stack first or set `PLAYGROUND_COMPOSE_BASE_URL` to its
web origin.

## Runtime configuration

| Variable                   | Default                         | Purpose                                 |
| -------------------------- | ------------------------------- | --------------------------------------- |
| `VITE_STOREFRONT_URL`      | `/__storefront/graphql`         | Browser GraphQL/REST URL; Compose uses the same-origin proxy |
| `STOREFRONT_URL`            | `http://localhost:8094/graphql` | SSR/server-proxy URL; in Compose this is the internal `http://storefront:8094/graphql` address |
| `VITE_SENTRY_DSN`          | —                               | Sentry browser DSN                      |
| `VITE_RELEASE`             | `dev`                           | Browser `service.version` / release id  |
| `VITE_PARALLAX_ENV`        | `playground`                    | Browser deployment environment          |
| `ROTEL_OTLP_HTTP_ENDPOINT` | `http://localhost:4318`         | Server-side OTLP proxy target           |

The browser demo explicitly selects the seeded `tenant-acme` and
`customer-acme-ava` fixtures. Prices, names, variants, reviews, order totals,
and analytics are parsed from the running services. Its visible success token
`tok_visa` is a provider test token; payment behavior remains owned by Payment.
