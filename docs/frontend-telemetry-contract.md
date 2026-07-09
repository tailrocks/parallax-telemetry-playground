# Frontend Telemetry Contract

Plan 050 defines the browser/RUM data shape that Parallax can consume later for
`frontendSessions`. The playground emits this via OTLP/HTTP protobuf from the
web app to the same-origin `/v1/traces` proxy, then to Rotel.

## Resource Attributes

Every browser span from `WebTracerProvider` carries:

| Attribute | Value |
|---|---|
| `service.name` | `web` |
| `service.version` | `VITE_RELEASE` or `dev` |
| `deployment.environment.name` | `playground` |
| `session.id` | sessionStorage-backed UUID, stable for one tab session |

`session.id` is also propagated as W3C baggage by the web app's `tracedFetch`
helper so backend spans can be correlated with the browser session. It is not
emitted as a metric label.

## Span And Event Names

| Name | Kind | Required attributes |
|---|---|---|
| `app.screen.name` | short browser span + same-named span event | `app.screen.name`, `url.path` |
| `ui.click` | short browser span + same-named span event | `app.screen.name`, `app.widget.name` |
| `ui.submit` | short browser span + same-named span event | `app.screen.name`, `app.widget.name`, `telemetry.propagation.disabled` |
| `web.checkout.submitted` | OTLP log event | `event.name`, `sku`, `quantity` |
| `browser.web_vital` | short browser span + same-named span event | `web_vital.name`, `web_vital.value`, `web_vital.rating`, `web_vital.id`, `web_vital.delta`, `web_vital.navigation_type`, `app.screen.name` |
| OTel exception event | span exception event | `error.type`, exception fields emitted by the Web SDK |

The `browser.web_vital` name is intentionally stable; the metric name is an
attribute (`CLS`, `FCP`, `INP`, `LCP`, `TTFB`) to avoid per-vital span names.

## Error Story

The home page's `break (RUM error)` button runs a traced checkout fetch that
returns a backend failure, then throws in the browser handler. The active
`ui.click` span records an OTel exception event and ERROR status while the fetch
instrumentation keeps the backend checkout call in the same trace.

## Propagation-Break Story

`/checkout?nopropagate=1` submits with plain `fetch` to
`VITE_CHECKOUT_URL_NOPROP`, defaulting to `http://127.0.0.1:8088`. Normal
submits use `tracedFetch` and `VITE_CHECKOUT_URL`, defaulting to
`http://localhost:8088`. The browser fetch span still exists in the broken case,
but W3C trace/baggage headers are not injected, so the checkout backend starts a
separate trace. This is an intentional broken-continuation gap for
telemetry-quality demos.

## Route Contract

The journey has at least three pages:

| Route | Purpose |
|---|---|
| `/` | journey start, rage-click OTLP signal, browser error story |
| `/checkout` | SKU/quantity form, normal browser-to-backend stitching |
| `/checkout?nopropagate=1` | disconnected frontend/backend trace case |
| `/orders` | browser-to-orders POST with the same CORS/baggage propagation path |
