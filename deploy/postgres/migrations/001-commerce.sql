-- Versioned commerce migration applied by `mise run infra:postgres_migrate`.
-- Re-running this file is safe: schema objects use IF NOT EXISTS and seed
-- records use stable IDs with conflict updates.
-- The runner supplies the transaction; keep transaction control out of files.


SET TIME ZONE 'UTC';
SET search_path TO public;

CREATE TABLE IF NOT EXISTS tenants (
    id TEXT PRIMARY KEY,
    slug TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    default_currency TEXT NOT NULL
        CHECK (char_length(default_currency) = 3
            AND default_currency = upper(default_currency)),
    status TEXT NOT NULL CHECK (status IN ('active', 'suspended')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS categories (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    parent_category_id TEXT,
    slug TEXT NOT NULL,
    name TEXT NOT NULL,
    sort_order INTEGER NOT NULL DEFAULT 0 CHECK (sort_order >= 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (tenant_id, id),
    UNIQUE (tenant_id, slug),
    FOREIGN KEY (tenant_id, parent_category_id)
        REFERENCES categories (tenant_id, id)
        ON DELETE CASCADE,
    CHECK (parent_category_id IS NULL OR parent_category_id <> id)
);

CREATE TABLE IF NOT EXISTS products (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    category_id TEXT NOT NULL,
    slug TEXT NOT NULL,
    name TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    brand TEXT,
    status TEXT NOT NULL CHECK (status IN ('draft', 'active', 'archived')),
    attributes JSONB NOT NULL DEFAULT '{}'::JSONB
        CHECK (jsonb_typeof(attributes) = 'object'),
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (tenant_id, id),
    UNIQUE (tenant_id, slug),
    UNIQUE (tenant_id, category_id, id),
    FOREIGN KEY (tenant_id, category_id)
        REFERENCES categories (tenant_id, id)
        ON DELETE RESTRICT
);

CREATE TABLE IF NOT EXISTS product_variants (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    product_id TEXT NOT NULL,
    sku TEXT NOT NULL,
    name TEXT NOT NULL,
    option_values JSONB NOT NULL DEFAULT '{}'::JSONB
        CHECK (jsonb_typeof(option_values) = 'object'),
    status TEXT NOT NULL CHECK (status IN ('active', 'inactive')),
    weight_grams INTEGER CHECK (weight_grams IS NULL OR weight_grams > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (tenant_id, id),
    UNIQUE (tenant_id, sku),
    UNIQUE (tenant_id, product_id, id),
    FOREIGN KEY (tenant_id, product_id)
        REFERENCES products (tenant_id, id)
        ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS prices (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    variant_id TEXT NOT NULL,
    currency TEXT NOT NULL
        CHECK (char_length(currency) = 3 AND currency = upper(currency)),
    amount_minor INTEGER NOT NULL CHECK (amount_minor >= 0),
    compare_at_minor INTEGER
        CHECK (compare_at_minor IS NULL OR compare_at_minor >= amount_minor),
    valid_from TIMESTAMPTZ NOT NULL,
    valid_to TIMESTAMPTZ,
    is_default BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (tenant_id, id),
    UNIQUE (tenant_id, variant_id, currency, valid_from),
    FOREIGN KEY (tenant_id, variant_id)
        REFERENCES product_variants (tenant_id, id)
        ON DELETE CASCADE,
    CHECK (valid_to IS NULL OR valid_to > valid_from)
);

CREATE TABLE IF NOT EXISTS promotions (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    code TEXT NOT NULL,
    name TEXT NOT NULL,
    discount_type TEXT NOT NULL CHECK (discount_type IN ('percentage', 'fixed')),
    discount_value DOUBLE PRECISION NOT NULL CHECK (discount_value > 0),
    minimum_subtotal_minor INTEGER NOT NULL DEFAULT 0 CHECK (minimum_subtotal_minor >= 0),
    currency TEXT
        CHECK (currency IS NULL OR (char_length(currency) = 3 AND currency = upper(currency))),
    starts_at TIMESTAMPTZ NOT NULL,
    ends_at TIMESTAMPTZ,
    max_redemptions INTEGER CHECK (max_redemptions IS NULL OR max_redemptions > 0),
    redemption_count INTEGER NOT NULL DEFAULT 0 CHECK (redemption_count >= 0),
    active BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (tenant_id, id),
    UNIQUE (tenant_id, code),
    CHECK (ends_at IS NULL OR ends_at > starts_at),
    CHECK (
        (discount_type = 'percentage' AND currency IS NULL AND discount_value <= 100)
        OR (discount_type = 'fixed' AND currency IS NOT NULL)
    ),
    CHECK (max_redemptions IS NULL OR redemption_count <= max_redemptions)
);

CREATE TABLE IF NOT EXISTS promotion_products (
    tenant_id TEXT NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    promotion_id TEXT NOT NULL,
    product_id TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (tenant_id, promotion_id, product_id),
    FOREIGN KEY (tenant_id, promotion_id)
        REFERENCES promotions (tenant_id, id)
        ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, product_id)
        REFERENCES products (tenant_id, id)
        ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS customers (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    email TEXT NOT NULL CHECK (position('@' IN email) > 1),
    first_name TEXT NOT NULL,
    last_name TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('active', 'disabled')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (tenant_id, id)
);

CREATE TABLE IF NOT EXISTS reviews (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    product_id TEXT NOT NULL,
    customer_id TEXT NOT NULL,
    rating SMALLINT NOT NULL CHECK (rating BETWEEN 1 AND 5),
    title TEXT NOT NULL,
    body TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('pending', 'published', 'rejected')),
    verified_purchase BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (tenant_id, id),
    UNIQUE (tenant_id, customer_id, product_id),
    FOREIGN KEY (tenant_id, product_id)
        REFERENCES products (tenant_id, id)
        ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, customer_id)
        REFERENCES customers (tenant_id, id)
        ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS inventory_locations (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    code TEXT NOT NULL,
    name TEXT NOT NULL,
    is_active BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (tenant_id, id),
    UNIQUE (tenant_id, code)
);

CREATE TABLE IF NOT EXISTS inventory (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    variant_id TEXT NOT NULL,
    location_id TEXT NOT NULL,
    on_hand_quantity INTEGER NOT NULL CHECK (on_hand_quantity >= 0),
    reserved_quantity INTEGER NOT NULL DEFAULT 0
        CHECK (reserved_quantity >= 0 AND reserved_quantity <= on_hand_quantity),
    available_quantity INTEGER GENERATED ALWAYS AS
        (on_hand_quantity - reserved_quantity) STORED,
    reorder_point INTEGER NOT NULL DEFAULT 0 CHECK (reorder_point >= 0),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (tenant_id, id),
    UNIQUE (tenant_id, variant_id, location_id),
    FOREIGN KEY (tenant_id, variant_id)
        REFERENCES product_variants (tenant_id, id)
        ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, location_id)
        REFERENCES inventory_locations (tenant_id, id)
        ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS carts (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    customer_id TEXT,
    session_id TEXT,
    status TEXT NOT NULL CHECK (status IN ('active', 'abandoned', 'checked_out')),
    currency TEXT NOT NULL
        CHECK (char_length(currency) = 3 AND currency = upper(currency)),
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    expires_at TIMESTAMPTZ,
    UNIQUE (tenant_id, id),
    UNIQUE (tenant_id, session_id),
    FOREIGN KEY (tenant_id, customer_id)
        REFERENCES customers (tenant_id, id)
        ON DELETE RESTRICT,
    CHECK (customer_id IS NOT NULL OR session_id IS NOT NULL),
    CHECK (expires_at IS NULL OR expires_at > created_at)
);

CREATE TABLE IF NOT EXISTS cart_items (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    cart_id TEXT NOT NULL,
    variant_id TEXT NOT NULL,
    quantity INTEGER NOT NULL CHECK (quantity > 0),
    unit_price_minor INTEGER NOT NULL CHECK (unit_price_minor >= 0),
    discount_minor INTEGER NOT NULL DEFAULT 0
        CHECK (discount_minor >= 0 AND discount_minor <= quantity * unit_price_minor),
    line_total_minor INTEGER GENERATED ALWAYS AS
        (quantity * unit_price_minor - discount_minor) STORED,
    added_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (tenant_id, id),
    UNIQUE (tenant_id, cart_id, variant_id),
    FOREIGN KEY (tenant_id, cart_id)
        REFERENCES carts (tenant_id, id)
        ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, variant_id)
        REFERENCES product_variants (tenant_id, id)
        ON DELETE RESTRICT
);

CREATE TABLE IF NOT EXISTS orders (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    customer_id TEXT,
    cart_id TEXT,
    order_number TEXT NOT NULL,
    status TEXT NOT NULL
        CHECK (status IN ('pending', 'confirmed', 'paid', 'processing',
                         'shipped', 'delivered', 'cancelled', 'refunded')),
    currency TEXT NOT NULL
        CHECK (char_length(currency) = 3 AND currency = upper(currency)),
    subtotal_minor INTEGER NOT NULL CHECK (subtotal_minor >= 0),
    discount_minor INTEGER NOT NULL CHECK (discount_minor >= 0 AND discount_minor <= subtotal_minor),
    tax_minor INTEGER NOT NULL CHECK (tax_minor >= 0),
    shipping_minor INTEGER NOT NULL CHECK (shipping_minor >= 0),
    total_minor INTEGER NOT NULL CHECK (total_minor >= 0),
    shipping_address JSONB NOT NULL DEFAULT '{}'::JSONB
        CHECK (jsonb_typeof(shipping_address) = 'object'),
    billing_address JSONB NOT NULL DEFAULT '{}'::JSONB
        CHECK (jsonb_typeof(billing_address) = 'object'),
    placed_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (tenant_id, id),
    UNIQUE (tenant_id, order_number),
    FOREIGN KEY (tenant_id, customer_id)
        REFERENCES customers (tenant_id, id)
        ON DELETE RESTRICT,
    FOREIGN KEY (tenant_id, cart_id)
        REFERENCES carts (tenant_id, id)
        ON DELETE RESTRICT,
    CHECK (total_minor = subtotal_minor - discount_minor + tax_minor + shipping_minor),
    CHECK (placed_at IS NULL OR placed_at >= created_at)
);

CREATE TABLE IF NOT EXISTS checkout_attempts (
    tenant_id TEXT NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    request_id TEXT NOT NULL,
    request_fingerprint TEXT NOT NULL,
    status TEXT NOT NULL
        CHECK (status IN ('started', 'pending', 'paid', 'degraded', 'failed')),
    order_id TEXT,
    response_payload JSONB,
    error_status SMALLINT,
    error_code TEXT,
    error_message TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (tenant_id, request_id),
    FOREIGN KEY (tenant_id, order_id)
        REFERENCES orders (tenant_id, id)
        ON DELETE CASCADE,
    CHECK (
        (status IN ('started', 'failed') AND response_payload IS NULL)
        OR (status IN ('pending', 'paid', 'degraded') AND response_payload IS NOT NULL)
    ),
    CHECK (
        (status = 'failed' AND error_status IS NOT NULL AND error_code IS NOT NULL)
        OR (status <> 'failed' AND error_status IS NULL AND error_code IS NULL AND error_message IS NULL)
    )
);

CREATE TABLE IF NOT EXISTS order_items (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    order_id TEXT NOT NULL,
    product_id TEXT NOT NULL,
    variant_id TEXT NOT NULL,
    sku TEXT NOT NULL,
    product_name TEXT NOT NULL,
    quantity INTEGER NOT NULL CHECK (quantity > 0),
    unit_price_minor INTEGER NOT NULL CHECK (unit_price_minor >= 0),
    discount_minor INTEGER NOT NULL DEFAULT 0
        CHECK (discount_minor >= 0 AND discount_minor <= quantity * unit_price_minor),
    line_total_minor INTEGER GENERATED ALWAYS AS
        (quantity * unit_price_minor - discount_minor) STORED,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (tenant_id, id),
    UNIQUE (tenant_id, order_id, variant_id),
    FOREIGN KEY (tenant_id, order_id)
        REFERENCES orders (tenant_id, id)
        ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, product_id)
        REFERENCES products (tenant_id, id)
        ON DELETE RESTRICT,
    FOREIGN KEY (tenant_id, product_id, variant_id)
        REFERENCES product_variants (tenant_id, product_id, id)
        ON DELETE RESTRICT
);

CREATE TABLE IF NOT EXISTS payments (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    order_id TEXT NOT NULL,
    provider TEXT NOT NULL,
    provider_reference TEXT,
    status TEXT NOT NULL
        CHECK (status IN ('pending', 'authorized', 'captured', 'partially_refunded',
                         'failed', 'refunded', 'voided')),
    amount_minor INTEGER NOT NULL CHECK (amount_minor >= 0),
    currency TEXT NOT NULL
        CHECK (char_length(currency) = 3 AND currency = upper(currency)),
    method_type TEXT NOT NULL DEFAULT 'card'
        CHECK (method_type IN ('card', 'bank_account', 'wallet', 'unspecified')),
    captured_amount_minor INTEGER NOT NULL DEFAULT 0
        CHECK (captured_amount_minor >= 0 AND captured_amount_minor <= amount_minor),
    refunded_amount_minor INTEGER NOT NULL DEFAULT 0
        CHECK (refunded_amount_minor >= 0 AND refunded_amount_minor <= captured_amount_minor),
    failure_code TEXT,
    authorized_at TIMESTAMPTZ,
    captured_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (tenant_id, id),
    UNIQUE (tenant_id, provider, provider_reference),
    FOREIGN KEY (tenant_id, order_id)
        REFERENCES orders (tenant_id, id)
        ON DELETE CASCADE,
    CHECK (status <> 'captured' OR captured_at IS NOT NULL),
    CHECK (status <> 'failed' OR failure_code IS NOT NULL),
    CHECK (captured_at IS NULL OR authorized_at IS NULL OR captured_at >= authorized_at)
);

CREATE UNIQUE INDEX IF NOT EXISTS uq_payments_one_active_order
    ON payments (tenant_id, order_id)
    WHERE status IN ('pending', 'authorized', 'captured', 'partially_refunded');

CREATE TABLE IF NOT EXISTS shipments (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    order_id TEXT NOT NULL,
    shipment_number INTEGER NOT NULL CHECK (shipment_number > 0),
    location_id TEXT NOT NULL,
    carrier TEXT NOT NULL,
    service_level TEXT NOT NULL,
    tracking_number TEXT,
    status TEXT NOT NULL
        CHECK (status IN ('pending', 'label_created', 'in_transit', 'delivered',
                         'cancelled', 'returned')),
    shipping_address JSONB NOT NULL
        CHECK (jsonb_typeof(shipping_address) = 'object'),
    shipped_at TIMESTAMPTZ,
    delivered_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (tenant_id, id),
    UNIQUE (tenant_id, order_id, shipment_number),
    UNIQUE (tenant_id, tracking_number),
    FOREIGN KEY (tenant_id, order_id)
        REFERENCES orders (tenant_id, id)
        ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, location_id)
        REFERENCES inventory_locations (tenant_id, id)
        ON DELETE RESTRICT,
    CHECK (status NOT IN ('in_transit', 'delivered') OR shipped_at IS NOT NULL),
    CHECK (status <> 'delivered' OR delivered_at IS NOT NULL),
    CHECK (delivered_at IS NULL OR shipped_at IS NOT NULL)
);

CREATE TABLE IF NOT EXISTS payment_operation_requests (
    tenant_id TEXT NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    request_id TEXT NOT NULL,
    operation TEXT NOT NULL CHECK (operation IN ('authorize', 'capture', 'void', 'refund')),
    request_fingerprint TEXT NOT NULL,
    payment_id TEXT,
    response_payload BYTEA,
    grpc_status TEXT,
    failure_reason TEXT,
    error_message TEXT,
    operation_amount_minor BIGINT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (tenant_id, request_id),
    CHECK ((response_payload IS NOT NULL AND grpc_status IS NULL)
        OR (response_payload IS NULL AND grpc_status IS NOT NULL))
);

CREATE INDEX IF NOT EXISTS idx_payment_operation_requests_payment
    ON payment_operation_requests (tenant_id, payment_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_checkout_attempts_order
    ON checkout_attempts (tenant_id, order_id, updated_at DESC);

CREATE TABLE IF NOT EXISTS shipment_items (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    shipment_id TEXT NOT NULL,
    order_item_id TEXT NOT NULL,
    quantity INTEGER NOT NULL CHECK (quantity > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (tenant_id, id),
    UNIQUE (tenant_id, shipment_id, order_item_id),
    FOREIGN KEY (tenant_id, shipment_id)
        REFERENCES shipments (tenant_id, id)
        ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, order_item_id)
        REFERENCES order_items (tenant_id, id)
        ON DELETE RESTRICT
);

CREATE TABLE IF NOT EXISTS outbox_events (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    event_key TEXT NOT NULL,
    aggregate_type TEXT NOT NULL,
    aggregate_id TEXT NOT NULL,
    event_type TEXT NOT NULL,
    schema_version INTEGER NOT NULL DEFAULT 1 CHECK (schema_version > 0),
    occurred_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    payload JSONB NOT NULL CHECK (jsonb_typeof(payload) = 'object'),
    -- W3C propagation belongs to the message carrier. These columns preserve
    -- the originating carrier until the background publisher injects it into
    -- RabbitMQ headers; the event payload remains business data only.
    traceparent TEXT,
    tracestate TEXT,
    baggage TEXT,
    status TEXT NOT NULL CHECK (status IN ('queued', 'processing', 'published', 'failed')),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    available_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    published_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (tenant_id, id),
    UNIQUE (tenant_id, event_key),
    CHECK (status <> 'published' OR published_at IS NOT NULL)
);

CREATE TABLE IF NOT EXISTS feature_exposures (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    customer_id TEXT,
    anonymous_id TEXT,
    session_id TEXT,
    feature_key TEXT NOT NULL,
    variant TEXT NOT NULL,
    exposure_key TEXT NOT NULL,
    context JSONB NOT NULL DEFAULT '{}'::JSONB
        CHECK (jsonb_typeof(context) = 'object'),
    exposed_at TIMESTAMPTZ NOT NULL,
    UNIQUE (tenant_id, id),
    UNIQUE (tenant_id, exposure_key),
    FOREIGN KEY (tenant_id, customer_id)
        REFERENCES customers (tenant_id, id)
        ON DELETE RESTRICT,
    CHECK (customer_id IS NOT NULL OR anonymous_id IS NOT NULL)
);

CREATE TABLE IF NOT EXISTS analytics_events (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    event_key TEXT NOT NULL,
    customer_id TEXT,
    session_id TEXT,
    event_name TEXT NOT NULL,
    event_version SMALLINT NOT NULL DEFAULT 1 CHECK (event_version > 0),
    source TEXT NOT NULL
        CHECK (source IN ('web', 'storefront', 'checkout', 'catalog', 'inventory',
                          'payment', 'fulfillment', 'system')),
    entity_type TEXT NOT NULL DEFAULT '',
    entity_id TEXT NOT NULL DEFAULT '',
    occurred_at TIMESTAMPTZ NOT NULL,
    received_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    trace_id TEXT,
    span_id TEXT,
    properties JSONB NOT NULL DEFAULT '{}'::JSONB
        CHECK (jsonb_typeof(properties) = 'object'),
    context JSONB NOT NULL DEFAULT '{}'::JSONB
        CHECK (jsonb_typeof(context) = 'object'),
    UNIQUE (tenant_id, id),
    UNIQUE (tenant_id, event_key),
    FOREIGN KEY (tenant_id, customer_id)
        REFERENCES customers (tenant_id, id)
        ON DELETE RESTRICT
);

CREATE TABLE IF NOT EXISTS notification_deliveries (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    event_key TEXT NOT NULL,
    order_id TEXT NOT NULL,
    channel TEXT NOT NULL CHECK (channel IN ('email', 'webhook', 'in_app')),
    status TEXT NOT NULL CHECK (status IN ('accepted', 'delivered', 'failed')),
    payload JSONB NOT NULL DEFAULT '{}'::JSONB
        CHECK (jsonb_typeof(payload) = 'object'),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    delivered_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (tenant_id, id),
    UNIQUE (tenant_id, event_key),
    FOREIGN KEY (tenant_id, order_id)
        REFERENCES orders (tenant_id, id)
        ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_categories_tenant_parent
    ON categories (tenant_id, parent_category_id, sort_order);

CREATE INDEX IF NOT EXISTS idx_products_tenant_category_status
    ON products (tenant_id, category_id, status);

CREATE INDEX IF NOT EXISTS idx_variants_tenant_product_status
    ON product_variants (tenant_id, product_id, status);

CREATE INDEX IF NOT EXISTS idx_prices_variant_validity
    ON prices (tenant_id, variant_id, valid_from DESC);

CREATE UNIQUE INDEX IF NOT EXISTS idx_prices_one_current_default
    ON prices (tenant_id, variant_id, currency)
    WHERE is_default AND valid_to IS NULL;

CREATE INDEX IF NOT EXISTS idx_promotions_active_window
    ON promotions (tenant_id, starts_at, ends_at)
    WHERE active;

CREATE INDEX IF NOT EXISTS idx_promotion_products_product
    ON promotion_products (tenant_id, product_id);

CREATE UNIQUE INDEX IF NOT EXISTS idx_customers_tenant_email
    ON customers (tenant_id, lower(email));

CREATE INDEX IF NOT EXISTS idx_reviews_product_published
    ON reviews (tenant_id, product_id, created_at DESC)
    WHERE status = 'published';

CREATE INDEX IF NOT EXISTS idx_inventory_variant
    ON inventory (tenant_id, variant_id);

CREATE INDEX IF NOT EXISTS idx_inventory_location
    ON inventory (tenant_id, location_id);

CREATE INDEX IF NOT EXISTS idx_carts_customer_status
    ON carts (tenant_id, customer_id, status, updated_at DESC);

CREATE INDEX IF NOT EXISTS idx_cart_items_cart
    ON cart_items (tenant_id, cart_id);

CREATE INDEX IF NOT EXISTS idx_orders_customer_created
    ON orders (tenant_id, customer_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_orders_status_created
    ON orders (tenant_id, status, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_order_items_order
    ON order_items (tenant_id, order_id);

CREATE INDEX IF NOT EXISTS idx_payments_order_status
    ON payments (tenant_id, order_id, status);

CREATE INDEX IF NOT EXISTS idx_shipments_order_status
    ON shipments (tenant_id, order_id, status);

CREATE INDEX IF NOT EXISTS idx_outbox_pending
    ON outbox_events (available_at, created_at)
    WHERE status IN ('queued', 'failed');

CREATE INDEX IF NOT EXISTS idx_outbox_aggregate
    ON outbox_events (tenant_id, aggregate_type, aggregate_id, created_at);

-- Durable claims for the fulfillment consumers.  This belongs to the clean
-- bootstrap so a service never mutates schema at application startup.
CREATE TABLE IF NOT EXISTS fulfillment_processed_events (
    consumer_name TEXT NOT NULL,
    event_key TEXT NOT NULL,
    tenant_id TEXT NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    event_type TEXT NOT NULL,
    order_id TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('processing', 'failed', 'completed')),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    last_error TEXT,
    claimed_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    completed_at TIMESTAMPTZ,
    PRIMARY KEY (consumer_name, event_key)
);

CREATE INDEX IF NOT EXISTS idx_fulfillment_processed_order
    ON fulfillment_processed_events (tenant_id, order_id, claimed_at DESC);

CREATE INDEX IF NOT EXISTS idx_feature_exposures_feature_time
    ON feature_exposures (tenant_id, feature_key, exposed_at DESC);

CREATE INDEX IF NOT EXISTS idx_feature_exposures_customer_time
    ON feature_exposures (tenant_id, customer_id, exposed_at DESC);

CREATE INDEX IF NOT EXISTS idx_analytics_events_name_time
    ON analytics_events (tenant_id, event_name, occurred_at DESC);

CREATE INDEX IF NOT EXISTS idx_analytics_events_entity_time
    ON analytics_events (tenant_id, entity_type, entity_id, occurred_at DESC);

CREATE INDEX IF NOT EXISTS idx_analytics_events_trace
    ON analytics_events (trace_id)
    WHERE trace_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_notification_deliveries_order
    ON notification_deliveries (tenant_id, order_id, created_at DESC);

INSERT INTO tenants (id, slug, name, default_currency, status, created_at)
VALUES
    ('tenant-acme', 'acme', 'Acme Home & Work', 'USD', 'active', TIMESTAMPTZ '2026-01-01 09:00:00+00'),
    ('tenant-nova', 'nova', 'Nova Outdoor Supply', 'USD', 'active', TIMESTAMPTZ '2026-01-02 09:00:00+00')
ON CONFLICT (id) DO UPDATE SET
    slug = EXCLUDED.slug,
    name = EXCLUDED.name,
    default_currency = EXCLUDED.default_currency,
    status = EXCLUDED.status,
    created_at = EXCLUDED.created_at;

INSERT INTO categories (id, tenant_id, parent_category_id, slug, name, sort_order, created_at)
VALUES
    ('cat-acme-home', 'tenant-acme', NULL, 'home-work', 'Home & Work', 0, TIMESTAMPTZ '2026-01-01 09:05:00+00'),
    ('cat-acme-kitchen', 'tenant-acme', 'cat-acme-home', 'kitchen', 'Kitchen', 1, TIMESTAMPTZ '2026-01-01 09:06:00+00'),
    ('cat-acme-electronics', 'tenant-acme', 'cat-acme-home', 'electronics', 'Electronics', 2, TIMESTAMPTZ '2026-01-01 09:07:00+00'),
    ('cat-nova-outdoor', 'tenant-nova', NULL, 'outdoor-travel', 'Outdoor & Travel', 0, TIMESTAMPTZ '2026-01-02 09:05:00+00'),
    ('cat-nova-packs', 'tenant-nova', 'cat-nova-outdoor', 'packs', 'Packs', 1, TIMESTAMPTZ '2026-01-02 09:06:00+00'),
    ('cat-nova-lighting', 'tenant-nova', 'cat-nova-outdoor', 'lighting', 'Lighting', 2, TIMESTAMPTZ '2026-01-02 09:07:00+00')
ON CONFLICT (id) DO UPDATE SET
    tenant_id = EXCLUDED.tenant_id,
    parent_category_id = EXCLUDED.parent_category_id,
    slug = EXCLUDED.slug,
    name = EXCLUDED.name,
    sort_order = EXCLUDED.sort_order,
    created_at = EXCLUDED.created_at;

INSERT INTO products
    (id, tenant_id, category_id, slug, name, description, brand, status, attributes, created_at, updated_at)
VALUES
    ('prod-acme-widget', 'tenant-acme', 'cat-acme-kitchen', 'everyday-widget',
     'Everyday Widget', 'A dependable widget for daily tasks.', 'Acme', 'active',
     '{"material":"steel","featured":true}'::JSONB,
     TIMESTAMPTZ '2026-01-03 09:00:00+00', TIMESTAMPTZ '2026-01-03 09:00:00+00'),
    ('prod-acme-gadget', 'tenant-acme', 'cat-acme-electronics', 'smart-gadget',
     'Smart Gadget', 'A compact connected gadget for the home.', 'Acme', 'active',
     '{"connectivity":"wifi","featured":true}'::JSONB,
     TIMESTAMPTZ '2026-01-03 09:01:00+00', TIMESTAMPTZ '2026-01-03 09:01:00+00'),
    ('prod-nova-backpack', 'tenant-nova', 'cat-nova-packs', 'trail-pack',
     'Trail Pack', 'A weather-resistant day pack for short adventures.', 'Nova', 'active',
     '{"capacity_liters":20,"waterproof":true}'::JSONB,
     TIMESTAMPTZ '2026-01-04 09:00:00+00', TIMESTAMPTZ '2026-01-04 09:00:00+00'),
    ('prod-nova-lamp', 'tenant-nova', 'cat-nova-lighting', 'camp-lamp',
     'Camp Lamp', 'Rechargeable warm light for camp and patio.', 'Nova', 'active',
     '{"lumens":800,"rechargeable":true}'::JSONB,
     TIMESTAMPTZ '2026-01-04 09:01:00+00', TIMESTAMPTZ '2026-01-04 09:01:00+00')
ON CONFLICT (id) DO UPDATE SET
    tenant_id = EXCLUDED.tenant_id,
    category_id = EXCLUDED.category_id,
    slug = EXCLUDED.slug,
    name = EXCLUDED.name,
    description = EXCLUDED.description,
    brand = EXCLUDED.brand,
    status = EXCLUDED.status,
    attributes = EXCLUDED.attributes,
    created_at = EXCLUDED.created_at,
    updated_at = EXCLUDED.updated_at;

INSERT INTO product_variants
    (id, tenant_id, product_id, sku, name, option_values, status, weight_grams, created_at)
VALUES
    ('var-acme-widget-1', 'tenant-acme', 'prod-acme-widget', 'WIDGET-1',
     'Everyday Widget / Standard', '{"finish":"silver","size":"standard"}'::JSONB,
     'active', 180, TIMESTAMPTZ '2026-01-03 10:00:00+00'),
    ('var-acme-widget-2', 'tenant-acme', 'prod-acme-widget', 'WIDGET-2',
     'Everyday Widget / Large', '{"finish":"graphite","size":"large"}'::JSONB,
     'active', 240, TIMESTAMPTZ '2026-01-03 10:01:00+00'),
    ('var-acme-gadget-1', 'tenant-acme', 'prod-acme-gadget', 'GADGET-1',
     'Smart Gadget / White', '{"color":"white"}'::JSONB,
     'active', 320, TIMESTAMPTZ '2026-01-03 10:02:00+00'),
    ('var-acme-gadget-2', 'tenant-acme', 'prod-acme-gadget', 'GADGET-2',
     'Smart Gadget / Black', '{"color":"black"}'::JSONB,
     'active', 320, TIMESTAMPTZ '2026-01-03 10:03:00+00'),
    ('var-nova-pack-20', 'tenant-nova', 'prod-nova-backpack', 'NOVA-PACK-20',
     'Trail Pack / 20L', '{"capacity_liters":20,"color":"pine"}'::JSONB,
     'active', 760, TIMESTAMPTZ '2026-01-04 10:00:00+00'),
    ('var-nova-pack-30', 'tenant-nova', 'prod-nova-backpack', 'NOVA-PACK-30',
     'Trail Pack / 30L', '{"capacity_liters":30,"color":"slate"}'::JSONB,
     'active', 920, TIMESTAMPTZ '2026-01-04 10:01:00+00'),
    ('var-nova-lamp-desk', 'tenant-nova', 'prod-nova-lamp', 'NOVA-LAMP-DESK',
     'Camp Lamp / Desk', '{"mount":"desk","color":"amber"}'::JSONB,
     'active', 410, TIMESTAMPTZ '2026-01-04 10:02:00+00'),
    ('var-nova-lamp-floor', 'tenant-nova', 'prod-nova-lamp', 'NOVA-LAMP-FLOOR',
     'Camp Lamp / Floor', '{"mount":"floor","color":"amber"}'::JSONB,
     'active', 880, TIMESTAMPTZ '2026-01-04 10:03:00+00')
ON CONFLICT (id) DO UPDATE SET
    tenant_id = EXCLUDED.tenant_id,
    product_id = EXCLUDED.product_id,
    sku = EXCLUDED.sku,
    name = EXCLUDED.name,
    option_values = EXCLUDED.option_values,
    status = EXCLUDED.status,
    weight_grams = EXCLUDED.weight_grams,
    created_at = EXCLUDED.created_at;

INSERT INTO prices
    (id, tenant_id, variant_id, currency, amount_minor, compare_at_minor,
     valid_from, valid_to, is_default, created_at)
VALUES
    ('price-acme-widget-1-usd', 'tenant-acme', 'var-acme-widget-1', 'USD', 1999, 2299,
     TIMESTAMPTZ '2026-01-03 10:00:00+00', NULL, TRUE, TIMESTAMPTZ '2026-01-03 10:00:00+00'),
    ('price-acme-widget-2-usd', 'tenant-acme', 'var-acme-widget-2', 'USD', 2499, 2799,
     TIMESTAMPTZ '2026-01-03 10:01:00+00', NULL, TRUE, TIMESTAMPTZ '2026-01-03 10:01:00+00'),
    ('price-acme-gadget-1-usd', 'tenant-acme', 'var-acme-gadget-1', 'USD', 4999, 5499,
     TIMESTAMPTZ '2026-01-03 10:02:00+00', NULL, TRUE, TIMESTAMPTZ '2026-01-03 10:02:00+00'),
    ('price-acme-gadget-2-usd', 'tenant-acme', 'var-acme-gadget-2', 'USD', 6999, NULL,
     TIMESTAMPTZ '2026-01-03 10:03:00+00', NULL, TRUE, TIMESTAMPTZ '2026-01-03 10:03:00+00'),
    ('price-nova-pack-20-usd', 'tenant-nova', 'var-nova-pack-20', 'USD', 7999, 8999,
     TIMESTAMPTZ '2026-01-04 10:00:00+00', NULL, TRUE, TIMESTAMPTZ '2026-01-04 10:00:00+00'),
    ('price-nova-pack-30-usd', 'tenant-nova', 'var-nova-pack-30', 'USD', 9999, NULL,
     TIMESTAMPTZ '2026-01-04 10:01:00+00', NULL, TRUE, TIMESTAMPTZ '2026-01-04 10:01:00+00'),
    ('price-nova-lamp-desk-usd', 'tenant-nova', 'var-nova-lamp-desk', 'USD', 4999, 5799,
     TIMESTAMPTZ '2026-01-04 10:02:00+00', NULL, TRUE, TIMESTAMPTZ '2026-01-04 10:02:00+00'),
    ('price-nova-lamp-floor-usd', 'tenant-nova', 'var-nova-lamp-floor', 'USD', 8999, NULL,
     TIMESTAMPTZ '2026-01-04 10:03:00+00', NULL, TRUE, TIMESTAMPTZ '2026-01-04 10:03:00+00')
ON CONFLICT (id) DO UPDATE SET
    tenant_id = EXCLUDED.tenant_id,
    variant_id = EXCLUDED.variant_id,
    currency = EXCLUDED.currency,
    amount_minor = EXCLUDED.amount_minor,
    compare_at_minor = EXCLUDED.compare_at_minor,
    valid_from = EXCLUDED.valid_from,
    valid_to = EXCLUDED.valid_to,
    is_default = EXCLUDED.is_default,
    created_at = EXCLUDED.created_at;

INSERT INTO promotions
    (id, tenant_id, code, name, discount_type, discount_value, minimum_subtotal_minor,
     currency, starts_at, ends_at, max_redemptions, redemption_count, active, created_at)
VALUES
    ('promo-acme-10', 'tenant-acme', 'ACME10', 'Acme welcome discount',
     'percentage', 10, 0, NULL, TIMESTAMPTZ '2026-01-01 00:00:00+00',
     TIMESTAMPTZ '2026-12-31 23:59:59+00', 1000, 12, TRUE, TIMESTAMPTZ '2026-01-05 09:00:00+00'),
    ('promo-acme-500', 'tenant-acme', 'ACME500', 'Acme gadget credit',
     'fixed', 500, 5000, 'USD', TIMESTAMPTZ '2026-01-01 00:00:00+00',
     TIMESTAMPTZ '2026-06-30 23:59:59+00', 250, 4, FALSE, TIMESTAMPTZ '2026-01-05 09:01:00+00'),
    ('promo-nova-15', 'tenant-nova', 'NOVA15', 'Nova trail-season offer',
     'percentage', 15, 0, NULL, TIMESTAMPTZ '2026-01-01 00:00:00+00',
     TIMESTAMPTZ '2026-09-30 23:59:59+00', 500, 7, TRUE, TIMESTAMPTZ '2026-01-06 09:00:00+00')
ON CONFLICT (id) DO UPDATE SET
    tenant_id = EXCLUDED.tenant_id,
    code = EXCLUDED.code,
    name = EXCLUDED.name,
    discount_type = EXCLUDED.discount_type,
    discount_value = EXCLUDED.discount_value,
    minimum_subtotal_minor = EXCLUDED.minimum_subtotal_minor,
    currency = EXCLUDED.currency,
    starts_at = EXCLUDED.starts_at,
    ends_at = EXCLUDED.ends_at,
    max_redemptions = EXCLUDED.max_redemptions,
    redemption_count = EXCLUDED.redemption_count,
    active = EXCLUDED.active,
    created_at = EXCLUDED.created_at;

INSERT INTO promotion_products (tenant_id, promotion_id, product_id)
VALUES
    ('tenant-acme', 'promo-acme-10', 'prod-acme-widget'),
    ('tenant-acme', 'promo-acme-10', 'prod-acme-gadget'),
    ('tenant-acme', 'promo-acme-500', 'prod-acme-gadget'),
    ('tenant-nova', 'promo-nova-15', 'prod-nova-backpack'),
    ('tenant-nova', 'promo-nova-15', 'prod-nova-lamp')
ON CONFLICT (tenant_id, promotion_id, product_id) DO NOTHING;

INSERT INTO customers
    (id, tenant_id, email, first_name, last_name, status, created_at)
VALUES
    ('customer-acme-ava', 'tenant-acme', 'ava@example.test', 'Ava', 'Chen',
     'active', TIMESTAMPTZ '2026-01-07 09:00:00+00'),
    ('customer-acme-liam', 'tenant-acme', 'liam@example.test', 'Liam', 'Patel',
     'active', TIMESTAMPTZ '2026-01-07 09:01:00+00'),
    ('customer-nova-mia', 'tenant-nova', 'mia@example.test', 'Mia', 'Rivera',
     'active', TIMESTAMPTZ '2026-01-08 09:00:00+00')
ON CONFLICT (id) DO UPDATE SET
    tenant_id = EXCLUDED.tenant_id,
    email = EXCLUDED.email,
    first_name = EXCLUDED.first_name,
    last_name = EXCLUDED.last_name,
    status = EXCLUDED.status,
    created_at = EXCLUDED.created_at;

INSERT INTO reviews
    (id, tenant_id, product_id, customer_id, rating, title, body, status,
     verified_purchase, created_at, updated_at)
VALUES
    ('review-acme-widget-ava', 'tenant-acme', 'prod-acme-widget', 'customer-acme-ava',
     5, 'Solid everyday tool', 'Simple, sturdy, and exactly as described.',
     'published', TRUE, TIMESTAMPTZ '2026-01-20 12:00:00+00', TIMESTAMPTZ '2026-01-20 12:00:00+00'),
    ('review-acme-gadget-liam', 'tenant-acme', 'prod-acme-gadget', 'customer-acme-liam',
     4, 'Useful little gadget', 'Setup was quick and the controls are clear.',
     'published', TRUE, TIMESTAMPTZ '2026-01-21 12:00:00+00', TIMESTAMPTZ '2026-01-21 12:00:00+00'),
    ('review-nova-pack-mia', 'tenant-nova', 'prod-nova-backpack', 'customer-nova-mia',
     5, 'Great day pack', 'Comfortable straps and enough room for a full day.',
     'published', TRUE, TIMESTAMPTZ '2026-01-22 12:00:00+00', TIMESTAMPTZ '2026-01-22 12:00:00+00'),
    ('review-nova-lamp-mia', 'tenant-nova', 'prod-nova-lamp', 'customer-nova-mia',
     4, 'Warm light', 'The battery lasted through a long evening outside.',
     'published', TRUE, TIMESTAMPTZ '2026-01-23 12:00:00+00', TIMESTAMPTZ '2026-01-23 12:00:00+00'),
    ('review-acme-widget-liam', 'tenant-acme', 'prod-acme-widget', 'customer-acme-liam',
     3, 'Good value', 'Works well, though the larger size would suit my desk better.',
     'pending', FALSE, TIMESTAMPTZ '2026-01-24 12:00:00+00', TIMESTAMPTZ '2026-01-24 12:00:00+00')
ON CONFLICT (id) DO UPDATE SET
    tenant_id = EXCLUDED.tenant_id,
    product_id = EXCLUDED.product_id,
    customer_id = EXCLUDED.customer_id,
    rating = EXCLUDED.rating,
    title = EXCLUDED.title,
    body = EXCLUDED.body,
    status = EXCLUDED.status,
    verified_purchase = EXCLUDED.verified_purchase,
    created_at = EXCLUDED.created_at,
    updated_at = EXCLUDED.updated_at;

INSERT INTO inventory_locations
    (id, tenant_id, code, name, is_active, created_at)
VALUES
    ('loc-acme-east', 'tenant-acme', 'ACME-EAST', 'Acme East Warehouse', TRUE,
     TIMESTAMPTZ '2026-01-09 09:00:00+00'),
    ('loc-acme-west', 'tenant-acme', 'ACME-WEST', 'Acme West Warehouse', TRUE,
     TIMESTAMPTZ '2026-01-09 09:01:00+00'),
    ('loc-nova-central', 'tenant-nova', 'NOVA-CENTRAL', 'Nova Central Warehouse', TRUE,
     TIMESTAMPTZ '2026-01-10 09:00:00+00')
ON CONFLICT (id) DO UPDATE SET
    tenant_id = EXCLUDED.tenant_id,
    code = EXCLUDED.code,
    name = EXCLUDED.name,
    is_active = EXCLUDED.is_active,
    created_at = EXCLUDED.created_at;

INSERT INTO inventory
    (id, tenant_id, variant_id, location_id, on_hand_quantity, reserved_quantity,
     reorder_point, updated_at)
VALUES
    ('inventory-acme-widget-1-east', 'tenant-acme', 'var-acme-widget-1', 'loc-acme-east',
     100, 4, 20, TIMESTAMPTZ '2026-01-25 09:00:00+00'),
    ('inventory-acme-widget-2-west', 'tenant-acme', 'var-acme-widget-2', 'loc-acme-west',
     80, 0, 15, TIMESTAMPTZ '2026-01-25 09:01:00+00'),
    ('inventory-acme-gadget-1-east', 'tenant-acme', 'var-acme-gadget-1', 'loc-acme-east',
     40, 2, 10, TIMESTAMPTZ '2026-01-25 09:02:00+00'),
    ('inventory-acme-gadget-2-west', 'tenant-acme', 'var-acme-gadget-2', 'loc-acme-west',
     25, 0, 8, TIMESTAMPTZ '2026-01-25 09:03:00+00'),
    ('inventory-nova-pack-20-central', 'tenant-nova', 'var-nova-pack-20', 'loc-nova-central',
     60, 5, 12, TIMESTAMPTZ '2026-01-25 09:04:00+00'),
    ('inventory-nova-pack-30-central', 'tenant-nova', 'var-nova-pack-30', 'loc-nova-central',
     35, 0, 8, TIMESTAMPTZ '2026-01-25 09:05:00+00'),
    ('inventory-nova-lamp-desk-central', 'tenant-nova', 'var-nova-lamp-desk', 'loc-nova-central',
     70, 3, 15, TIMESTAMPTZ '2026-01-25 09:06:00+00'),
    ('inventory-nova-lamp-floor-central', 'tenant-nova', 'var-nova-lamp-floor', 'loc-nova-central',
     20, 0, 5, TIMESTAMPTZ '2026-01-25 09:07:00+00')
ON CONFLICT (id) DO UPDATE SET
    tenant_id = EXCLUDED.tenant_id,
    variant_id = EXCLUDED.variant_id,
    location_id = EXCLUDED.location_id,
    on_hand_quantity = EXCLUDED.on_hand_quantity,
    reserved_quantity = EXCLUDED.reserved_quantity,
    reorder_point = EXCLUDED.reorder_point,
    updated_at = EXCLUDED.updated_at;

INSERT INTO carts
    (id, tenant_id, customer_id, session_id, status, currency, created_at, updated_at, expires_at)
VALUES
    ('cart-acme-ava', 'tenant-acme', 'customer-acme-ava', 'session-acme-ava',
     'checked_out', 'USD', TIMESTAMPTZ '2026-01-26 08:00:00+00',
     TIMESTAMPTZ '2026-01-26 08:15:00+00', NULL),
    ('cart-acme-liam', 'tenant-acme', 'customer-acme-liam', 'session-acme-liam',
     'active', 'USD', TIMESTAMPTZ '2026-01-27 08:00:00+00',
     TIMESTAMPTZ '2026-01-27 08:05:00+00', TIMESTAMPTZ '2026-02-03 08:00:00+00'),
    ('cart-nova-mia', 'tenant-nova', 'customer-nova-mia', 'session-nova-mia',
     'checked_out', 'USD', TIMESTAMPTZ '2026-01-28 08:00:00+00',
     TIMESTAMPTZ '2026-01-28 08:20:00+00', NULL)
ON CONFLICT (id) DO UPDATE SET
    tenant_id = EXCLUDED.tenant_id,
    customer_id = EXCLUDED.customer_id,
    session_id = EXCLUDED.session_id,
    status = EXCLUDED.status,
    currency = EXCLUDED.currency,
    created_at = EXCLUDED.created_at,
    updated_at = EXCLUDED.updated_at,
    expires_at = EXCLUDED.expires_at;

INSERT INTO cart_items
    (id, tenant_id, cart_id, variant_id, quantity, unit_price_minor, discount_minor, added_at)
VALUES
    ('cart-item-acme-ava-widget', 'tenant-acme', 'cart-acme-ava', 'var-acme-widget-1',
     2, 1999, 200, TIMESTAMPTZ '2026-01-26 08:01:00+00'),
    ('cart-item-acme-ava-gadget', 'tenant-acme', 'cart-acme-ava', 'var-acme-gadget-1',
     1, 4999, 300, TIMESTAMPTZ '2026-01-26 08:02:00+00'),
    ('cart-item-acme-liam-widget', 'tenant-acme', 'cart-acme-liam', 'var-acme-widget-2',
     1, 2499, 0, TIMESTAMPTZ '2026-01-27 08:01:00+00'),
    ('cart-item-nova-mia-pack', 'tenant-nova', 'cart-nova-mia', 'var-nova-pack-20',
     1, 7999, 1000, TIMESTAMPTZ '2026-01-28 08:01:00+00'),
    ('cart-item-nova-mia-lamp', 'tenant-nova', 'cart-nova-mia', 'var-nova-lamp-desk',
     1, 4999, 0, TIMESTAMPTZ '2026-01-28 08:02:00+00')
ON CONFLICT (id) DO UPDATE SET
    tenant_id = EXCLUDED.tenant_id,
    cart_id = EXCLUDED.cart_id,
    variant_id = EXCLUDED.variant_id,
    quantity = EXCLUDED.quantity,
    unit_price_minor = EXCLUDED.unit_price_minor,
    discount_minor = EXCLUDED.discount_minor,
    added_at = EXCLUDED.added_at;

INSERT INTO orders
    (id, tenant_id, customer_id, cart_id, order_number, status, currency,
     subtotal_minor, discount_minor, tax_minor, shipping_minor, total_minor,
     shipping_address, billing_address, placed_at, created_at, updated_at)
VALUES
    ('order-acme-1001', 'tenant-acme', 'customer-acme-ava', 'cart-acme-ava',
     'ACME-1001', 'delivered', 'USD', 8997, 500, 680, 0, 9177,
     '{"name":"Ava Chen","line1":"100 Market Street","city":"San Francisco","region":"CA","postal_code":"94105","country":"US"}'::JSONB,
     '{"name":"Ava Chen","line1":"100 Market Street","city":"San Francisco","region":"CA","postal_code":"94105","country":"US"}'::JSONB,
     TIMESTAMPTZ '2026-01-26 08:20:00+00', TIMESTAMPTZ '2026-01-26 08:20:00+00',
     TIMESTAMPTZ '2026-01-30 16:00:00+00'),
    ('order-nova-2001', 'tenant-nova', 'customer-nova-mia', 'cart-nova-mia',
     'NOVA-2001', 'processing', 'USD', 12998, 1000, 960, 1200, 14158,
     '{"name":"Mia Rivera","line1":"22 Pine Road","city":"Boulder","region":"CO","postal_code":"80302","country":"US"}'::JSONB,
     '{"name":"Mia Rivera","line1":"22 Pine Road","city":"Boulder","region":"CO","postal_code":"80302","country":"US"}'::JSONB,
     TIMESTAMPTZ '2026-01-28 08:25:00+00', TIMESTAMPTZ '2026-01-28 08:25:00+00',
     TIMESTAMPTZ '2026-01-28 08:30:00+00')
ON CONFLICT (id) DO UPDATE SET
    tenant_id = EXCLUDED.tenant_id,
    customer_id = EXCLUDED.customer_id,
    cart_id = EXCLUDED.cart_id,
    order_number = EXCLUDED.order_number,
    status = EXCLUDED.status,
    currency = EXCLUDED.currency,
    subtotal_minor = EXCLUDED.subtotal_minor,
    discount_minor = EXCLUDED.discount_minor,
    tax_minor = EXCLUDED.tax_minor,
    shipping_minor = EXCLUDED.shipping_minor,
    total_minor = EXCLUDED.total_minor,
    shipping_address = EXCLUDED.shipping_address,
    billing_address = EXCLUDED.billing_address,
    placed_at = EXCLUDED.placed_at,
    created_at = EXCLUDED.created_at,
    updated_at = EXCLUDED.updated_at;

INSERT INTO order_items
    (id, tenant_id, order_id, product_id, variant_id, sku, product_name,
     quantity, unit_price_minor, discount_minor, created_at)
VALUES
    ('order-item-acme-1001-widget', 'tenant-acme', 'order-acme-1001',
     'prod-acme-widget', 'var-acme-widget-1', 'WIDGET-1', 'Everyday Widget / Standard',
     2, 1999, 200, TIMESTAMPTZ '2026-01-26 08:20:01+00'),
    ('order-item-acme-1001-gadget', 'tenant-acme', 'order-acme-1001',
     'prod-acme-gadget', 'var-acme-gadget-1', 'GADGET-1', 'Smart Gadget / White',
     1, 4999, 300, TIMESTAMPTZ '2026-01-26 08:20:02+00'),
    ('order-item-nova-2001-pack', 'tenant-nova', 'order-nova-2001',
     'prod-nova-backpack', 'var-nova-pack-20', 'NOVA-PACK-20', 'Trail Pack / 20L',
     1, 7999, 1000, TIMESTAMPTZ '2026-01-28 08:25:01+00'),
    ('order-item-nova-2001-lamp', 'tenant-nova', 'order-nova-2001',
     'prod-nova-lamp', 'var-nova-lamp-desk', 'NOVA-LAMP-DESK', 'Camp Lamp / Desk',
     1, 4999, 0, TIMESTAMPTZ '2026-01-28 08:25:02+00')
ON CONFLICT (id) DO UPDATE SET
    tenant_id = EXCLUDED.tenant_id,
    order_id = EXCLUDED.order_id,
    product_id = EXCLUDED.product_id,
    variant_id = EXCLUDED.variant_id,
    sku = EXCLUDED.sku,
    product_name = EXCLUDED.product_name,
    quantity = EXCLUDED.quantity,
    unit_price_minor = EXCLUDED.unit_price_minor,
    discount_minor = EXCLUDED.discount_minor,
    created_at = EXCLUDED.created_at;

INSERT INTO payments
    (id, tenant_id, order_id, provider, provider_reference, status, amount_minor,
     currency, method_type, captured_amount_minor, refunded_amount_minor, failure_code,
     authorized_at, captured_at, created_at)
VALUES
    ('payment-acme-1001', 'tenant-acme', 'order-acme-1001', 'stripe', 'pi_acme_1001',
     'captured', 9177, 'USD', 'card', 9177, 0, NULL,
     TIMESTAMPTZ '2026-01-26 08:21:00+00',
     TIMESTAMPTZ '2026-01-26 08:21:02+00', TIMESTAMPTZ '2026-01-26 08:20:30+00'),
    ('payment-nova-2001', 'tenant-nova', 'order-nova-2001', 'stripe', 'pi_nova_2001',
     'authorized', 14158, 'USD', 'card', 0, 0, NULL,
     TIMESTAMPTZ '2026-01-28 08:26:00+00',
     NULL, TIMESTAMPTZ '2026-01-28 08:25:30+00')
ON CONFLICT (id) DO UPDATE SET
    tenant_id = EXCLUDED.tenant_id,
    order_id = EXCLUDED.order_id,
    provider = EXCLUDED.provider,
    provider_reference = EXCLUDED.provider_reference,
    status = EXCLUDED.status,
    amount_minor = EXCLUDED.amount_minor,
    currency = EXCLUDED.currency,
    method_type = EXCLUDED.method_type,
    captured_amount_minor = EXCLUDED.captured_amount_minor,
    refunded_amount_minor = EXCLUDED.refunded_amount_minor,
    failure_code = EXCLUDED.failure_code,
    authorized_at = EXCLUDED.authorized_at,
    captured_at = EXCLUDED.captured_at,
    created_at = EXCLUDED.created_at;

INSERT INTO shipments
    (id, tenant_id, order_id, shipment_number, location_id, carrier, service_level,
     tracking_number, status, shipping_address, shipped_at, delivered_at, created_at, updated_at)
VALUES
    ('shipment-acme-1001-1', 'tenant-acme', 'order-acme-1001', 1, 'loc-acme-east',
     'parcel-post', 'ground', 'ACME-TRACK-1001', 'delivered',
     '{"name":"Ava Chen","line1":"100 Market Street","city":"San Francisco","region":"CA","postal_code":"94105","country":"US"}'::JSONB,
     TIMESTAMPTZ '2026-01-27 09:00:00+00', TIMESTAMPTZ '2026-01-30 15:45:00+00',
     TIMESTAMPTZ '2026-01-26 08:22:00+00', TIMESTAMPTZ '2026-01-30 15:45:00+00'),
    ('shipment-nova-2001-1', 'tenant-nova', 'order-nova-2001', 1, 'loc-nova-central',
     'parcel-post', 'two_day', 'NOVA-TRACK-2001', 'label_created',
     '{"name":"Mia Rivera","line1":"22 Pine Road","city":"Boulder","region":"CO","postal_code":"80302","country":"US"}'::JSONB,
     NULL, NULL, TIMESTAMPTZ '2026-01-28 08:31:00+00', TIMESTAMPTZ '2026-01-28 08:31:00+00')
ON CONFLICT (id) DO UPDATE SET
    tenant_id = EXCLUDED.tenant_id,
    order_id = EXCLUDED.order_id,
    shipment_number = EXCLUDED.shipment_number,
    location_id = EXCLUDED.location_id,
    carrier = EXCLUDED.carrier,
    service_level = EXCLUDED.service_level,
    tracking_number = EXCLUDED.tracking_number,
    status = EXCLUDED.status,
    shipping_address = EXCLUDED.shipping_address,
    shipped_at = EXCLUDED.shipped_at,
    delivered_at = EXCLUDED.delivered_at,
    created_at = EXCLUDED.created_at,
    updated_at = EXCLUDED.updated_at;

INSERT INTO shipment_items (id, tenant_id, shipment_id, order_item_id, quantity, created_at)
VALUES
    ('shipment-item-acme-1001-widget', 'tenant-acme', 'shipment-acme-1001-1',
     'order-item-acme-1001-widget', 2, TIMESTAMPTZ '2026-01-27 09:00:01+00'),
    ('shipment-item-acme-1001-gadget', 'tenant-acme', 'shipment-acme-1001-1',
     'order-item-acme-1001-gadget', 1, TIMESTAMPTZ '2026-01-27 09:00:02+00'),
    ('shipment-item-nova-2001-pack', 'tenant-nova', 'shipment-nova-2001-1',
     'order-item-nova-2001-pack', 1, TIMESTAMPTZ '2026-01-28 08:31:01+00'),
    ('shipment-item-nova-2001-lamp', 'tenant-nova', 'shipment-nova-2001-1',
     'order-item-nova-2001-lamp', 1, TIMESTAMPTZ '2026-01-28 08:31:02+00')
ON CONFLICT (tenant_id, shipment_id, order_item_id) DO UPDATE SET
    id = EXCLUDED.id,
    quantity = EXCLUDED.quantity,
    created_at = EXCLUDED.created_at;

INSERT INTO outbox_events
    (id, tenant_id, event_key, aggregate_type, aggregate_id, event_type, schema_version,
     payload, traceparent, tracestate, baggage, status, attempts, available_at,
     published_at, created_at)
VALUES
    ('outbox-acme-order-created', 'tenant-acme', 'order-acme-1001-created',
     'order', 'order-acme-1001', 'order.created', 1,
     '{"order_number":"ACME-1001","total_minor":9177}'::JSONB,
     '00-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-aaaaaaaaaaaaaaaa-01', 'playground=commerce',
     'tenant.id=tenant-acme,user.tier=pro',
     'published', 1, TIMESTAMPTZ '2026-01-26 08:20:10+00',
     TIMESTAMPTZ '2026-01-26 08:20:11+00', TIMESTAMPTZ '2026-01-26 08:20:10+00'),
    ('outbox-acme-payment-captured', 'tenant-acme', 'payment-acme-1001-captured',
     'payment', 'payment-acme-1001', 'payment.captured', 1,
     '{"order_id":"order-acme-1001","payment_id":"payment-acme-1001","amount_minor":9177,"currency":"USD","payment_status":"captured"}'::JSONB,
     '00-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-bbbbbbbbbbbbbbbb-01', 'playground=commerce',
     'tenant.id=tenant-acme,user.tier=pro',
     'published', 1, TIMESTAMPTZ '2026-01-26 08:21:03+00',
     TIMESTAMPTZ '2026-01-26 08:21:04+00', TIMESTAMPTZ '2026-01-26 08:21:03+00'),
    ('outbox-nova-order-created', 'tenant-nova', 'order-nova-2001-created',
     'order', 'order-nova-2001', 'order.created', 1,
     '{"order_number":"NOVA-2001","total_minor":14158}'::JSONB,
     '00-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-cccccccccccccccc-01', 'playground=commerce',
     'tenant.id=tenant-nova,user.tier=free',
     'published', 1, TIMESTAMPTZ '2026-01-28 08:25:10+00',
     TIMESTAMPTZ '2026-01-28 08:25:11+00', TIMESTAMPTZ '2026-01-28 08:25:10+00'),
    ('outbox-nova-shipment-created', 'tenant-nova', 'shipment-nova-2001-created',
     'shipment', 'shipment-nova-2001-1', 'shipment.created', 1,
     '{"order_id":"order-nova-2001","service_level":"two_day"}'::JSONB,
     '00-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-dddddddddddddddd-01', 'playground=commerce',
     'tenant.id=tenant-nova,user.tier=free',
     'queued', 0, TIMESTAMPTZ '2026-01-28 08:31:05+00',
     NULL, TIMESTAMPTZ '2026-01-28 08:31:05+00')
ON CONFLICT (id) DO UPDATE SET
    tenant_id = EXCLUDED.tenant_id,
    event_key = EXCLUDED.event_key,
    aggregate_type = EXCLUDED.aggregate_type,
    aggregate_id = EXCLUDED.aggregate_id,
    event_type = EXCLUDED.event_type,
    schema_version = EXCLUDED.schema_version,
    payload = EXCLUDED.payload,
    traceparent = EXCLUDED.traceparent,
    tracestate = EXCLUDED.tracestate,
    baggage = EXCLUDED.baggage,
    status = EXCLUDED.status,
    attempts = EXCLUDED.attempts,
    available_at = EXCLUDED.available_at,
    published_at = EXCLUDED.published_at,
    created_at = EXCLUDED.created_at;

INSERT INTO feature_exposures
    (id, tenant_id, customer_id, anonymous_id, session_id, feature_key, variant,
     exposure_key, context, exposed_at)
VALUES
    ('exposure-acme-catalog-ava', 'tenant-acme', 'customer-acme-ava', NULL,
     'session-acme-ava', 'catalog_promo', 'treatment', 'acme-ava-catalog-promo-20260126',
     '{"source":"web","device":"desktop"}'::JSONB, TIMESTAMPTZ '2026-01-26 08:01:10+00'),
    ('exposure-acme-checkout-liam', 'tenant-acme', 'customer-acme-liam', NULL,
     'session-acme-liam', 'checkout_v2', 'control', 'acme-liam-checkout-v2-20260127',
     '{"source":"web","device":"mobile"}'::JSONB, TIMESTAMPTZ '2026-01-27 08:01:10+00'),
    ('exposure-nova-checkout-mia', 'tenant-nova', 'customer-nova-mia', NULL,
     'session-nova-mia', 'checkout_v2', 'treatment', 'nova-mia-checkout-v2-20260128',
     '{"source":"storefront","device":"mobile"}'::JSONB, TIMESTAMPTZ '2026-01-28 08:01:10+00'),
    ('exposure-nova-recommendations-anon', 'tenant-nova', NULL, 'anon-nova-demo',
     'session-nova-anon', 'recommendations', 'control', 'nova-anon-recommendations-20260128',
     '{"source":"storefront","device":"desktop"}'::JSONB, TIMESTAMPTZ '2026-01-28 09:01:10+00')
ON CONFLICT (id) DO UPDATE SET
    tenant_id = EXCLUDED.tenant_id,
    customer_id = EXCLUDED.customer_id,
    anonymous_id = EXCLUDED.anonymous_id,
    session_id = EXCLUDED.session_id,
    feature_key = EXCLUDED.feature_key,
    variant = EXCLUDED.variant,
    exposure_key = EXCLUDED.exposure_key,
    context = EXCLUDED.context,
    exposed_at = EXCLUDED.exposed_at;

INSERT INTO analytics_events
    (id, tenant_id, event_key, customer_id, session_id, event_name, event_version,
     source, entity_type, entity_id, occurred_at, received_at, trace_id, span_id,
     properties, context)
VALUES
    ('event-acme-widget-viewed', 'tenant-acme', 'acme-widget-viewed-20260126-0001',
     'customer-acme-ava', 'session-acme-ava', 'product.viewed', 1, 'web',
     'product', 'prod-acme-widget', TIMESTAMPTZ '2026-01-26 08:00:40+00',
     TIMESTAMPTZ '2026-01-26 08:00:41+00',
     'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', '1111111111111111',
     '{"sku":"WIDGET-1","source":"search"}'::JSONB,
     '{"user_tier":"pro","page":"/products/everyday-widget"}'::JSONB),
    ('event-acme-cart-added', 'tenant-acme', 'acme-cart-added-20260126-0002',
     'customer-acme-ava', 'session-acme-ava', 'cart.item_added', 1, 'web',
     'cart', 'cart-acme-ava', TIMESTAMPTZ '2026-01-26 08:01:00+00',
     TIMESTAMPTZ '2026-01-26 08:01:01+00',
     'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', '2222222222222222',
     '{"sku":"WIDGET-1","quantity":2,"unit_price_minor":1999}'::JSONB,
     '{"user_tier":"pro","currency":"USD"}'::JSONB),
    ('event-acme-checkout-started', 'tenant-acme', 'acme-checkout-started-20260126-0003',
     'customer-acme-ava', 'session-acme-ava', 'checkout.started', 1, 'checkout',
     'order', 'order-acme-1001', TIMESTAMPTZ '2026-01-26 08:20:00+00',
     TIMESTAMPTZ '2026-01-26 08:20:01+00',
     'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', '3333333333333333',
     '{"item_count":3,"total_minor":9177}'::JSONB,
     '{"tenant":"acme","currency":"USD"}'::JSONB),
    ('event-acme-order-completed', 'tenant-acme', 'acme-order-completed-20260126-0004',
     'customer-acme-ava', 'session-acme-ava', 'order.completed', 1, 'payment',
     'order', 'order-acme-1001', TIMESTAMPTZ '2026-01-26 08:21:02+00',
     TIMESTAMPTZ '2026-01-26 08:21:03+00',
     'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', '4444444444444444',
     '{"order_number":"ACME-1001","total_minor":9177,"currency":"USD"}'::JSONB,
     '{"payment_provider":"stripe"}'::JSONB),
    ('event-acme-inventory-reserved', 'tenant-acme', 'acme-inventory-reserved-20260126-0005',
     NULL, NULL, 'inventory.reserved', 1, 'inventory',
     'order', 'order-acme-1001', TIMESTAMPTZ '2026-01-26 08:21:05+00',
     TIMESTAMPTZ '2026-01-26 08:21:06+00',
     'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', '5555555555555555',
     '{"sku":"WIDGET-1","quantity":2,"remaining":94}'::JSONB,
     '{"location":"ACME-EAST"}'::JSONB),
    ('event-nova-pack-viewed', 'tenant-nova', 'nova-pack-viewed-20260128-0001',
     'customer-nova-mia', 'session-nova-mia', 'product.viewed', 1, 'storefront',
     'product', 'prod-nova-backpack', TIMESTAMPTZ '2026-01-28 08:00:30+00',
     TIMESTAMPTZ '2026-01-28 08:00:31+00',
     'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb', '6666666666666666',
     '{"sku":"NOVA-PACK-20","capacity_liters":20}'::JSONB,
     '{"user_tier":"free","page":"/products/trail-pack"}'::JSONB),
    ('event-nova-feature-exposed', 'tenant-nova', 'nova-feature-exposed-20260128-0002',
     'customer-nova-mia', 'session-nova-mia', 'feature.exposed', 1, 'storefront',
     'feature', 'checkout_v2', TIMESTAMPTZ '2026-01-28 08:01:10+00',
     TIMESTAMPTZ '2026-01-28 08:01:11+00',
     'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb', '7777777777777777',
     '{"feature_key":"checkout_v2","variant":"treatment"}'::JSONB,
     '{"exposure_key":"nova-mia-checkout-v2-20260128"}'::JSONB),
    ('event-nova-order-created', 'tenant-nova', 'nova-order-created-20260128-0003',
     'customer-nova-mia', 'session-nova-mia', 'order.created', 1, 'checkout',
     'order', 'order-nova-2001', TIMESTAMPTZ '2026-01-28 08:25:00+00',
     TIMESTAMPTZ '2026-01-28 08:25:01+00',
     'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb', '8888888888888888',
     '{"order_number":"NOVA-2001","total_minor":14158,"currency":"USD"}'::JSONB,
     '{"promotion_code":"NOVA15"}'::JSONB),
    ('event-nova-shipment-created', 'tenant-nova', 'nova-shipment-created-20260128-0004',
     NULL, NULL, 'shipment.created', 1, 'fulfillment',
     'shipment', 'shipment-nova-2001-1', TIMESTAMPTZ '2026-01-28 08:31:00+00',
     TIMESTAMPTZ '2026-01-28 08:31:01+00',
     'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb', '9999999999999999',
     '{"carrier":"parcel-post","service_level":"two_day"}'::JSONB,
     '{"location":"NOVA-CENTRAL"}'::JSONB)
ON CONFLICT (id) DO UPDATE SET
    tenant_id = EXCLUDED.tenant_id,
    event_key = EXCLUDED.event_key,
    customer_id = EXCLUDED.customer_id,
    session_id = EXCLUDED.session_id,
    event_name = EXCLUDED.event_name,
    event_version = EXCLUDED.event_version,
    source = EXCLUDED.source,
    entity_type = EXCLUDED.entity_type,
    entity_id = EXCLUDED.entity_id,
    occurred_at = EXCLUDED.occurred_at,
    received_at = EXCLUDED.received_at,
    trace_id = EXCLUDED.trace_id,
    span_id = EXCLUDED.span_id,
    properties = EXCLUDED.properties,
    context = EXCLUDED.context;
