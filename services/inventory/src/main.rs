//! Inventory HTTP service — reserves stock for a SKU. Chaos knobs:
//!   ?slow=<ms>   real pg_sleep latency
//!   ?db_n1=<n>   repeated stock SELECTs before update, bounded at 50
//!   ?hold_ms=<n> hold one pool connection, bounded at 10s
//!   ?fail=1      reservation failure -> 503 + ERROR span (B2)
use anyhow::Context;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::{Json, Router, routing::get};
use opentelemetry::global;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::pool::PoolConnection;
use sqlx::postgres::{PgPool, PgPoolOptions};
use sqlx::{Postgres, Row};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tracing::Instrument;

const DEFAULT_DATABASE_URL: &str = "postgres://postgres:playground@postgres:5432/playground";
const DB_MAX_CONNECTIONS: u32 = 5;
const DB_ACQUIRE_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_DB_N1: u32 = 50;
const MAX_HOLD_MS: u64 = 10_000;

const CREATE_STOCK_SQL: &str =
    "CREATE TABLE IF NOT EXISTS stock (sku TEXT PRIMARY KEY, quantity BIGINT NOT NULL)";
const UPSERT_STOCK_SQL: &str = "INSERT INTO stock (sku, quantity) VALUES ($1, $2) ON CONFLICT (sku) DO UPDATE SET quantity = EXCLUDED.quantity";
const RESERVE_STOCK_SQL: &str = "UPDATE stock SET quantity = quantity - $1 WHERE sku = $2 AND quantity >= $1 RETURNING quantity";
const SELECT_STOCK_SQL: &str = "SELECT quantity FROM stock WHERE sku = $1";
const SLOW_QUERY_SQL: &str = "SELECT pg_sleep($1::float / 1000)";

const SEED_STOCK: &[(&str, i64)] = &[
    ("WIDGET-1", 100_000),
    ("WIDGET-2", 100_000),
    ("WIDGET-3", 100_000),
    ("WIDGET-4", 100_000),
    ("GADGET-1", 100_000),
];

#[derive(Deserialize)]
struct Reserve {
    sku: String,
    #[serde(default = "one")]
    quantity: u32,
    #[serde(default)]
    slow: u64,
    #[serde(default)]
    db_n1: u32,
    #[serde(default)]
    hold_ms: u64,
    #[serde(default, deserialize_with = "de_flag")]
    fail: bool,
}
fn one() -> u32 {
    1
}
fn de_flag<'de, D: serde::Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    let s = String::deserialize(d)?;
    Ok(matches!(s.as_str(), "1" | "true" | "yes" | "on"))
}

type InventoryResponse = (StatusCode, Json<Value>);

#[derive(Clone)]
struct AppState {
    db: Option<DbState>,
}

#[derive(Clone)]
struct DbState {
    pool: PgPool,
    pending: Arc<AtomicU64>,
    timeouts: Arc<AtomicU64>,
}

async fn reserve(
    headers: HeaderMap,
    State(state): State<AppState>,
    Query(p): Query<Reserve>,
) -> impl IntoResponse {
    let span = tracing::info_span!("reserve", otel.kind = "server");
    playground_telemetry::set_parent_from_headers(&span, &headers);
    reserve_inner(state, p).instrument(span).await
}

async fn reserve_inner(state: AppState, p: Reserve) -> InventoryResponse {
    if let Some(db) = state.db {
        return reserve_db(db, p).await;
    }
    reserve_memory(p).await
}

async fn reserve_memory(p: Reserve) -> InventoryResponse {
    if p.slow > 0 {
        tracing::info!(ms = p.slow, "slow db query (chaos)");
        tokio::time::sleep(Duration::from_millis(p.slow)).await;
    }
    if p.hold_ms > 0 {
        tokio::time::sleep(Duration::from_millis(p.hold_ms.min(MAX_HOLD_MS))).await;
    }
    if p.fail {
        playground_telemetry::mark_span_error("out_of_stock");
        tracing::error!(sku = %p.sku, "reservation failed (chaos)");
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "out of stock", "sku": p.sku })),
        );
    }
    tracing::info!(sku = %p.sku, quantity = p.quantity, "reserved");
    (
        StatusCode::OK,
        Json(json!({ "sku": p.sku, "reserved": p.quantity, "in_stock": true })),
    )
}

async fn reserve_db(db: DbState, p: Reserve) -> InventoryResponse {
    if p.hold_ms > 0
        && let Err(err) = hold_connection(&db, p.hold_ms.min(MAX_HOLD_MS)).await
    {
        return db_error_response(err, p.sku);
    }
    if p.slow > 0
        && let Err(err) = run_slow_query(&db, p.slow).await
    {
        return db_error_response(err, p.sku);
    }
    for _ in 0..p.db_n1.min(MAX_DB_N1) {
        if let Err(err) = select_stock_once(&db, &p.sku).await {
            return db_error_response(err, p.sku);
        }
    }
    if p.fail {
        playground_telemetry::mark_span_error("out_of_stock");
        tracing::error!(sku = %p.sku, "reservation failed (chaos)");
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "out of stock", "sku": p.sku })),
        );
    }

    let sku = p.sku.clone();
    match reserve_stock(&db, &p.sku, p.quantity).await {
        Ok(Some(remaining)) => {
            tracing::info!(sku = %p.sku, quantity = p.quantity, remaining, "reserved");
            (
                StatusCode::OK,
                Json(json!({
                    "sku": p.sku,
                    "reserved": p.quantity,
                    "remaining": remaining,
                    "in_stock": true,
                })),
            )
        }
        Ok(None) => {
            playground_telemetry::mark_span_error("out_of_stock");
            tracing::error!(sku = %p.sku, quantity = p.quantity, "reservation failed");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "error": "out of stock", "sku": p.sku })),
            )
        }
        Err(err) => db_error_response(err, sku),
    }
}

async fn hold_connection(db: &DbState, hold_ms: u64) -> Result<(), sqlx::Error> {
    let span = tracing::info_span!(
        "postgres.pool",
        otel.kind = "client",
        "db.system.name" = "postgresql",
        "db.namespace" = "playground",
        "db.operation.name" = "ACQUIRE",
        "db.query.summary" = "pool hold",
        "server.address" = "postgres",
        "server.port" = 5432_i64,
    );
    async {
        let result = async {
            let _conn = acquire(db).await?;
            tracing::info!(hold_ms, "holding postgres connection");
            tokio::time::sleep(Duration::from_millis(hold_ms)).await;
            Ok(())
        }
        .await;
        if let Err(err) = &result {
            mark_current_db_error(err);
        }
        result
    }
    .instrument(span)
    .await
}

async fn run_slow_query(db: &DbState, slow_ms: u64) -> Result<(), sqlx::Error> {
    let span = playground_telemetry::db_span("SELECT", "SELECT pg_sleep", SLOW_QUERY_SQL);
    async {
        let result = async {
            let mut conn = acquire(db).await?;
            sqlx::query(SLOW_QUERY_SQL)
                .bind(slow_ms as f64)
                .execute(&mut *conn)
                .await?;
            Ok(())
        }
        .await;
        if let Err(err) = &result {
            mark_current_db_error(err);
        }
        result
    }
    .instrument(span)
    .await
}

async fn select_stock_once(db: &DbState, sku: &str) -> Result<Option<i64>, sqlx::Error> {
    let span = playground_telemetry::db_span("SELECT", "SELECT stock quantity", SELECT_STOCK_SQL);
    async {
        let result = async {
            let mut conn = acquire(db).await?;
            sqlx::query_scalar::<_, i64>(SELECT_STOCK_SQL)
                .bind(sku)
                .fetch_optional(&mut *conn)
                .await
        }
        .await;
        if let Err(err) = &result {
            mark_current_db_error(err);
        }
        result
    }
    .instrument(span)
    .await
}

async fn reserve_stock(db: &DbState, sku: &str, quantity: u32) -> Result<Option<i64>, sqlx::Error> {
    let span = playground_telemetry::db_span("UPDATE", "UPDATE stock reserve", RESERVE_STOCK_SQL);
    async {
        let result = async {
            let mut conn = acquire(db).await?;
            sqlx::query(RESERVE_STOCK_SQL)
                .bind(i64::from(quantity))
                .bind(sku)
                .fetch_optional(&mut *conn)
                .await
                .map(|row| row.map(|row| row.get::<i64, _>("quantity")))
        }
        .await;
        if let Err(err) = &result {
            mark_current_db_error(err);
        }
        result
    }
    .instrument(span)
    .await
}

async fn acquire(db: &DbState) -> Result<PoolConnection<Postgres>, sqlx::Error> {
    db.pending.fetch_add(1, Ordering::Relaxed);
    let started = Instant::now();
    let result = db.pool.acquire().await;
    let wait_ms = started.elapsed().as_millis() as u64;
    db.pending.fetch_sub(1, Ordering::Relaxed);
    if matches!(result, Err(sqlx::Error::PoolTimedOut)) {
        db.timeouts.fetch_add(1, Ordering::Relaxed);
    }
    if wait_ms > 0 {
        tracing::debug!(wait_ms, "postgres pool acquire wait");
    }
    result
}

fn db_error_response(err: sqlx::Error, sku: String) -> InventoryResponse {
    if matches!(err, sqlx::Error::PoolTimedOut) {
        playground_telemetry::mark_span_error("pool_exhausted");
        tracing::error!(sku = %sku, error = %err, "postgres pool exhausted");
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "pool exhausted", "sku": sku })),
        );
    }
    playground_telemetry::mark_span_error("db_error");
    tracing::error!(sku = %sku, error = %err, "postgres reservation failed");
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({ "error": "database unavailable", "sku": sku })),
    )
}

fn mark_current_db_error(err: &sqlx::Error) {
    if matches!(err, sqlx::Error::PoolTimedOut) {
        playground_telemetry::mark_span_error("pool_exhausted");
    } else {
        playground_telemetry::mark_span_error("db_error");
    }
}

async fn init_db() -> anyhow::Result<Option<DbState>> {
    if inventory_no_db_enabled() {
        tracing::warn!("INVENTORY_NO_DB=1 active; using in-memory inventory fallback");
        return Ok(None);
    }

    let database_url =
        std::env::var("DATABASE_URL").unwrap_or_else(|_| DEFAULT_DATABASE_URL.to_string());
    let pool = PgPoolOptions::new()
        .max_connections(DB_MAX_CONNECTIONS)
        .acquire_timeout(DB_ACQUIRE_TIMEOUT)
        .connect(&database_url)
        .await
        .with_context(|| {
            format!(
                "inventory cannot connect to Postgres at {database_url}; set INVENTORY_NO_DB=1 for no-Docker demos"
            )
        })?;
    bootstrap_stock(&pool).await?;
    tracing::info!("connected to postgres");
    let db = DbState {
        pool,
        pending: Arc::new(AtomicU64::new(0)),
        timeouts: Arc::new(AtomicU64::new(0)),
    };
    spawn_pool_metrics(db.clone());
    Ok(Some(db))
}

async fn bootstrap_stock(pool: &PgPool) -> anyhow::Result<()> {
    sqlx::query(CREATE_STOCK_SQL)
        .execute(pool)
        .await
        .context("create stock table")?;
    for (sku, quantity) in SEED_STOCK {
        sqlx::query(UPSERT_STOCK_SQL)
            .bind(*sku)
            .bind(*quantity)
            .execute(pool)
            .await
            .with_context(|| format!("seed stock row {sku}"))?;
    }
    Ok(())
}

fn spawn_pool_metrics(db: DbState) {
    tokio::spawn(async move {
        let meter = global::meter("playground.db");
        let connection_count = meter
            .u64_gauge("db.client.connection.count")
            .with_description("SQLx Postgres pool connections currently open")
            .build();
        let idle_count = meter
            .u64_gauge("db.client.connection.idle")
            .with_description("SQLx Postgres pool idle connections")
            .build();
        let max_count = meter
            .u64_gauge("db.client.connection.max")
            .with_description("SQLx Postgres pool configured max connections")
            .build();
        let pending_requests = meter
            .u64_gauge("db.client.connection.pending_requests")
            .with_description("Application tasks waiting on SQLx pool acquire")
            .build();
        let timeout_count = meter
            .u64_gauge("db.client.connection.timeouts")
            .with_description("Cumulative SQLx Postgres pool acquire timeouts")
            .build();

        let mut ticker = tokio::time::interval(Duration::from_secs(5));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            connection_count.record(u64::from(db.pool.size()), &[]);
            idle_count.record(db.pool.num_idle() as u64, &[]);
            max_count.record(u64::from(DB_MAX_CONNECTIONS), &[]);
            pending_requests.record(db.pending.load(Ordering::Relaxed), &[]);
            timeout_count.record(db.timeouts.load(Ordering::Relaxed), &[]);
        }
    });
}

fn inventory_no_db_enabled() -> bool {
    env_flag(std::env::var("INVENTORY_NO_DB").ok().as_deref())
}

fn env_flag(value: Option<&str>) -> bool {
    value
        .map(str::trim)
        .map(|value| matches!(value, "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let telemetry = playground_telemetry::init("inventory")?;
    let state = AppState {
        db: init_db().await?,
    };
    let app = Router::new()
        .route("/reserve", get(reserve))
        .route("/healthz", get(|| async { "ok" }))
        .with_state(state);
    let addr = std::env::var("ADDR").unwrap_or_else(|_| "0.0.0.0:8089".into());
    tracing::info!(%addr, "inventory HTTP listening");
    axum::serve(tokio::net::TcpListener::bind(&addr).await?, app)
        .with_graceful_shutdown(playground_telemetry::shutdown_signal())
        .await?;
    telemetry.shutdown();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inventory_no_db_flag_accepts_explicit_truthy_values() {
        assert!(env_flag(Some("1")));
        assert!(env_flag(Some("true")));
        assert!(env_flag(Some("yes")));
        assert!(env_flag(Some("on")));
    }

    #[test]
    fn inventory_no_db_flag_rejects_absent_or_falsey_values() {
        assert!(!env_flag(None));
        assert!(!env_flag(Some("")));
        assert!(!env_flag(Some("0")));
        assert!(!env_flag(Some("false")));
    }
}
