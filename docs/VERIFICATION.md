# Verification contract

This is the current fixture contract for the telemetry playground. It records
what each scenario actually drives; source inspection alone is not a runtime
pass.

## Deterministic identities

Normal scenarios use seeded data:

- Acme: `tenant-acme`, `customer-acme-ava`, `WIDGET-1`, `WIDGET-2`,
  `GADGET-1`, and `GADGET-2`.
- Nova: `tenant-nova`, `customer-nova-mia`, `NOVA-PACK-20`,
  `NOVA-PACK-30`, `NOVA-LAMP-DESK`, and `NOVA-LAMP-FLOOR`.

Every direct checkout fixture sends `currency_code: "USD"`,
`payment_method_token`, and `payment_method_type: "card"`. Failure fixtures
change the explicit provider token or bounded delay/control field only.

## Async boundaries

The real checkout async path is:

```bash
POST /checkout
  → PostgreSQL order + transactional outbox
  → commerce.events / RabbitMQ
  → fulfillment.orders / Java fulfillment
  → PostgreSQL shipment + notifications HTTP
```

`mise run commerce:checkout_saga` and
`mise run messaging:checkout_outbox` exercise this path. They extract the
returned order ID and poll
`/verify/order` until fulfillment is complete. The poll sends both required
identity controls:

```text
Authorization: Bearer $FULFILLMENT_INTERNAL_TOKEN
X-Tenant-Id: <matching tenant>
```

`mise run messaging:seeded_order_replay` and
`mise run messaging:java_fulfillment_replay` use the authenticated fulfillment
operational seam to replay
the seeded `order-acme-1001` and `order-nova-2001` events. This is a real
shared RabbitMQ consumer path, but it is not a checkout-outbox proof.

The orders service `/order` endpoint publishes an intentionally synthetic
message to the private `orders.synthetic` exchange.
`mise run messaging:batch_fanin`, `mise run messaging:poison_retry`,
`mise run messaging:orphan_consumer`, and the synthetic order leg of
`mise run events:typed_business_events` use it for fan-in, lag, poison, orphan,
and typed-event fixtures. These scenarios do not claim fulfillment or
checkout-outbox coverage.

## Focused corpus gates

`mise run verify:commerce_stack` is a separate Compose-configuration and core
HTTP-health check. It does not execute the scenario proofs in this document.

```text
mise run check:scenarios
rtk git diff --check
```

Run `mise run commerce:checkout_saga` or
`mise run messaging:checkout_outbox` against a healthy stack
for real checkout/outbox/fulfillment evidence. Set
`FULFILLMENT_INTERNAL_TOKEN` when the stack does not use the local Compose
value. A Parallax trace query is a separate evidence step; service and queue
co-presence must not be treated as proof of causal span topology.
