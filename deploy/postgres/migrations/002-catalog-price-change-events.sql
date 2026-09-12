-- Durable Catalog price-change journal. LISTEN is only the low-latency wake-up;
-- consumers replay this table by sequence after reconnects or missed notices.

SET TIME ZONE 'UTC';
SET search_path TO public;

CREATE TABLE IF NOT EXISTS price_change_events (
    event_id TEXT PRIMARY KEY,
    sequence BIGINT GENERATED ALWAYS AS IDENTITY NOT NULL UNIQUE,
    tenant_id TEXT NOT NULL,
    sku TEXT NOT NULL,
    product_id TEXT NOT NULL,
    variant_id TEXT NOT NULL,
    price_id TEXT NOT NULL,
    currency TEXT NOT NULL
        CHECK (char_length(currency) = 3 AND currency = upper(currency)),
    amount_minor INTEGER NOT NULL CHECK (amount_minor >= 0),
    compare_at_minor INTEGER
        CHECK (compare_at_minor IS NULL OR compare_at_minor >= amount_minor),
    valid_from TIMESTAMPTZ NOT NULL,
    observed_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (tenant_id, price_id),
    FOREIGN KEY (tenant_id, product_id)
        REFERENCES products (tenant_id, id),
    FOREIGN KEY (tenant_id, variant_id)
        REFERENCES product_variants (tenant_id, id),
    FOREIGN KEY (tenant_id, price_id)
        REFERENCES prices (tenant_id, id)
);

CREATE INDEX IF NOT EXISTS idx_price_change_events_tenant_sku_sequence
    ON price_change_events (tenant_id, sku, sequence);
