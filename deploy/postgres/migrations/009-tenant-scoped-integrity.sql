-- Bind durable identifiers to the tenant-scoped rows they identify.
-- Nullable child identifiers remain nullable for failed/transport-only records;
-- when present, the tenant and identifier must resolve as one composite key.

SET TIME ZONE 'UTC';
SET search_path TO public;

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE conname = 'price_change_events_tenant_fk'
    ) THEN
        ALTER TABLE price_change_events
            ADD CONSTRAINT price_change_events_tenant_fk
            FOREIGN KEY (tenant_id)
            REFERENCES tenants (id)
            ON DELETE CASCADE;
    END IF;

    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE conname = 'payment_operation_requests_payment_fk'
    ) THEN
        ALTER TABLE payment_operation_requests
            ADD CONSTRAINT payment_operation_requests_payment_fk
            FOREIGN KEY (tenant_id, payment_id)
            REFERENCES payments (tenant_id, id)
            ON DELETE CASCADE;
    END IF;

    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE conname = 'fulfillment_processed_events_order_fk'
    ) THEN
        ALTER TABLE fulfillment_processed_events
            ADD CONSTRAINT fulfillment_processed_events_order_fk
            FOREIGN KEY (tenant_id, order_id)
            REFERENCES orders (tenant_id, id)
            ON DELETE CASCADE;
    END IF;

    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE conname = 'checkout_compensation_tasks_reservation_fk'
    ) THEN
        ALTER TABLE checkout_compensation_tasks
            ADD CONSTRAINT checkout_compensation_tasks_reservation_fk
            FOREIGN KEY (tenant_id, reservation_id)
            REFERENCES inventory_reservations (tenant_id, reservation_id)
            ON DELETE CASCADE;
    END IF;

    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE conname = 'checkout_compensation_tasks_payment_fk'
    ) THEN
        ALTER TABLE checkout_compensation_tasks
            ADD CONSTRAINT checkout_compensation_tasks_payment_fk
            FOREIGN KEY (tenant_id, payment_id)
            REFERENCES payments (tenant_id, id)
            ON DELETE CASCADE;
    END IF;

    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE conname = 'orders_consumer_inbox_tenant_fk'
    ) THEN
        ALTER TABLE orders_consumer_inbox
            ADD CONSTRAINT orders_consumer_inbox_tenant_fk
            FOREIGN KEY (tenant_id)
            REFERENCES tenants (id)
            ON DELETE CASCADE;
    END IF;
END
$$;

CREATE INDEX IF NOT EXISTS idx_checkout_compensation_reservation
    ON checkout_compensation_tasks (tenant_id, reservation_id)
    WHERE reservation_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_checkout_compensation_payment
    ON checkout_compensation_tasks (tenant_id, payment_id)
    WHERE payment_id IS NOT NULL;
