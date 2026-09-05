-- Deterministic assortment used by catalog pagination, filtering, joins, and
-- nested GraphQL batching.  Keep the generated identifiers stable so clean
-- verifier runs and local demonstrations observe the same records.

SET TIME ZONE 'UTC';
SET search_path TO public;

INSERT INTO products
    (id, tenant_id, category_id, slug, name, description, brand, status,
     attributes, created_at, updated_at)
SELECT
    format('prod-acme-assortment-%s', lpad(n::text, 2, '0')),
    'tenant-acme',
    CASE WHEN n % 2 = 0 THEN 'cat-acme-electronics' ELSE 'cat-acme-kitchen' END,
    format('acme-assortment-%s', lpad(n::text, 2, '0')),
    format('Acme Assortment Product %s', n),
    format('A deterministic Acme catalog item number %s for browse and search.', n),
    'Acme',
    'active',
    jsonb_build_object('assortment_index', n, 'featured', n <= 4),
    TIMESTAMPTZ '2026-02-01 00:00:00+00' + (n * INTERVAL '1 minute'),
    TIMESTAMPTZ '2026-02-01 00:00:00+00' + (n * INTERVAL '1 minute')
FROM generate_series(1, 24) AS series(n)
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
    (id, tenant_id, product_id, sku, name, option_values, status,
     weight_grams, created_at)
SELECT
    format('var-acme-assortment-%s', lpad(n::text, 2, '0')),
    'tenant-acme',
    format('prod-acme-assortment-%s', lpad(n::text, 2, '0')),
    format('ACME-DEMO-%s', lpad(n::text, 2, '0')),
    format('Acme Assortment Product %s / Standard', n),
    jsonb_build_object('finish', CASE WHEN n % 2 = 0 THEN 'graphite' ELSE 'silver' END,
                       'assortment_index', n),
    'active',
    100 + n,
    TIMESTAMPTZ '2026-02-01 01:00:00+00' + (n * INTERVAL '1 minute')
FROM generate_series(1, 24) AS series(n)
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
SELECT
    format('price-acme-assortment-%s-usd', lpad(n::text, 2, '0')),
    'tenant-acme',
    format('var-acme-assortment-%s', lpad(n::text, 2, '0')),
    'USD',
    1000 + (n * 125),
    1200 + (n * 125),
    TIMESTAMPTZ '2026-02-01 01:00:00+00' + (n * INTERVAL '1 minute'),
    NULL,
    TRUE,
    TIMESTAMPTZ '2026-02-01 01:00:00+00' + (n * INTERVAL '1 minute')
FROM generate_series(1, 24) AS series(n)
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

INSERT INTO inventory
    (id, tenant_id, variant_id, location_id, on_hand_quantity,
     reserved_quantity, reorder_point, updated_at)
SELECT
    format('inventory-acme-assortment-%s', lpad(n::text, 2, '0')),
    'tenant-acme',
    format('var-acme-assortment-%s', lpad(n::text, 2, '0')),
    CASE WHEN n % 2 = 0 THEN 'loc-acme-west' ELSE 'loc-acme-east' END,
    50 + n,
    0,
    10,
    TIMESTAMPTZ '2026-02-01 02:00:00+00' + (n * INTERVAL '1 minute')
FROM generate_series(1, 24) AS series(n)
ON CONFLICT (id) DO UPDATE SET
    tenant_id = EXCLUDED.tenant_id,
    variant_id = EXCLUDED.variant_id,
    location_id = EXCLUDED.location_id,
    on_hand_quantity = EXCLUDED.on_hand_quantity,
    reserved_quantity = EXCLUDED.reserved_quantity,
    reorder_point = EXCLUDED.reorder_point,
    updated_at = EXCLUDED.updated_at;

INSERT INTO reviews
    (id, tenant_id, product_id, customer_id, rating, title, body, status,
     verified_purchase, created_at, updated_at)
SELECT
    format('review-acme-assortment-%s', lpad(n::text, 2, '0')),
    'tenant-acme',
    format('prod-acme-assortment-%s', lpad(n::text, 2, '0')),
    CASE WHEN n % 2 = 0 THEN 'customer-acme-liam' ELSE 'customer-acme-ava' END,
    CASE WHEN n % 5 = 0 THEN 4 ELSE 5 END,
    format('Reliable item %s', n),
    format('Deterministic review for Acme assortment item %s.', n),
    'published',
    TRUE,
    TIMESTAMPTZ '2026-02-02 00:00:00+00' + (n * INTERVAL '1 minute'),
    TIMESTAMPTZ '2026-02-02 00:00:00+00' + (n * INTERVAL '1 minute')
FROM generate_series(1, 24) AS series(n)
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
