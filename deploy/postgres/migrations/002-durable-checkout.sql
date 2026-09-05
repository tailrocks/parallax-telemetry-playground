-- Durable checkout and inventory coordination.
--
-- This migration is additive. Existing orders, stock counts, and checkout
-- attempt rows remain valid while new writes gain stable identities and
-- retryable compensation state.

ALTER TABLE orders
    ADD COLUMN IF NOT EXISTS checkout_request_id TEXT;

CREATE UNIQUE INDEX IF NOT EXISTS uq_orders_checkout_request
    ON orders (tenant_id, checkout_request_id)
    WHERE checkout_request_id IS NOT NULL;

CREATE TABLE IF NOT EXISTS inventory_reservations (
    reservation_id TEXT NOT NULL,
    tenant_id TEXT NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    sku TEXT NOT NULL,
    variant_id TEXT NOT NULL,
    location_id TEXT NOT NULL,
    quantity INTEGER NOT NULL CHECK (quantity > 0),
    status TEXT NOT NULL CHECK (status IN ('reserved', 'released')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    released_at TIMESTAMPTZ,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (tenant_id, reservation_id),
    FOREIGN KEY (tenant_id, variant_id)
        REFERENCES product_variants (tenant_id, id)
        ON DELETE RESTRICT,
    FOREIGN KEY (tenant_id, location_id)
        REFERENCES inventory_locations (tenant_id, id)
        ON DELETE RESTRICT,
    CHECK (
        (status = 'reserved' AND released_at IS NULL)
        OR (status = 'released' AND released_at IS NOT NULL)
    )
);

CREATE INDEX IF NOT EXISTS idx_inventory_reservations_status
    ON inventory_reservations (tenant_id, status, updated_at DESC);

CREATE INDEX IF NOT EXISTS idx_inventory_reservations_sku
    ON inventory_reservations (tenant_id, sku, status);

CREATE TABLE IF NOT EXISTS checkout_compensation_tasks (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    order_id TEXT NOT NULL,
    task_key TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('inventory_release', 'payment')),
    reservation_id TEXT,
    sku TEXT,
    quantity INTEGER CHECK (quantity IS NULL OR quantity > 0),
    payment_id TEXT,
    request_id TEXT,
    currency TEXT,
    status TEXT NOT NULL CHECK (status IN ('queued', 'processing', 'completed')),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    available_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    claimed_at TIMESTAMPTZ,
    completed_at TIMESTAMPTZ,
    last_error TEXT,
    traceparent TEXT,
    tracestate TEXT,
    baggage TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (tenant_id, task_key),
    FOREIGN KEY (tenant_id, order_id)
        REFERENCES orders (tenant_id, id)
        ON DELETE CASCADE,
    CHECK (
        (kind = 'inventory_release'
            AND reservation_id IS NOT NULL
            AND sku IS NOT NULL
            AND quantity IS NOT NULL
            AND payment_id IS NULL
            AND request_id IS NULL
            AND currency IS NULL)
        OR (kind = 'payment'
            AND reservation_id IS NULL
            AND sku IS NULL
            AND quantity IS NULL
            AND payment_id IS NOT NULL
            AND request_id IS NOT NULL
            AND currency IS NOT NULL)
    ),
    CHECK (
        (status = 'completed' AND completed_at IS NOT NULL)
        OR (status <> 'completed' AND completed_at IS NULL)
    )
);

CREATE INDEX IF NOT EXISTS idx_checkout_compensation_queue
    ON checkout_compensation_tasks (available_at, updated_at)
    WHERE status IN ('queued', 'processing');

CREATE INDEX IF NOT EXISTS idx_checkout_compensation_order
    ON checkout_compensation_tasks (tenant_id, order_id, status);
