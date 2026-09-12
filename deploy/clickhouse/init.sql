-- Idempotent analytics schema. Payloads stay as JSON text so ingestion works
-- across current ClickHouse stable releases without optional JSON settings.

CREATE DATABASE IF NOT EXISTS analytics;

CREATE TABLE IF NOT EXISTS analytics.analytics_events
(
    event_id UUID,
    tenant_id LowCardinality(String),
    event_key String,
    customer_id Nullable(String),
    session_id Nullable(String),
    event_name LowCardinality(String),
    event_version UInt16 DEFAULT 1,
    source LowCardinality(String),
    entity_type LowCardinality(String),
    entity_id String,
    occurred_at DateTime64(3, 'UTC'),
    received_at DateTime64(3, 'UTC') DEFAULT now64(3),
    trace_id String DEFAULT '',
    span_id String DEFAULT '',
    traceparent String DEFAULT '',
    tracestate String DEFAULT '',
    baggage String DEFAULT '',
    feature_variant String DEFAULT '',
    properties String DEFAULT '{}',
    context String DEFAULT '{}',
    is_test UInt8 DEFAULT 0,
    INDEX idx_analytics_events_key event_key TYPE bloom_filter(0.01) GRANULARITY 4,
    INDEX idx_analytics_events_trace trace_id TYPE bloom_filter(0.01) GRANULARITY 4
)
-- The event identity is the replacing key. Consumers may retry after a
-- successful insert but before acknowledging RabbitMQ; FINAL reads one row.
ENGINE = ReplacingMergeTree(received_at)
PARTITION BY toYYYYMM(occurred_at)
ORDER BY (tenant_id, event_id)
SETTINGS index_granularity = 8192;

CREATE TABLE IF NOT EXISTS analytics.feature_exposures
(
    exposure_id UUID,
    tenant_id LowCardinality(String),
    exposure_key String,
    customer_id Nullable(String),
    anonymous_id Nullable(String),
    session_id Nullable(String),
    feature_key LowCardinality(String),
    variant LowCardinality(String),
    exposed_at DateTime64(3, 'UTC'),
    received_at DateTime64(3, 'UTC') DEFAULT now64(3),
    context String DEFAULT '{}',
    trace_id String DEFAULT '',
    span_id String DEFAULT '',
    is_test UInt8 DEFAULT 0,
    INDEX idx_feature_exposures_key exposure_key TYPE bloom_filter(0.01) GRANULARITY 4
)
ENGINE = ReplacingMergeTree(received_at)
PARTITION BY toYYYYMM(exposed_at)
ORDER BY (tenant_id, exposure_id)
SETTINGS index_granularity = 8192;

-- Keep existing volumes aligned with the clean-install default. This changes
-- only future omitted values; existing rows retain their recorded flag.
ALTER TABLE analytics.analytics_events
    MODIFY COLUMN is_test UInt8 DEFAULT 0;

ALTER TABLE analytics.feature_exposures
    MODIFY COLUMN is_test UInt8 DEFAULT 0;
