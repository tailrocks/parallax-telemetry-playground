# Same-workload UI bake-off (M4)

Same trace, three UIs. The workload is the deterministic `traces:deep` shape
emitted once through the fan-out collector; `mise run check:fanout` asserted
14/14 spans in Parallax, Jaeger, and OpenObserve before any screenshot was
taken. This doc records what each UI shows for that one trace — factual,
no marketing.

- Date: 2026-09-13 (UTC; lab clock showed 2026-09-14 Asia/Saigon in one shot)
- Trace: `23b34698f09b4d540000000000000001` (`gateway.step_0`, 14 spans)
- Fan-out proof: `PASS parallax: 14/14`, `PASS jaeger: 14/14`,
  `PASS openobserve: 14/14` (same run, same trace id)
- Capture: headless Chrome 153, viewport 1600x1000, no interaction except
  scroll (Parallax tail shot) and the OpenObserve login/query/drill-down flow

## Pinned versions in this pass

| Role | Build |
|---|---|
| Playground | `05820ac` (`feat(macos): join failure/hang/lifecycle on one trace`, origin/main at capture) |
| Parallax server + UI | `626ee330` (`docs: restamp GraphQL Query count 77→78`, debug build + `ui/dist/client` from that checkout) |
| GreptimeDB (external) | `1.1.2` (commit `8ad2d24`) |
| Collector | `otel/opentelemetry-collector-contrib:0.160.0` (digest `799dc6cf…`) |
| Jaeger | `jaegertracing/jaeger:2.20.0` (digest `46a88626…`) |
| OpenObserve | `openobserve/openobserve:v1.0.0` (digest `d581789c…`) |

Lab-only port map (unique to this run; the committed compose/README defaults
are unchanged): Parallax API `26500` / OTLP `26517`+`26518`, Greptime
`26400`–`26403`, collector ingress `26317`+`26318`, Jaeger UI `37686`,
OpenObserve `5180`. The collector config was a host-port-shifted copy of
`otelcol-comparison.yaml` with the Parallax leg pointed at the lab OTLP
ports; nothing else differs.

## Parallax — trace detail

Files: [`trace-parallax-header.png`](bakeoff/trace-parallax-header.png),
[`trace-parallax-waterfall.png`](bakeoff/trace-parallax-waterfall.png)

Access: deep link `/traces/{traceId}`, no login on the token-free lab
instance.

Visible in the shots:

- Header: trace id, `invocation b18e263f-be6` link, service chip
  `playground-shapes`; stat row reads **14 spans, 28ms, 1 service,
  0 errors, 0 logs, 0/0 links/events**.
- Waterfall/Story tabs; Waterfall offers color-by-service plus Tree / Errors /
  Lanes / Flame views. Pin, Critical path, and Compare actions sit above the
  waterfall.
- Right panel: trace id, span count, service, logs; a Span index with all 14
  span-id chips.
- The header shot shows the first ~9 waterfall rows; the second shot (inner
  container scrolled) shows all 14 rows down to `orders.step_13 · 2.0ms`.
  Same nesting and durations as the other two UIs.

## Jaeger — trace detail

File: [`trace-jaeger.png`](bakeoff/trace-jaeger.png)

Access: deep link `/trace/{traceId}`, no login.

Visible in the shot:

- Header: `playground-shapes: gateway.step_0 23b3469`; sub-header reads trace
  start, **Duration 28.0ms, Services 1, Depth 14, Total Spans 14**.
- Minimap strip plus full timeline: all 14 bars in one viewport with
  per-span durations (28ms → 2.0ms), matching the Parallax/OpenObserve shape.
- Left Service & Operation tree (labels truncate in the narrow pane at this
  viewport); view switcher set to Trace Timeline; in-trace Find box.

## OpenObserve — trace list + detail

Files: [`trace-openobserve-list.png`](bakeoff/trace-openobserve-list.png),
[`trace-openobserve-waterfall.png`](bakeoff/trace-openobserve-waterfall.png)

Access: login (`root@example.com`), pick the `default` trace stream, Traces
tab, Run query, click the row. There is no bare `/web/traces/{id}` route —
that URL returns the app's 404 page. The drill-down lands on
`/web/traces/trace-details?stream=default&trace_id={id}&from={µs}&to={µs}&org_identifier=default`,
which is deep-linkable once the microsecond window is known.

Visible in the shots:

- List: "1 Traces Found"; row reads `playground-shapes / gateway.step_0 /
  28.00ms / 14 spans / SUCCESS`, with Rate/Errors/Duration charts above and
  the stream field list (`trace_id`, `span_id`, `operation_name`, …) left.
- Detail: `gateway.step_0`, full trace id, **14 spans, 0 errors** chips;
  Waterfall / Flame Graph / Trace Graph tabs; all 14 bars in one viewport
  with per-span durations; span search box and a logs-stream correlation
  picker.

## Comparison (same trace, same viewport)

| | Parallax | Jaeger 2.20.0 | OpenObserve v1.0.0 |
|---|---|---|---|
| Spans shown | 14 (header + span index) | 14 (header + timeline) | 14 (list + detail chips) |
| Duration shown | 28ms | 28.0ms | 28.00ms |
| All 14 rows in one 1600x1000 viewport | No — inner scroll, ~9 visible | Yes | Yes |
| Deep link | `/traces/{id}` | `/trace/{id}` | trace-details URL with `stream` + `trace_id` + `from/to` window |
| Login for this pass | No (token-free lab) | No | Yes (root user) |
| Extra views on the trace page | Story, Tree/Errors/Lanes/Flame, Critical path, Compare, Pin | View switcher, Find, minimap | Flame Graph, Trace Graph, span search, logs correlation |
| Correlation shown | invocation id link, service chip | service/operation tree | service latency bar, status, stream fields |

Where Parallax leads in this pass (observed, this trace page only):

- Investigation actions inline on the trace: Pin, Critical path, Compare,
  plus a Story tab — Jaeger and OpenObserve show no equivalent on their
  trace pages in these shots.
- Every span id is one click away via the Span index chips; Jaeger's tree
  truncates labels and OpenObserve needs span-search or row clicks.

Where Parallax lags in this pass (observed, same scope):

- Waterfall density: Jaeger and OpenObserve fit all 14 rows in the viewport;
  Parallax needs an inner scroll (~9 rows visible).
- The context pill reads `Local 127.0.0.1:4000` although the lab instance is
  served on port 26500 — cosmetic (data loads from the same origin), but the
  label does not reflect the actual endpoint.

Other observations (competitors, same pass):

- OpenObserve needed four steps (login → stream → run → row click) where the
  other two needed one deep link; during login the app threw two identical
  non-blocking page errors (`Cannot read properties of null (reading
  'getAttribute')`) and still completed login, query, and drill-down.
- Jaeger was the fastest path from cold start to rendered trace (one URL,
  no login, no stream picker).

## Reproduce

```bash
# 1. Parallax on lab ports (token-free, external Greptime on 26400-26403).
# 2. Comparison stack with host ports shifted to 26317/26318/37686/5180 and
#    the collector's Parallax leg pointed at the lab OTLP ports.
# 3. Emit + assert before shooting:
PARALLAX_URL=http://127.0.0.1:26500 FANOUT_OTLP_HTTP=http://127.0.0.1:26318 \
JAEGER_URL=http://127.0.0.1:37686 OO_URL=http://127.0.0.1:5180 \
  mise run check:fanout
# 4. Screenshot the printed trace id in each UI (headless Chrome, 1600x1000):
#    Parallax  http://127.0.0.1:26500/traces/{id}
#    Jaeger    http://127.0.0.1:37686/trace/{id}
#    OpenObserve: login -> stream `default` -> Traces tab -> Run query -> row
```

## Limits of this pass

- One trace shape (`traces:deep`, single service, no errors, no logs). It says
  nothing about multi-service traces, error rendering, or log correlation.
- One viewport (1600x1000), default themes (Parallax dark, Jaeger light,
  OpenObserve light).
- The README workload section (`commerce:checkout_saga`,
  `metrics:cardinality_stress`, `propagation:malformed`) was not screenshotted;
  only the fan-out proof trace was.
- SigNoz / Grafana / Uptrace remain unscaffolded (see README).
