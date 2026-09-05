-- Durable RabbitMQ consumer fence. Event identity is scoped by tenant so
-- redelivery and process crashes cannot reset the retry budget in the payload.

SET TIME ZONE 'UTC';
SET search_path TO public;

CREATE TABLE IF NOT EXISTS orders_consumer_inbox (
    tenant_id TEXT NOT NULL,
    event_id TEXT NOT NULL,
    status TEXT NOT NULL CHECK (
        status IN ('processing', 'retry_scheduled', 'completed', 'dead_lettered')
    ),
    attempts INTEGER NOT NULL CHECK (attempts > 0),
    lease_until TIMESTAMPTZ NOT NULL,
    traceparent TEXT,
    tracestate TEXT,
    baggage TEXT,
    last_error TEXT,
    completed_at TIMESTAMPTZ,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (tenant_id, event_id),
    CHECK (status <> 'completed' OR completed_at IS NOT NULL)
);

CREATE INDEX IF NOT EXISTS idx_orders_consumer_inbox_active
    ON orders_consumer_inbox (lease_until, tenant_id, event_id)
    WHERE status IN ('processing', 'retry_scheduled');
