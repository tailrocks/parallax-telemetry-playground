# Frontend Telemetry Contract

The browser emits OpenTelemetry RUM signals to the same-origin `/v1/traces`
and `/v1/logs` proxy. The web server forwards those signals to Rotel using
`ROTEL_OTLP_HTTP_ENDPOINT`. Browser commerce requests use the same Storefront
gateway for GraphQL and REST, so the browser-to-Storefront boundary remains in
the distributed trace.

## Build-time configuration

`VITE_*` values are compiled into the client bundle by Vite during the web
image build. The build stage in `deploy/Dockerfile.web` declares the supported
build arguments and supplies their defaults; changing `VITE_*` variables only
in the running container cannot change an already-built browser bundle.

| Build argument        | Default                         | Client use                                     |
| --------------------- | ------------------------------- | ---------------------------------------------- |
| `VITE_STOREFRONT_URL` | `/__storefront/graphql` | Same-origin Storefront GraphQL and `/api/*` gateway proxy |
| `VITE_SENTRY_DSN`     | empty                           | Sentry browser DSN                             |
| `VITE_RELEASE`        | `dev`                           | Browser `service.version` and release id       |
| `VITE_PARALLAX_ENV`   | `playground`                    | Browser deployment environment                 |
| `VITE_GIT_SHA`        | `local`                         | Browser build identity                         |

`ROTEL_OTLP_HTTP_ENDPOINT` is server runtime configuration. It is not a
browser `VITE_*` value.

## Resource attributes

Every browser span from `WebTracerProvider` carries:

| Attribute                     | Value                                            |
| ----------------------------- | ------------------------------------------------ |
| `service.name`                | `web`                                            |
| `service.version`             | Compiled `VITE_RELEASE`, or `dev`                |
| `deployment.environment.name` | Compiled `VITE_PARALLAX_ENV`, or `playground`    |
| `session.id`                  | Session-storage UUID, stable for one tab session |
| `vcs.ref.head.revision`       | Compiled `VITE_GIT_SHA` when present             |

`session.id` is propagated as W3C baggage by `tracedFetch`. It is not emitted
as a metric label.

## Propagation and commerce boundaries

All browser commerce calls use `tracedFetch`, which injects W3C
`traceparent`, `tracestate`, and `baggage` when available. The browser calls
the Storefront gateway at the compiled `VITE_STOREFRONT_URL` origin:

| Browser operation          | Storefront path            | Downstream responsibility                                         |
| -------------------------- | -------------------------- | ----------------------------------------------------------------- |
| Browse products/categories | `POST /graphql`            | Catalog GraphQL and Redis-backed reads                            |
| Get checkout quote         | `POST /graphql`            | Pricing gRPC and quote contract                                   |
| Submit checkout            | `POST /graphql` (`checkout` mutation) | Rust Checkout orchestration, Payment, Inventory, Postgres, outbox |
| List orders                | `GET /api/orders`          | Durable customer-filtered order projection                        |
| Read order status          | `GET /api/orders/:orderId` | Durable status and asynchronous fulfillment projection            |
| Read analytics             | `POST /graphql`            | ClickHouse-backed analytics query                                 |

The checkout page always submits the Storefront GraphQL `checkout` mutation
with `tracedFetch`; it does not select a separate transport or endpoint.

## Span and event names

| Name                     | Kind                                         | Required attributes                                                    |
| ------------------------ | -------------------------------------------- | ---------------------------------------------------------------------- |
| `app.screen.name`        | Short browser span and same-named span event | `app.screen.name`, `url.path`                                          |
| `ui.click`               | Short browser span and same-named span event | `app.screen.name`, `app.widget.name`                                   |
| `ui.submit`              | Short browser span and same-named span event | `app.screen.name`, `app.widget.name`, `quote_id` when checkout submits |
| `web.checkout.submitted` | OTLP log event                               | `event.name`, `item_count`, `quote_id`                                 |
| `browser.web_vital`      | Short browser span and same-named span event | Vital name, value, rating, id, delta, navigation type, screen          |
| OTel exception event     | Span exception event                         | Exception fields emitted by the Web SDK                                |

Checkout failures record `web.checkout.failed` as a browser span with the
error type. The UI does not convert a failed commerce response into a
successful result. Best-effort application analytics write failures are
recorded separately as `web.analytics.record_failed`.

## Route contract

| Route              | Purpose                                                        |
| ------------------ | -------------------------------------------------------------- |
| `/`                | Live featured Catalog browse and cart entry                    |
| `/catalog`         | Searchable Catalog GraphQL browse with server-owned categories |
| `/products/:sku`   | Catalog product, variants, reviews, and current price          |
| `/cart`            | Browser-session SKU/quantity working set                       |
| `/checkout`        | Fresh quote, then Storefront checkout submission               |
| `/orders`          | Customer-filtered durable order list                           |
| `/orders/:orderId` | Durable order detail and asynchronous status timeline          |
| `/analytics`       | ClickHouse-backed analytics query through Storefront GraphQL   |

## Executable Compose smoke

Normal `bun run e2e` remains the local-server suite; its existing tests keep
their request fixtures. The Compose smoke is opt-in and contains no
`page.route` or response interception:

```bash
docker compose -f deploy/docker-compose.yml up -d --build
cd web
bun run e2e:compose
```

`e2e:compose` sets `PLAYGROUND_COMPOSE_E2E=1`, points Playwright at the
Compose web origin `http://localhost:5173`, and skips the local `webServer`
launcher. It verifies live browse → quote/checkout → order status →
ClickHouse analytics. A different exposed web origin requires matching
Storefront CORS configuration and `PLAYGROUND_COMPOSE_BASE_URL`.
