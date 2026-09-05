-- Durable fulfillment side effects with an order-version fence.
-- A fulfillment transaction queues shipment/cancellation and notification
-- effects atomically with the order state change. Cancellation supersedes
-- every unclaimed effect before it can be dispatched.

SET TIME ZONE 'UTC';
SET search_path TO public;

CREATE TABLE IF NOT EXISTS fulfillment_effects (
    tenant_id TEXT NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    order_id TEXT NOT NULL,
    effect_key TEXT NOT NULL,
    operation TEXT NOT NULL CHECK (operation IN ('fulfillment', 'cancellation')),
    effect_kind TEXT NOT NULL CHECK (effect_kind IN ('event', 'notification')),
    payload JSONB NOT NULL CHECK (jsonb_typeof(payload) = 'object'),
    notification_status TEXT,
    aggregate_version TIMESTAMPTZ NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('queued', 'publishing', 'published', 'cancelled', 'failed')),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    claim_token TEXT,
    claim_until TIMESTAMPTZ,
    published_at TIMESTAMPTZ,
    cancelled_at TIMESTAMPTZ,
    last_error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (tenant_id, effect_key),
    FOREIGN KEY (tenant_id, order_id)
        REFERENCES orders (tenant_id, id)
        ON DELETE CASCADE,
    CHECK (
        (effect_kind = 'notification' AND notification_status IS NOT NULL)
        OR (effect_kind = 'event' AND notification_status IS NULL)
    ),
    CHECK (
        (status = 'publishing' AND claim_token IS NOT NULL AND claim_until IS NOT NULL)
        OR (status <> 'publishing' AND claim_token IS NULL AND claim_until IS NULL)
    ),
    CHECK ((status = 'published' AND published_at IS NOT NULL)
        OR (status <> 'published' AND published_at IS NULL)),
    CHECK ((status = 'cancelled' AND cancelled_at IS NOT NULL)
        OR (status <> 'cancelled' AND cancelled_at IS NULL))
);

CREATE INDEX IF NOT EXISTS idx_fulfillment_effects_dispatch
    ON fulfillment_effects (tenant_id, order_id, status, created_at, effect_key)
    WHERE status IN ('queued', 'publishing');

CREATE INDEX IF NOT EXISTS idx_fulfillment_effects_claim_expiry
    ON fulfillment_effects (claim_until)
    WHERE status = 'publishing';
