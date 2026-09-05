#!/bin/sh
set -eu

schema_statement_timeout_ms="${SCHEMA_STATEMENT_TIMEOUT_MS:-${MIGRATION_STATEMENT_TIMEOUT_MS:-10000}}"
schema_lock_timeout_ms="${SCHEMA_LOCK_TIMEOUT_MS:-${MIGRATION_LOCK_TIMEOUT_MS:-5000}}"
pg_connect_timeout="${PGCONNECT_TIMEOUT:-5}"

case "$schema_statement_timeout_ms" in
  ''|*[!0-9]*|0)
    echo "SCHEMA_STATEMENT_TIMEOUT_MS must be a positive integer: $schema_statement_timeout_ms" >&2
    exit 1
    ;;
esac
case "$schema_lock_timeout_ms" in
  ''|*[!0-9]*|0)
    echo "SCHEMA_LOCK_TIMEOUT_MS must be a positive integer: $schema_lock_timeout_ms" >&2
    exit 1
    ;;
esac
case "$pg_connect_timeout" in
  ''|*[!0-9]*|0)
    echo "PGCONNECT_TIMEOUT must be a positive integer: $pg_connect_timeout" >&2
    exit 1
    ;;
esac

# Direct invocations must receive the same bounded verification session as the
# migration runner. Appending makes the verifier's limits win over inherited
# values without discarding unrelated PGOPTIONS such as application_name.
export PGOPTIONS="${PGOPTIONS:+$PGOPTIONS }-c statement_timeout=${schema_statement_timeout_ms}ms -c lock_timeout=${schema_lock_timeout_ms}ms"
export PGCONNECT_TIMEOUT="$pg_connect_timeout"

expected_tables='tenants categories products product_variants prices promotions promotion_products customers reviews inventory_locations inventory carts cart_items orders checkout_attempts order_items payments shipments payment_operation_requests shipment_items outbox_events feature_exposures analytics_events notification_deliveries fulfillment_processed_events price_change_events inventory_reservations checkout_compensation_tasks checkout_payment_reconciliations payment_provider_authorization_decisions fulfillment_effects notification_delivery_attempts notification_channel_messages orders_consumer_inbox'

missing_tables=$(psql -v ON_ERROR_STOP=1 -Atqc "
  WITH expected(table_name) AS (
    VALUES
      ('tenants'), ('categories'), ('products'), ('product_variants'),
      ('prices'), ('promotions'), ('promotion_products'), ('customers'),
      ('reviews'), ('inventory_locations'), ('inventory'), ('carts'),
      ('cart_items'), ('orders'), ('checkout_attempts'), ('order_items'),
      ('payments'), ('payment_provider_authorization_decisions'),
      ('shipments'), ('payment_operation_requests'),
      ('shipment_items'), ('outbox_events'), ('feature_exposures'),
      ('analytics_events'), ('notification_deliveries'),
      ('fulfillment_processed_events'), ('price_change_events'),
      ('inventory_reservations'), ('checkout_compensation_tasks'),
      ('checkout_payment_reconciliations'),
      ('fulfillment_effects'),
      ('notification_delivery_attempts'), ('notification_channel_messages'),
      ('orders_consumer_inbox')
  )
  SELECT COALESCE(string_agg(expected.table_name, ', ' ORDER BY expected.table_name), '')
  FROM expected
  LEFT JOIN information_schema.tables actual
    ON actual.table_schema = 'public' AND actual.table_name = expected.table_name
  WHERE actual.table_name IS NULL;
")

if [ -n "$missing_tables" ]; then
  echo "PostgreSQL schema is incomplete: required table(s) missing: $missing_tables" >&2
  exit 1
fi

missing_columns=$(psql -v ON_ERROR_STOP=1 -Atqc "
  WITH expected(table_name, column_name) AS (
    VALUES
      ('tenants', 'id'), ('tenants', 'default_currency'),
      ('products', 'tenant_id'), ('product_variants', 'sku'),
      ('prices', 'amount_minor'), ('prices', 'valid_from'),
      ('customers', 'tenant_id'), ('inventory', 'available_quantity'),
      ('carts', 'customer_id'), ('cart_items', 'unit_price_minor'),
      ('orders', 'tenant_id'), ('orders', 'total_minor'),
      ('orders', 'checkout_request_id'),
      ('checkout_attempts', 'request_fingerprint'),
      ('checkout_attempts', 'lease_token'),
      ('checkout_attempts', 'response_payload'),
      ('order_items', 'line_total_minor'),
      ('payments', 'status'), ('payments', 'captured_amount_minor'),
      ('payment_operation_requests', 'request_fingerprint'),
      ('shipments', 'status'), ('shipment_items', 'quantity'),
      ('outbox_events', 'available_at'), ('outbox_events', 'traceparent'),
      ('outbox_events', 'occurred_at'),
      ('outbox_events', 'failure_code'), ('outbox_events', 'failure_message'),
      ('outbox_events', 'failed_at'),
      ('outbox_events', 'claim_token'), ('outbox_events', 'claim_expires_at'),
      ('feature_exposures', 'variant'), ('analytics_events', 'trace_id'),
      ('notification_deliveries', 'event_key'),
      ('notification_deliveries', 'next_attempt_at'),
      ('notification_deliveries', 'lease_until'),
      ('notification_deliveries', 'lease_token'),
      ('notification_deliveries', 'last_error'),
      ('notification_deliveries', 'dead_lettered_at'),
      ('notification_deliveries', 'acknowledged_at'),
      ('notification_deliveries', 'updated_at'),
      ('notification_deliveries', 'traceparent'),
      ('notification_deliveries', 'tracestate'),
      ('notification_deliveries', 'baggage'),
      ('fulfillment_processed_events', 'claimed_at'),
      ('fulfillment_processed_events', 'lease_token'),
      ('fulfillment_processed_events', 'lease_until'),
      ('fulfillment_processed_events', 'dead_lettered_at'),
      ('price_change_events', 'sequence'),
      ('inventory_reservations', 'reservation_id'),
      ('inventory_reservations', 'status'),
      ('inventory_reservations', 'expires_at'),
      ('inventory_reservations', 'consumed_at'),
      ('inventory_reservations', 'owner_request_id'),
      ('inventory_reservations', 'owner_lease_token'),
      ('checkout_compensation_tasks', 'task_key'),
      ('checkout_compensation_tasks', 'status'),
      ('checkout_compensation_tasks', 'checkout_request_id'),
      ('checkout_compensation_tasks', 'checkout_lease_token'),
      ('checkout_compensation_tasks', 'remote_operation_id'),
      ('checkout_compensation_tasks', 'remote_operation_started_at'),
      ('checkout_compensation_tasks', 'remote_operation_completed_at'),
      ('checkout_compensation_tasks', 'traceparent'),
      ('checkout_compensation_tasks', 'tracestate'),
      ('checkout_compensation_tasks', 'baggage'),
      ('checkout_payment_reconciliations', 'tenant_id'),
      ('checkout_payment_reconciliations', 'request_id'),
      ('checkout_payment_reconciliations', 'order_id'),
      ('checkout_payment_reconciliations', 'authorize_request_id'),
      ('checkout_payment_reconciliations', 'payment_id'),
      ('checkout_payment_reconciliations', 'merchant_reference'),
      ('checkout_payment_reconciliations', 'amount_minor'),
      ('checkout_payment_reconciliations', 'currency'),
      ('checkout_payment_reconciliations', 'method_type'),
      ('checkout_payment_reconciliations', 'feature_variant'),
      ('checkout_payment_reconciliations', 'status'),
      ('checkout_payment_reconciliations', 'attempts'),
      ('checkout_payment_reconciliations', 'available_at'),
      ('checkout_payment_reconciliations', 'claimed_at'),
      ('checkout_payment_reconciliations', 'lease_token'),
      ('checkout_payment_reconciliations', 'lease_expires_at'),
      ('checkout_payment_reconciliations', 'checkout_lease_token'),
      ('checkout_payment_reconciliations', 'completed_at'),
      ('checkout_payment_reconciliations', 'last_payment_status'),
      ('checkout_payment_reconciliations', 'last_operation_status'),
      ('checkout_payment_reconciliations', 'last_failure_reason'),
      ('checkout_payment_reconciliations', 'last_error'),
      ('checkout_payment_reconciliations', 'traceparent'),
      ('checkout_payment_reconciliations', 'tracestate'),
      ('checkout_payment_reconciliations', 'baggage'),
      ('checkout_payment_reconciliations', 'created_at'),
      ('checkout_payment_reconciliations', 'updated_at'),
      ('fulfillment_effects', 'tenant_id'),
      ('fulfillment_effects', 'order_id'),
      ('fulfillment_effects', 'effect_key'),
      ('fulfillment_effects', 'operation'),
      ('fulfillment_effects', 'effect_kind'),
      ('fulfillment_effects', 'payload'),
      ('fulfillment_effects', 'notification_status'),
      ('fulfillment_effects', 'aggregate_version'),
      ('fulfillment_effects', 'status'),
      ('fulfillment_effects', 'attempts'),
      ('fulfillment_effects', 'claim_token'),
      ('fulfillment_effects', 'claim_until'),
      ('fulfillment_effects', 'published_at'),
      ('fulfillment_effects', 'cancelled_at'),
      ('fulfillment_effects', 'last_error'),
      ('fulfillment_effects', 'created_at'),
      ('notification_delivery_attempts', 'id'),
      ('notification_delivery_attempts', 'delivery_id'),
      ('notification_delivery_attempts', 'tenant_id'),
      ('notification_delivery_attempts', 'attempt'),
      ('notification_delivery_attempts', 'outcome'),
      ('notification_delivery_attempts', 'error'),
      ('notification_delivery_attempts', 'started_at'),
      ('notification_delivery_attempts', 'completed_at'),
      ('notification_channel_messages', 'delivery_id'),
      ('notification_channel_messages', 'tenant_id'),
      ('notification_channel_messages', 'order_id'),
      ('notification_channel_messages', 'channel'),
      ('notification_channel_messages', 'payload'),
      ('notification_channel_messages', 'dispatched_at'),
      ('notification_channel_messages', 'acknowledged_at'),
      ('notification_channel_messages', 'acknowledgement_reference'),
      ('payments', 'pending_resolution'),
      ('payments', 'pending_reconciliation_attempts'),
      ('payments', 'pending_reconciliation_at'),
      ('payments', 'pending_resolution_at'),
      ('payments', 'provider_decision_verified_at'),
      ('payment_provider_authorization_decisions', 'tenant_id'),
      ('payment_provider_authorization_decisions', 'payment_id'),
      ('payment_provider_authorization_decisions', 'provider'),
      ('payment_provider_authorization_decisions', 'provider_reference'),
      ('payment_provider_authorization_decisions', 'amount_minor'),
      ('payment_provider_authorization_decisions', 'currency'),
      ('payment_provider_authorization_decisions', 'outcome'),
      ('payment_provider_authorization_decisions', 'verified_at'),
      ('payment_provider_authorization_decisions', 'created_at'),
      ('orders_consumer_inbox', 'tenant_id'),
      ('orders_consumer_inbox', 'event_id'),
      ('orders_consumer_inbox', 'status'),
      ('orders_consumer_inbox', 'attempts'),
      ('orders_consumer_inbox', 'lease_until')
  )
  SELECT COALESCE(
    string_agg(expected.table_name || '.' || expected.column_name, ', ' ORDER BY expected.table_name, expected.column_name),
    ''
  )
  FROM expected
  LEFT JOIN information_schema.columns actual
    ON actual.table_schema = 'public'
   AND actual.table_name = expected.table_name
   AND actual.column_name = expected.column_name
  WHERE actual.column_name IS NULL;
")

if [ -n "$missing_columns" ]; then
  echo "PostgreSQL schema is incomplete: required column(s) missing: $missing_columns" >&2
  exit 1
fi

missing_primary_keys=$(psql -v ON_ERROR_STOP=1 -Atqc "
  WITH expected(table_name) AS (
    VALUES
      ('tenants'), ('categories'), ('products'), ('product_variants'),
      ('prices'), ('promotions'), ('promotion_products'), ('customers'),
      ('reviews'), ('inventory_locations'), ('inventory'), ('carts'),
      ('cart_items'), ('orders'), ('checkout_attempts'), ('order_items'),
      ('payments'), ('payment_provider_authorization_decisions'),
      ('shipments'), ('payment_operation_requests'),
      ('shipment_items'), ('outbox_events'), ('feature_exposures'),
      ('analytics_events'), ('notification_deliveries'),
      ('fulfillment_processed_events'), ('price_change_events'),
      ('inventory_reservations'), ('checkout_compensation_tasks'),
      ('checkout_payment_reconciliations'),
      ('fulfillment_effects'),
      ('notification_delivery_attempts'), ('notification_channel_messages'),
      ('orders_consumer_inbox')
  )
  SELECT COALESCE(string_agg(expected.table_name, ', ' ORDER BY expected.table_name), '')
  FROM expected
  WHERE NOT EXISTS (
    SELECT 1
    FROM pg_constraint constraint_row
    JOIN pg_class table_row ON table_row.oid = constraint_row.conrelid
    JOIN pg_namespace schema_row ON schema_row.oid = table_row.relnamespace
    WHERE schema_row.nspname = 'public'
      AND table_row.relname = expected.table_name
      AND constraint_row.contype = 'p'
  );
")

if [ -n "$missing_primary_keys" ]; then
  echo "PostgreSQL schema is incomplete: missing primary key on $missing_primary_keys" >&2
  exit 1
fi

missing_payment_reconciliation_primary_key=$(psql -v ON_ERROR_STOP=1 -Atqc "
  SELECT CASE WHEN EXISTS (
    SELECT 1
    FROM pg_constraint constraint_row
    JOIN pg_class table_row ON table_row.oid = constraint_row.conrelid
    JOIN pg_namespace schema_row ON schema_row.oid = table_row.relnamespace
    WHERE schema_row.nspname = 'public'
      AND table_row.relname = 'checkout_payment_reconciliations'
      AND constraint_row.contype = 'p'
      AND pg_get_constraintdef(constraint_row.oid) =
          'PRIMARY KEY (tenant_id, request_id)'
  ) THEN '' ELSE 'checkout_payment_reconciliations (tenant_id, request_id)' END;
")

if [ -n "$missing_payment_reconciliation_primary_key" ]; then
  echo "PostgreSQL schema is incomplete: missing checkout payment reconciliation primary key: $missing_payment_reconciliation_primary_key" >&2
  exit 1
fi

missing_notification_primary_keys=$(psql -v ON_ERROR_STOP=1 -Atqc "
  WITH expected(table_name, required_definition) AS (
    VALUES
      ('notification_delivery_attempts', 'PRIMARY KEY (id)'),
      ('notification_channel_messages', 'PRIMARY KEY (delivery_id)')
  )
  SELECT COALESCE(
    string_agg(expected.table_name, ', ' ORDER BY expected.table_name),
    ''
  )
  FROM expected
  WHERE NOT EXISTS (
    SELECT 1
    FROM pg_constraint constraint_row
    JOIN pg_class table_row ON table_row.oid = constraint_row.conrelid
    JOIN pg_namespace schema_row ON schema_row.oid = table_row.relnamespace
    WHERE schema_row.nspname = 'public'
      AND table_row.relname = expected.table_name
      AND constraint_row.contype = 'p'
      AND pg_get_constraintdef(constraint_row.oid) = expected.required_definition
  );
")

if [ -n "$missing_notification_primary_keys" ]; then
  echo "PostgreSQL schema is incomplete: missing notification primary key(s): $missing_notification_primary_keys" >&2
  exit 1
fi

missing_payment_provider_decision_primary_key=$(psql -v ON_ERROR_STOP=1 -Atqc "
  WITH actual AS (
    SELECT pg_get_constraintdef(constraint_row.oid) AS definition
    FROM pg_constraint constraint_row
    JOIN pg_class table_row ON table_row.oid = constraint_row.conrelid
    JOIN pg_namespace schema_row ON schema_row.oid = table_row.relnamespace
    WHERE schema_row.nspname = 'public'
      AND table_row.relname = 'payment_provider_authorization_decisions'
      AND constraint_row.contype = 'p'
  )
  SELECT CASE
    WHEN (SELECT count(*) FROM actual) <> 1
      THEN 'payment_provider_authorization_decisions expected exactly 1 PRIMARY KEY, got '
        || (SELECT count(*) FROM actual)::text
    WHEN NOT EXISTS (
      SELECT 1
      FROM actual
      WHERE definition = 'PRIMARY KEY (tenant_id, payment_id)'
    ) THEN 'payment_provider_authorization_decisions PRIMARY KEY (tenant_id, payment_id) is missing'
    ELSE ''
  END;
"
)

if [ -n "$missing_payment_provider_decision_primary_key" ]; then
  echo "PostgreSQL schema is incomplete: migration 012 payment provider decision primary key contract missing or mismatched: $missing_payment_provider_decision_primary_key" >&2
  exit 1
fi

missing_column_contracts=$(psql -v ON_ERROR_STOP=1 -Atqc "
  WITH expected(table_name, column_name) AS (
    VALUES
      ('checkout_attempts', 'lease_token'),
      ('inventory_reservations', 'expires_at'),
      ('payments', 'pending_reconciliation_attempts'),
      ('checkout_payment_reconciliations', 'checkout_lease_token'),
      ('payment_provider_authorization_decisions', 'tenant_id'),
      ('payment_provider_authorization_decisions', 'payment_id'),
      ('payment_provider_authorization_decisions', 'provider'),
      ('payment_provider_authorization_decisions', 'provider_reference'),
      ('payment_provider_authorization_decisions', 'amount_minor'),
      ('payment_provider_authorization_decisions', 'currency'),
      ('payment_provider_authorization_decisions', 'outcome'),
      ('payment_provider_authorization_decisions', 'verified_at'),
      ('payment_provider_authorization_decisions', 'created_at'),
      ('notification_deliveries', 'next_attempt_at'),
      ('notification_deliveries', 'updated_at'),
      ('outbox_events', 'traceparent'),
      ('outbox_events', 'tracestate'),
      ('outbox_events', 'baggage'),
      ('notification_delivery_attempts', 'delivery_id'),
      ('notification_delivery_attempts', 'tenant_id'),
      ('notification_delivery_attempts', 'attempt'),
      ('notification_delivery_attempts', 'outcome'),
      ('notification_delivery_attempts', 'started_at'),
      ('notification_channel_messages', 'delivery_id'),
      ('notification_channel_messages', 'tenant_id'),
      ('notification_channel_messages', 'order_id'),
      ('notification_channel_messages', 'channel'),
      ('notification_channel_messages', 'payload'),
      ('notification_channel_messages', 'dispatched_at'),
      ('notification_channel_messages', 'acknowledged_at'),
      ('notification_channel_messages', 'acknowledgement_reference'),
      ('fulfillment_effects', 'tenant_id'),
      ('fulfillment_effects', 'order_id'),
      ('fulfillment_effects', 'effect_key'),
      ('fulfillment_effects', 'operation'),
      ('fulfillment_effects', 'effect_kind'),
      ('fulfillment_effects', 'payload'),
      ('fulfillment_effects', 'aggregate_version'),
      ('fulfillment_effects', 'status'),
      ('fulfillment_effects', 'attempts'),
      ('fulfillment_effects', 'created_at')
  )
  SELECT COALESCE(
    string_agg(expected.table_name || '.' || expected.column_name, ', ' ORDER BY expected.table_name, expected.column_name),
    ''
  )
  FROM expected
  LEFT JOIN information_schema.columns actual
    ON actual.table_schema = 'public'
   AND actual.table_name = expected.table_name
   AND actual.column_name = expected.column_name
   AND actual.is_nullable = 'NO'
  WHERE actual.column_name IS NULL;
")

if [ -n "$missing_column_contracts" ]; then
  echo "PostgreSQL schema is incomplete: required non-null column contract(s) missing: $missing_column_contracts" >&2
  exit 1
fi

missing_payment_provider_decision_column_contracts=$(psql -v ON_ERROR_STOP=1 -Atqc "
  WITH expected(table_name, column_name, data_type, required_nullability) AS (
    VALUES
      ('payments', 'provider_decision_verified_at', 'timestamp with time zone', 'YES'),
      ('payment_provider_authorization_decisions', 'tenant_id', 'text', 'NO'),
      ('payment_provider_authorization_decisions', 'payment_id', 'text', 'NO'),
      ('payment_provider_authorization_decisions', 'provider', 'text', 'NO'),
      ('payment_provider_authorization_decisions', 'provider_reference', 'text', 'NO'),
      ('payment_provider_authorization_decisions', 'amount_minor', 'integer', 'NO'),
      ('payment_provider_authorization_decisions', 'currency', 'text', 'NO'),
      ('payment_provider_authorization_decisions', 'outcome', 'text', 'NO'),
      ('payment_provider_authorization_decisions', 'verified_at', 'timestamp with time zone', 'NO'),
      ('payment_provider_authorization_decisions', 'created_at', 'timestamp with time zone', 'NO')
  )
  SELECT COALESCE(
    string_agg(
      expected.table_name || '.' || expected.column_name
        || ' (expected ' || expected.data_type || ', nullable=' || expected.required_nullability || ')',
      ', ' ORDER BY expected.table_name, expected.column_name
    ),
    ''
  )
  FROM expected
  LEFT JOIN information_schema.columns actual
    ON actual.table_schema = 'public'
   AND actual.table_name = expected.table_name
   AND actual.column_name = expected.column_name
  WHERE actual.column_name IS NULL
     OR actual.data_type <> expected.data_type
     OR actual.is_nullable <> expected.required_nullability;
")

if [ -n "$missing_payment_provider_decision_column_contracts" ]; then
  echo "PostgreSQL schema is incomplete: migration 012 payment provider decision column contract(s) missing or mismatched: $missing_payment_provider_decision_column_contracts" >&2
  exit 1
fi

missing_payment_provider_decision_table_shape=$(psql -v ON_ERROR_STOP=1 -Atqc "
  WITH expected(label, actual_count, expected_count) AS (
    VALUES
      ('payment_provider_authorization_decisions base table',
       (SELECT count(*)
        FROM pg_class table_row
        JOIN pg_namespace schema_row ON schema_row.oid = table_row.relnamespace
        WHERE schema_row.nspname = 'public'
          AND table_row.relname = 'payment_provider_authorization_decisions'
          AND table_row.relkind = 'r'), 1),
      ('payment_provider_authorization_decisions columns',
       (SELECT count(*)
        FROM information_schema.columns
        WHERE table_schema = 'public'
          AND table_name = 'payment_provider_authorization_decisions'), 9),
      ('payment_provider_authorization_decisions non-null columns',
       (SELECT count(*)
        FROM information_schema.columns
        WHERE table_schema = 'public'
          AND table_name = 'payment_provider_authorization_decisions'
          AND is_nullable = 'NO'), 9)
  )
  SELECT COALESCE(
    string_agg(
      label || ' expected ' || expected_count::text || ', got ' || actual_count::text,
      '; ' ORDER BY label
    ),
    ''
  )
  FROM expected
  WHERE actual_count <> expected_count;
"
)

if [ -n "$missing_payment_provider_decision_table_shape" ]; then
  echo "PostgreSQL schema is incomplete: migration 012 payment provider decision table contract missing or mismatched: $missing_payment_provider_decision_table_shape" >&2
  exit 1
fi

missing_foreign_keys=$(psql -v ON_ERROR_STOP=1 -Atqc "
  WITH expected(table_name, child_columns, referenced_table, referenced_columns) AS (
    VALUES
      ('categories', ARRAY['tenant_id']::text[], 'tenants', ARRAY['id']::text[]),
      ('categories', ARRAY['tenant_id', 'parent_category_id']::text[], 'categories', ARRAY['tenant_id', 'id']::text[]),
      ('products', ARRAY['tenant_id']::text[], 'tenants', ARRAY['id']::text[]),
      ('products', ARRAY['tenant_id', 'category_id']::text[], 'categories', ARRAY['tenant_id', 'id']::text[]),
      ('product_variants', ARRAY['tenant_id']::text[], 'tenants', ARRAY['id']::text[]),
      ('product_variants', ARRAY['tenant_id', 'product_id']::text[], 'products', ARRAY['tenant_id', 'id']::text[]),
      ('prices', ARRAY['tenant_id']::text[], 'tenants', ARRAY['id']::text[]),
      ('prices', ARRAY['tenant_id', 'variant_id']::text[], 'product_variants', ARRAY['tenant_id', 'id']::text[]),
      ('promotions', ARRAY['tenant_id']::text[], 'tenants', ARRAY['id']::text[]),
      ('promotion_products', ARRAY['tenant_id']::text[], 'tenants', ARRAY['id']::text[]),
      ('promotion_products', ARRAY['tenant_id', 'promotion_id']::text[], 'promotions', ARRAY['tenant_id', 'id']::text[]),
      ('promotion_products', ARRAY['tenant_id', 'product_id']::text[], 'products', ARRAY['tenant_id', 'id']::text[]),
      ('customers', ARRAY['tenant_id']::text[], 'tenants', ARRAY['id']::text[]),
      ('reviews', ARRAY['tenant_id']::text[], 'tenants', ARRAY['id']::text[]),
      ('reviews', ARRAY['tenant_id', 'product_id']::text[], 'products', ARRAY['tenant_id', 'id']::text[]),
      ('reviews', ARRAY['tenant_id', 'customer_id']::text[], 'customers', ARRAY['tenant_id', 'id']::text[]),
      ('inventory_locations', ARRAY['tenant_id']::text[], 'tenants', ARRAY['id']::text[]),
      ('inventory', ARRAY['tenant_id']::text[], 'tenants', ARRAY['id']::text[]),
      ('inventory', ARRAY['tenant_id', 'variant_id']::text[], 'product_variants', ARRAY['tenant_id', 'id']::text[]),
      ('inventory', ARRAY['tenant_id', 'location_id']::text[], 'inventory_locations', ARRAY['tenant_id', 'id']::text[]),
      ('carts', ARRAY['tenant_id']::text[], 'tenants', ARRAY['id']::text[]),
      ('carts', ARRAY['tenant_id', 'customer_id']::text[], 'customers', ARRAY['tenant_id', 'id']::text[]),
      ('cart_items', ARRAY['tenant_id']::text[], 'tenants', ARRAY['id']::text[]),
      ('cart_items', ARRAY['tenant_id', 'cart_id']::text[], 'carts', ARRAY['tenant_id', 'id']::text[]),
      ('cart_items', ARRAY['tenant_id', 'variant_id']::text[], 'product_variants', ARRAY['tenant_id', 'id']::text[]),
      ('orders', ARRAY['tenant_id']::text[], 'tenants', ARRAY['id']::text[]),
      ('orders', ARRAY['tenant_id', 'customer_id']::text[], 'customers', ARRAY['tenant_id', 'id']::text[]),
      ('orders', ARRAY['tenant_id', 'cart_id']::text[], 'carts', ARRAY['tenant_id', 'id']::text[]),
      ('checkout_attempts', ARRAY['tenant_id']::text[], 'tenants', ARRAY['id']::text[]),
      ('checkout_attempts', ARRAY['tenant_id', 'order_id']::text[], 'orders', ARRAY['tenant_id', 'id']::text[]),
      ('order_items', ARRAY['tenant_id']::text[], 'tenants', ARRAY['id']::text[]),
      ('order_items', ARRAY['tenant_id', 'order_id']::text[], 'orders', ARRAY['tenant_id', 'id']::text[]),
      ('order_items', ARRAY['tenant_id', 'product_id']::text[], 'products', ARRAY['tenant_id', 'id']::text[]),
      ('order_items', ARRAY['tenant_id', 'product_id', 'variant_id']::text[], 'product_variants', ARRAY['tenant_id', 'product_id', 'id']::text[]),
      ('payments', ARRAY['tenant_id']::text[], 'tenants', ARRAY['id']::text[]),
      ('payments', ARRAY['tenant_id', 'order_id']::text[], 'orders', ARRAY['tenant_id', 'id']::text[]),
      ('payment_provider_authorization_decisions', ARRAY['tenant_id']::text[], 'tenants', ARRAY['id']::text[]),
      ('payment_provider_authorization_decisions', ARRAY['tenant_id', 'payment_id']::text[], 'payments', ARRAY['tenant_id', 'id']::text[]),
      ('shipments', ARRAY['tenant_id']::text[], 'tenants', ARRAY['id']::text[]),
      ('shipments', ARRAY['tenant_id', 'order_id']::text[], 'orders', ARRAY['tenant_id', 'id']::text[]),
      ('shipments', ARRAY['tenant_id', 'location_id']::text[], 'inventory_locations', ARRAY['tenant_id', 'id']::text[]),
      ('payment_operation_requests', ARRAY['tenant_id']::text[], 'tenants', ARRAY['id']::text[]),
      ('payment_operation_requests', ARRAY['tenant_id', 'payment_id']::text[], 'payments', ARRAY['tenant_id', 'id']::text[]),
      ('shipment_items', ARRAY['tenant_id']::text[], 'tenants', ARRAY['id']::text[]),
      ('shipment_items', ARRAY['tenant_id', 'shipment_id']::text[], 'shipments', ARRAY['tenant_id', 'id']::text[]),
      ('shipment_items', ARRAY['tenant_id', 'order_item_id']::text[], 'order_items', ARRAY['tenant_id', 'id']::text[]),
      ('outbox_events', ARRAY['tenant_id']::text[], 'tenants', ARRAY['id']::text[]),
      ('feature_exposures', ARRAY['tenant_id']::text[], 'tenants', ARRAY['id']::text[]),
      ('feature_exposures', ARRAY['tenant_id', 'customer_id']::text[], 'customers', ARRAY['tenant_id', 'id']::text[]),
      ('analytics_events', ARRAY['tenant_id']::text[], 'tenants', ARRAY['id']::text[]),
      ('analytics_events', ARRAY['tenant_id', 'customer_id']::text[], 'customers', ARRAY['tenant_id', 'id']::text[]),
      ('notification_deliveries', ARRAY['tenant_id']::text[], 'tenants', ARRAY['id']::text[]),
      ('notification_deliveries', ARRAY['tenant_id', 'order_id']::text[], 'orders', ARRAY['tenant_id', 'id']::text[]),
      ('fulfillment_processed_events', ARRAY['tenant_id']::text[], 'tenants', ARRAY['id']::text[]),
      ('fulfillment_processed_events', ARRAY['tenant_id', 'order_id']::text[], 'orders', ARRAY['tenant_id', 'id']::text[]),
      ('fulfillment_effects', ARRAY['tenant_id']::text[], 'tenants', ARRAY['id']::text[]),
      ('fulfillment_effects', ARRAY['tenant_id', 'order_id']::text[], 'orders', ARRAY['tenant_id', 'id']::text[]),
      ('price_change_events', ARRAY['tenant_id']::text[], 'tenants', ARRAY['id']::text[]),
      ('price_change_events', ARRAY['tenant_id', 'product_id']::text[], 'products', ARRAY['tenant_id', 'id']::text[]),
      ('price_change_events', ARRAY['tenant_id', 'variant_id']::text[], 'product_variants', ARRAY['tenant_id', 'id']::text[]),
      ('price_change_events', ARRAY['tenant_id', 'price_id']::text[], 'prices', ARRAY['tenant_id', 'id']::text[]),
      ('inventory_reservations', ARRAY['tenant_id']::text[], 'tenants', ARRAY['id']::text[]),
      ('inventory_reservations', ARRAY['tenant_id', 'variant_id']::text[], 'product_variants', ARRAY['tenant_id', 'id']::text[]),
      ('inventory_reservations', ARRAY['tenant_id', 'location_id']::text[], 'inventory_locations', ARRAY['tenant_id', 'id']::text[]),
      ('checkout_compensation_tasks', ARRAY['tenant_id']::text[], 'tenants', ARRAY['id']::text[]),
      ('checkout_compensation_tasks', ARRAY['tenant_id', 'order_id']::text[], 'orders', ARRAY['tenant_id', 'id']::text[]),
      ('checkout_compensation_tasks', ARRAY['tenant_id', 'payment_id']::text[], 'payments', ARRAY['tenant_id', 'id']::text[]),
      ('checkout_payment_reconciliations', ARRAY['tenant_id']::text[], 'tenants', ARRAY['id']::text[]),
      ('checkout_payment_reconciliations', ARRAY['tenant_id', 'order_id']::text[], 'orders', ARRAY['tenant_id', 'id']::text[]),
      ('checkout_payment_reconciliations', ARRAY['tenant_id', 'payment_id']::text[], 'payments', ARRAY['tenant_id', 'id']::text[]),
      ('notification_delivery_attempts', ARRAY['tenant_id', 'delivery_id']::text[], 'notification_deliveries', ARRAY['tenant_id', 'id']::text[]),
      ('notification_channel_messages', ARRAY['tenant_id', 'delivery_id']::text[], 'notification_deliveries', ARRAY['tenant_id', 'id']::text[]),
      ('notification_channel_messages', ARRAY['tenant_id', 'order_id']::text[], 'orders', ARRAY['tenant_id', 'id']::text[]),
      ('orders_consumer_inbox', ARRAY['tenant_id']::text[], 'tenants', ARRAY['id']::text[])
  ), actual AS (
    SELECT
      child_table.relname AS table_name,
      ARRAY(
        SELECT child_column.attname::text
        FROM unnest(constraint_row.conkey) WITH ORDINALITY AS key(attnum, ordinal)
        JOIN pg_attribute child_column
          ON child_column.attrelid = constraint_row.conrelid
         AND child_column.attnum = key.attnum
         AND NOT child_column.attisdropped
        ORDER BY key.ordinal
      ) AS child_columns,
      referenced_table.relname AS referenced_table,
      ARRAY(
        SELECT referenced_column.attname::text
        FROM unnest(constraint_row.confkey) WITH ORDINALITY AS key(attnum, ordinal)
        JOIN pg_attribute referenced_column
          ON referenced_column.attrelid = constraint_row.confrelid
         AND referenced_column.attnum = key.attnum
         AND NOT referenced_column.attisdropped
        ORDER BY key.ordinal
      ) AS referenced_columns
    FROM pg_constraint constraint_row
    JOIN pg_class child_table ON child_table.oid = constraint_row.conrelid
    JOIN pg_namespace child_schema ON child_schema.oid = child_table.relnamespace
    JOIN pg_class referenced_table ON referenced_table.oid = constraint_row.confrelid
    JOIN pg_namespace referenced_schema ON referenced_schema.oid = referenced_table.relnamespace
    WHERE constraint_row.contype = 'f'
      AND child_schema.nspname = 'public'
      AND referenced_schema.nspname = 'public'
  )
  SELECT COALESCE(
    string_agg(
      expected.table_name || ' (' || array_to_string(expected.child_columns, ', ') || ') -> '
        || expected.referenced_table || ' (' || array_to_string(expected.referenced_columns, ', ') || ')',
      ', ' ORDER BY expected.table_name, expected.child_columns::text
    ),
    ''
  )
  FROM expected
  WHERE NOT EXISTS (
    SELECT 1
    FROM actual
    WHERE actual.table_name = expected.table_name
      AND actual.child_columns = expected.child_columns
      AND actual.referenced_table = expected.referenced_table
      AND actual.referenced_columns = expected.referenced_columns
  );
")

missing_fulfillment_identity=$(psql -v ON_ERROR_STOP=1 -Atqc "
  SELECT CASE WHEN EXISTS (
    SELECT 1
    FROM pg_constraint constraint_row
    JOIN pg_class table_row ON table_row.oid = constraint_row.conrelid
    JOIN pg_namespace schema_row ON schema_row.oid = table_row.relnamespace
    WHERE schema_row.nspname = 'public'
      AND table_row.relname = 'fulfillment_processed_events'
      AND constraint_row.contype = 'p'
      AND pg_get_constraintdef(constraint_row.oid) =
          'PRIMARY KEY (tenant_id, consumer_name, event_key)'
  ) THEN '' ELSE 'fulfillment_processed_events (tenant_id, consumer_name, event_key)' END;
")

if [ -n "$missing_fulfillment_identity" ]; then
  echo "PostgreSQL schema is incomplete: missing tenant-scoped fulfillment identity: $missing_fulfillment_identity" >&2
  exit 1
fi

if [ -n "$missing_foreign_keys" ]; then
  echo "PostgreSQL schema is incomplete: missing foreign key relationship(s) for $missing_foreign_keys" >&2
  exit 1
fi

missing_payment_provider_decision_foreign_keys=$(psql -v ON_ERROR_STOP=1 -Atqc "
  WITH expected(label, child_columns, referenced_table, referenced_columns) AS (
    VALUES
      ('payment_provider_authorization_decisions (tenant_id) -> tenants (id) ON DELETE CASCADE',
       ARRAY['tenant_id']::text[], 'tenants', ARRAY['id']::text[]),
      ('payment_provider_authorization_decisions (tenant_id, payment_id) -> payments (tenant_id, id) ON DELETE CASCADE',
       ARRAY['tenant_id', 'payment_id']::text[], 'payments', ARRAY['tenant_id', 'id']::text[])
  ), actual AS (
    SELECT
      ARRAY(
        SELECT child_column.attname::text
        FROM unnest(constraint_row.conkey) WITH ORDINALITY AS key(attnum, ordinal)
        JOIN pg_attribute child_column
          ON child_column.attrelid = constraint_row.conrelid
         AND child_column.attnum = key.attnum
         AND NOT child_column.attisdropped
        ORDER BY key.ordinal
      ) AS child_columns,
      referenced_table.relname AS referenced_table,
      ARRAY(
        SELECT referenced_column.attname::text
        FROM unnest(constraint_row.confkey) WITH ORDINALITY AS key(attnum, ordinal)
        JOIN pg_attribute referenced_column
          ON referenced_column.attrelid = constraint_row.confrelid
         AND referenced_column.attnum = key.attnum
         AND NOT referenced_column.attisdropped
        ORDER BY key.ordinal
      ) AS referenced_columns,
      constraint_row.confdeltype AS delete_action
    FROM pg_constraint constraint_row
    JOIN pg_class child_table ON child_table.oid = constraint_row.conrelid
    JOIN pg_namespace child_schema ON child_schema.oid = child_table.relnamespace
    JOIN pg_class referenced_table ON referenced_table.oid = constraint_row.confrelid
    JOIN pg_namespace referenced_schema ON referenced_schema.oid = referenced_table.relnamespace
    WHERE constraint_row.contype = 'f'
      AND child_schema.nspname = 'public'
      AND child_table.relname = 'payment_provider_authorization_decisions'
      AND referenced_schema.nspname = 'public'
  )
  SELECT concat_ws(
    '; ',
    CASE WHEN (SELECT count(*) FROM actual) <> 2
      THEN 'payment_provider_authorization_decisions expected exactly 2 foreign keys, got '
        || (SELECT count(*) FROM actual)::text
    END,
    (
      SELECT string_agg(expected.label, ', ' ORDER BY expected.label)
      FROM expected
      WHERE NOT EXISTS (
        SELECT 1
        FROM actual
        WHERE actual.child_columns = expected.child_columns
          AND actual.referenced_table = expected.referenced_table
          AND actual.referenced_columns = expected.referenced_columns
          AND actual.delete_action = 'c'
      )
    )
  );
")

if [ -n "$missing_payment_provider_decision_foreign_keys" ]; then
  echo "PostgreSQL schema is incomplete: migration 012 payment provider decision foreign key contract(s) missing or mismatched: $missing_payment_provider_decision_foreign_keys" >&2
  exit 1
fi

missing_indexes=$(psql -v ON_ERROR_STOP=1 -Atqc "
  WITH expected(index_name) AS (
    VALUES
      ('idx_categories_tenant_parent'),
      ('idx_products_tenant_category_status'),
      ('idx_variants_tenant_product_status'),
      ('idx_prices_variant_validity'),
      ('idx_prices_one_current_default'),
      ('idx_promotions_active_window'),
      ('idx_promotion_products_product'),
      ('idx_customers_tenant_email'),
      ('idx_reviews_product_published'),
      ('idx_inventory_variant'),
      ('idx_inventory_location'),
      ('idx_carts_customer_status'),
      ('idx_cart_items_cart'),
      ('idx_orders_customer_created'),
      ('idx_orders_status_created'),
      ('idx_order_items_order'),
      ('idx_payments_order_status'),
      ('idx_shipments_order_status'),
      ('idx_outbox_pending'),
      ('idx_outbox_aggregate'),
      ('idx_fulfillment_processed_order'),
      ('idx_feature_exposures_feature_time'),
      ('idx_feature_exposures_customer_time'),
      ('idx_analytics_events_name_time'),
      ('idx_analytics_events_entity_time'),
      ('idx_analytics_events_trace'),
      ('idx_notification_deliveries_order'),
      ('idx_payment_operation_requests_payment'),
      ('idx_checkout_attempts_order'),
      ('idx_fulfillment_processed_active_lease'),
      ('idx_fulfillment_processed_retry'),
      ('idx_price_change_events_tenant_sku_sequence'),
      ('uq_payments_one_active_order'),
      ('uq_orders_checkout_request'),
      ('idx_inventory_reservations_status'),
      ('idx_inventory_reservations_sku'),
      ('idx_inventory_reservations_expiry'),
      ('idx_inventory_reservations_owner'),
      ('idx_checkout_compensation_queue'),
      ('idx_checkout_compensation_order'),
      ('idx_checkout_compensation_reservation'),
      ('idx_checkout_compensation_payment'),
      ('idx_checkout_attempts_lease'),
      ('idx_checkout_compensation_fence'),
      ('idx_checkout_payment_reconciliation_queue'),
      ('idx_checkout_payment_reconciliation_payment'),
      ('idx_checkout_payment_reconciliation_parent_generation'),
      ('idx_outbox_claim_expiry'),
      ('idx_payment_provider_decisions_lookup'),
      ('idx_fulfillment_effects_dispatch'),
      ('idx_fulfillment_effects_claim_expiry'),
      ('idx_payments_pending_reconciliation'),
      ('idx_notification_deliveries_dispatch_queue'),
      ('idx_notification_deliveries_expired_leases'),
      ('idx_notification_delivery_attempts_tenant'),
      ('idx_outbox_dead_letters'),
      ('idx_orders_consumer_inbox_active')
  )
  SELECT COALESCE(string_agg(expected.index_name, ', ' ORDER BY expected.index_name), '')
  FROM expected
  WHERE to_regclass('public.' || expected.index_name) IS NULL;
")

if [ -n "$missing_indexes" ]; then
  echo "PostgreSQL schema is incomplete: missing index(es): $missing_indexes" >&2
  exit 1
fi

missing_index_contracts=$(psql -v ON_ERROR_STOP=1 -Atqc "
  WITH expected(index_name, table_name, column_names, predicate_fragment, predicate_fragment_two) AS (
    VALUES
      ('idx_checkout_attempts_lease', 'checkout_attempts',
       ARRAY['tenant_id', 'request_id', 'lease_token', 'status']::text[], '', ''),
      ('idx_inventory_reservations_expiry', 'inventory_reservations',
       ARRAY['expires_at', 'tenant_id', 'reservation_id']::text[], 'status = ''reserved''', ''),
      ('idx_inventory_reservations_owner', 'inventory_reservations',
       ARRAY['tenant_id', 'owner_request_id', 'owner_lease_token']::text[], '', ''),
      ('idx_checkout_compensation_fence', 'checkout_compensation_tasks',
       ARRAY['tenant_id', 'checkout_request_id', 'checkout_lease_token']::text[],
       'checkout_request_id IS NOT NULL', ''),
      ('idx_checkout_payment_reconciliation_queue', 'checkout_payment_reconciliations',
       ARRAY['available_at', 'updated_at']::text[], 'queued', 'processing'),
      ('idx_checkout_payment_reconciliation_payment', 'checkout_payment_reconciliations',
       ARRAY['tenant_id', 'payment_id', 'status']::text[], '', ''),
      ('idx_checkout_payment_reconciliation_parent_generation', 'checkout_payment_reconciliations',
       ARRAY['tenant_id', 'request_id', 'checkout_lease_token']::text[], '', ''),
      ('idx_outbox_claim_expiry', 'outbox_events',
       ARRAY['claim_expires_at', 'created_at']::text[], 'status = ''processing''', ''),
      ('idx_payment_provider_decisions_lookup', 'payment_provider_authorization_decisions',
       ARRAY['tenant_id', 'provider', 'provider_reference', 'verified_at']::text[], '', ''),
      ('idx_fulfillment_effects_dispatch', 'fulfillment_effects',
       ARRAY['tenant_id', 'order_id', 'status', 'created_at', 'effect_key']::text[], 'status = ANY', ''),
      ('idx_fulfillment_effects_claim_expiry', 'fulfillment_effects',
       ARRAY['claim_until']::text[], 'status = ''publishing''', ''),
      ('idx_payments_pending_reconciliation', 'payments',
       ARRAY['pending_reconciliation_at', 'tenant_id', 'id']::text[], 'status = ''pending''', ''),
      ('idx_notification_deliveries_dispatch_queue', 'notification_deliveries',
       ARRAY['next_attempt_at', 'created_at', 'id']::text[], 'accepted', 'queued'),
      ('idx_notification_deliveries_expired_leases', 'notification_deliveries',
       ARRAY['lease_until', 'updated_at', 'id']::text[], 'status = ''processing''', ''),
      ('idx_notification_delivery_attempts_tenant', 'notification_delivery_attempts',
       ARRAY['tenant_id', 'delivery_id', 'attempt']::text[], '', '')
  ), actual AS (
    SELECT
      index_row.relname AS index_name,
      table_row.relname AS table_name,
      ARRAY(
        SELECT attribute_row.attname::text
        FROM unnest(index_meta.indkey) WITH ORDINALITY AS key(attnum, ordinal)
        JOIN pg_attribute attribute_row
          ON attribute_row.attrelid = index_meta.indrelid
         AND attribute_row.attnum = key.attnum
         AND NOT attribute_row.attisdropped
        ORDER BY key.ordinal
      ) AS column_names,
      COALESCE(pg_get_expr(index_meta.indpred, index_meta.indrelid), '') AS predicate
    FROM pg_index index_meta
    JOIN pg_class index_row ON index_row.oid = index_meta.indexrelid
    JOIN pg_class table_row ON table_row.oid = index_meta.indrelid
    JOIN pg_namespace schema_row ON schema_row.oid = table_row.relnamespace
    WHERE schema_row.nspname = 'public'
  )
  SELECT COALESCE(
    string_agg(expected.index_name, ', ' ORDER BY expected.index_name),
    ''
  )
  FROM expected
  WHERE NOT EXISTS (
    SELECT 1
    FROM actual
    WHERE actual.index_name = expected.index_name
      AND actual.table_name = expected.table_name
      AND actual.column_names = expected.column_names
      AND (expected.predicate_fragment = ''
           OR actual.predicate LIKE '%' || expected.predicate_fragment || '%')
      AND (expected.predicate_fragment_two = ''
           OR actual.predicate LIKE '%' || expected.predicate_fragment_two || '%')
  );
")

if [ -n "$missing_index_contracts" ]; then
  echo "PostgreSQL schema is incomplete: new index definition contract(s) missing: $missing_index_contracts" >&2
  exit 1
fi

missing_payment_provider_decision_index=$(psql -v ON_ERROR_STOP=1 -Atqc "
  WITH actual AS (
    SELECT indexname, indexdef
    FROM pg_indexes
    WHERE schemaname = 'public'
      AND tablename = 'payment_provider_authorization_decisions'
  )
  SELECT CASE
    WHEN (SELECT count(*) FROM actual) <> 3
      THEN 'payment_provider_authorization_decisions expected exactly 3 indexes, got '
        || (SELECT count(*) FROM actual)::text
    WHEN NOT EXISTS (
      SELECT 1
      FROM actual
      WHERE indexname = 'idx_payment_provider_decisions_lookup'
    ) THEN 'idx_payment_provider_decisions_lookup is missing'
    WHEN NOT EXISTS (
      SELECT 1
      FROM actual
      WHERE indexname = 'idx_payment_provider_decisions_lookup'
        AND indexdef LIKE 'CREATE INDEX %'
        AND indexdef LIKE '%(tenant_id, provider, provider_reference, verified_at DESC)%'
        AND indexdef NOT LIKE '% WHERE %'
    ) THEN 'idx_payment_provider_decisions_lookup definition mismatch: '
      || (SELECT indexdef
          FROM actual
          WHERE indexname = 'idx_payment_provider_decisions_lookup')
    ELSE ''
  END;
")

if [ -n "$missing_payment_provider_decision_index" ]; then
  echo "PostgreSQL schema is incomplete: migration 012 payment provider decision index contract missing or mismatched: $missing_payment_provider_decision_index" >&2
  exit 1
fi

missing_recovery_objects=$(psql -v ON_ERROR_STOP=1 -Atqc "
  SELECT COALESCE(string_agg(object_name, ', ' ORDER BY object_name), '')
  FROM (
    SELECT 'outbox_dead_letters' AS object_name
    WHERE to_regclass('public.outbox_dead_letters') IS NULL
    UNION ALL
    SELECT 'replay_failed_outbox_event(text,text)'
    WHERE to_regprocedure('public.replay_failed_outbox_event(text,text)') IS NULL
  ) missing;
")

if [ -n "$missing_recovery_objects" ]; then
  echo "PostgreSQL schema is incomplete: missing outbox recovery object(s): $missing_recovery_objects" >&2
  exit 1
fi

missing_unique_indexes=$(psql -v ON_ERROR_STOP=1 -Atqc "
  WITH expected(index_name) AS (
    VALUES
      ('idx_prices_one_current_default'),
      ('idx_customers_tenant_email'),
      ('uq_payments_one_active_order'),
      ('uq_orders_checkout_request')
  )
  SELECT COALESCE(string_agg(expected.index_name, ', ' ORDER BY expected.index_name), '')
  FROM expected
  WHERE NOT EXISTS (
    SELECT 1
    FROM pg_indexes actual
    WHERE actual.schemaname = 'public'
      AND actual.indexname = expected.index_name
      AND actual.indexdef LIKE 'CREATE UNIQUE INDEX%'
  );
")

if [ -n "$missing_unique_indexes" ]; then
  echo "PostgreSQL schema is incomplete: missing unique index(es): $missing_unique_indexes" >&2
  exit 1
fi

missing_unique_constraints=$(psql -v ON_ERROR_STOP=1 -Atqc "
  WITH expected(table_name, required_fragment) AS (
    VALUES
      ('outbox_events', 'tenant_id, event_key'),
      ('analytics_events', 'tenant_id, event_key'),
      ('notification_deliveries', 'tenant_id, event_key'),
      ('checkout_compensation_tasks', 'tenant_id, task_key'),
      ('price_change_events', 'tenant_id, price_id'),
      ('checkout_payment_reconciliations', 'tenant_id, authorize_request_id'),
      ('payment_provider_authorization_decisions', 'tenant_id, provider, provider_reference'),
      ('notification_delivery_attempts', 'delivery_id, attempt'),
      ('notification_channel_messages', 'tenant_id, delivery_id')
  )
  SELECT COALESCE(
    string_agg(expected.table_name || ' (' || expected.required_fragment || ')', ', ' ORDER BY expected.table_name),
    ''
  )
  FROM expected
  WHERE NOT EXISTS (
    SELECT 1
    FROM pg_constraint constraint_row
    JOIN pg_class table_row ON table_row.oid = constraint_row.conrelid
    JOIN pg_namespace schema_row ON schema_row.oid = table_row.relnamespace
    WHERE schema_row.nspname = 'public'
      AND table_row.relname = expected.table_name
      AND constraint_row.contype = 'u'
      AND pg_get_constraintdef(constraint_row.oid) LIKE '%(' || expected.required_fragment || ')%'
  );
")

if [ -n "$missing_unique_constraints" ]; then
  echo "PostgreSQL schema is incomplete: missing unique constraint(s): $missing_unique_constraints" >&2
  exit 1
fi

missing_payment_provider_decision_unique_constraint=$(psql -v ON_ERROR_STOP=1 -Atqc "
  WITH actual AS (
    SELECT pg_get_constraintdef(constraint_row.oid) AS definition
    FROM pg_constraint constraint_row
    JOIN pg_class table_row ON table_row.oid = constraint_row.conrelid
    JOIN pg_namespace schema_row ON schema_row.oid = table_row.relnamespace
    WHERE schema_row.nspname = 'public'
      AND table_row.relname = 'payment_provider_authorization_decisions'
      AND constraint_row.contype = 'u'
  )
  SELECT CASE
    WHEN (SELECT count(*) FROM actual) <> 1
      THEN 'payment_provider_authorization_decisions expected exactly 1 UNIQUE constraint, got '
        || (SELECT count(*) FROM actual)::text
    WHEN NOT EXISTS (
      SELECT 1
      FROM actual
      WHERE definition = 'UNIQUE (tenant_id, provider, provider_reference)'
    ) THEN 'payment_provider_authorization_decisions UNIQUE (tenant_id, provider, provider_reference) is missing'
    ELSE ''
  END;
")

if [ -n "$missing_payment_provider_decision_unique_constraint" ]; then
  echo "PostgreSQL schema is incomplete: migration 012 payment provider decision unique constraint contract missing or mismatched: $missing_payment_provider_decision_unique_constraint" >&2
  exit 1
fi

missing_checks=$(psql -v ON_ERROR_STOP=1 -Atqc "
  WITH expected(table_name, required_fragment) AS (
    VALUES
      ('prices', 'amount_minor >= 0'),
      ('orders', 'total_minor ='),
      ('payments', 'status <> ''captured'''),
      ('outbox_events', 'published_at IS NOT NULL'),
      ('outbox_events', 'length(btrim(traceparent)) > 0'),
      ('outbox_events', 'length(btrim(tracestate)) > 0'),
      ('outbox_events', 'length(btrim(baggage)) > 0'),
      ('analytics_events', 'jsonb_typeof(properties)'),
      ('inventory_reservations', 'released_at IS NULL'),
      ('inventory_reservations', '= ANY'),
      ('inventory_reservations', 'consumed_at IS NOT NULL'),
      ('checkout_attempts', 'length(btrim(lease_token)) > 0'),
      ('checkout_compensation_tasks', 'kind = ''inventory_release'''),
      ('checkout_compensation_tasks', 'failed'),
      ('checkout_compensation_tasks', 'remote_operation_id IS NULL'),
      ('checkout_payment_reconciliations', 'amount_minor >= 0'),
      ('checkout_payment_reconciliations', 'char_length(currency) = 3'),
      ('checkout_payment_reconciliations', 'currency = upper(currency)'),
      ('checkout_payment_reconciliations', 'method_type = ANY'),
      ('checkout_payment_reconciliations', 'status = ANY'),
      ('checkout_payment_reconciliations', 'attempts >= 0'),
      ('checkout_payment_reconciliations', 'completed_at IS NOT NULL'),
      ('checkout_payment_reconciliations', 'lease_token IS NOT NULL'),
      ('checkout_payment_reconciliations', 'lease_expires_at IS NOT NULL'),
      ('payments', 'pending_resolution = ANY'),
      ('payments', 'status <> ''pending'''),
      ('payments', 'pending_resolution_at IS NULL'),
      ('payments', 'pending_reconciliation_attempts >= 0'),
      ('payment_provider_authorization_decisions', 'amount_minor >= 0'),
      ('payment_provider_authorization_decisions', 'char_length(currency) = 3'),
      ('payment_provider_authorization_decisions', 'currency = upper(currency)'),
      ('payment_provider_authorization_decisions', 'outcome = ANY'),
      ('notification_deliveries', 'status = ANY'),
      ('notification_deliveries', 'delivered_at IS NOT NULL'),
      ('notification_deliveries', 'dead_lettered_at IS NOT NULL'),
      ('notification_deliveries', 'lease_until IS NOT NULL'),
      ('notification_deliveries', 'lease_token IS NOT NULL'),
      ('fulfillment_effects', 'operation = ANY'),
      ('fulfillment_effects', 'effect_kind = ANY'),
      ('fulfillment_effects', 'jsonb_typeof(payload)'),
      ('fulfillment_effects', 'status = ANY'),
      ('fulfillment_effects', 'attempts >= 0'),
      ('fulfillment_effects', 'claim_token IS NOT NULL'),
      ('fulfillment_effects', 'published_at IS NOT NULL'),
      ('fulfillment_effects', 'cancelled_at IS NOT NULL'),
      ('notification_delivery_attempts', 'attempt > 0'),
      ('notification_delivery_attempts', 'outcome = ANY'),
      ('notification_delivery_attempts', 'completed_at IS NOT NULL'),
      ('notification_channel_messages', 'channel = ANY'),
      ('notification_channel_messages', 'jsonb_typeof(payload)'),
      ('price_change_events', 'amount_minor >= 0'),
      ('fulfillment_processed_events', 'dead_lettered'),
      ('fulfillment_processed_events', 'attempts <= 5')
  )
  SELECT COALESCE(
    string_agg(expected.table_name || ' [' || expected.required_fragment || ']', ', ' ORDER BY expected.table_name),
    ''
  )
  FROM expected
  WHERE NOT EXISTS (
    SELECT 1
    FROM pg_constraint constraint_row
    JOIN pg_class table_row ON table_row.oid = constraint_row.conrelid
    JOIN pg_namespace schema_row ON schema_row.oid = table_row.relnamespace
    WHERE schema_row.nspname = 'public'
      AND table_row.relname = expected.table_name
      AND constraint_row.contype = 'c'
      AND pg_get_constraintdef(constraint_row.oid) LIKE '%' || expected.required_fragment || '%'
  );
")

if [ -n "$missing_checks" ]; then
  echo "PostgreSQL schema is incomplete: missing CHECK constraint(s): $missing_checks" >&2
  exit 1
fi

missing_payment_provider_decision_checks=$(psql -v ON_ERROR_STOP=1 -Atqc "
  WITH expected(fragment) AS (
    VALUES
      ('amount_minor >= 0'),
      ('char_length(currency) = 3'),
      ('currency = upper(currency)'),
      ('outcome = ANY')
  ), actual AS (
    SELECT pg_get_constraintdef(constraint_row.oid) AS definition
    FROM pg_constraint constraint_row
    JOIN pg_class table_row ON table_row.oid = constraint_row.conrelid
    JOIN pg_namespace schema_row ON schema_row.oid = table_row.relnamespace
    WHERE schema_row.nspname = 'public'
      AND table_row.relname = 'payment_provider_authorization_decisions'
      AND constraint_row.contype = 'c'
  )
  SELECT concat_ws(
    '; ',
    CASE WHEN (SELECT count(*) FROM actual) <> 3
      THEN 'payment_provider_authorization_decisions expected exactly 3 CHECK constraints, got '
        || (SELECT count(*) FROM actual)::text
    END,
    (
      SELECT string_agg('payment_provider_authorization_decisions CHECK [' || expected.fragment || ']', ', ' ORDER BY expected.fragment)
      FROM expected
      WHERE NOT EXISTS (
        SELECT 1
        FROM actual
        WHERE actual.definition LIKE '%' || expected.fragment || '%'
      )
    )
  );
")

if [ -n "$missing_payment_provider_decision_checks" ]; then
  echo "PostgreSQL schema is incomplete: migration 012 payment provider decision CHECK contract(s) missing or mismatched: $missing_payment_provider_decision_checks" >&2
  exit 1
fi

missing_named_constraints=$(psql -v ON_ERROR_STOP=1 -Atqc "
  WITH expected(table_name, constraint_name) AS (
    VALUES
      ('checkout_attempts', 'checkout_attempts_lease_token_nonempty'),
      ('checkout_compensation_tasks', 'checkout_compensation_tasks_remote_operation_check'),
      ('checkout_payment_reconciliations', 'checkout_payment_reconciliations_lease_check'),
      ('checkout_payment_reconciliations', 'checkout_payment_reconciliations_parent_generation_check'),
      ('outbox_events', 'outbox_claim_check'),
      ('inventory_reservations', 'inventory_reservations_status_check'),
      ('inventory_reservations', 'inventory_reservations_lifecycle_check'),
      ('payments', 'payments_pending_resolution_value_check'),
      ('payments', 'payments_pending_reconciliation_state_check'),
      ('payments', 'payments_pending_resolution_timestamp_check'),
      ('payments', 'payments_pending_reconciliation_attempts_check'),
      ('payments', 'payments_pending_reconciliation_lifecycle_check'),
      ('notification_deliveries', 'notification_deliveries_status_check'),
      ('notification_deliveries', 'notification_deliveries_delivered_at_check'),
      ('notification_deliveries', 'notification_deliveries_dead_lettered_at_check'),
      ('notification_deliveries', 'notification_deliveries_lease_check')
  )
  SELECT COALESCE(
    string_agg(expected.table_name || '.' || expected.constraint_name, ', ' ORDER BY expected.table_name, expected.constraint_name),
    ''
  )
  FROM expected
  WHERE NOT EXISTS (
    SELECT 1
    FROM pg_constraint constraint_row
    JOIN pg_class table_row ON table_row.oid = constraint_row.conrelid
    JOIN pg_namespace schema_row ON schema_row.oid = table_row.relnamespace
    WHERE schema_row.nspname = 'public'
      AND table_row.relname = expected.table_name
      AND constraint_row.conname = expected.constraint_name
      AND constraint_row.contype = 'c'
  );
")

if [ -n "$missing_named_constraints" ]; then
  echo "PostgreSQL schema is incomplete: missing named CHECK constraint(s): $missing_named_constraints" >&2
  exit 1
fi

echo "PostgreSQL schema verified: $expected_tables"
