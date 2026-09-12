use crate::domain::{CachedLine, CachedQuote, QUOTE_TTL_SECONDS, quote_expiry_from, request_id};
use anyhow::Context;
use deadpool_postgres::{
    Config, GenericClient, ManagerConfig, Pool, PoolConfig, RecyclingMethod, Runtime,
};
use playground_proto::pricing::v1::QuoteRequest;
use redis::{AsyncCommands, aio::ConnectionManager};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime},
};
use tokio::sync::Mutex;
use tokio_postgres::IsolationLevel;
use tonic::Status;

const DEFAULT_DATABASE_URL: &str = "postgres://postgres:playground@postgres:5432/playground";
const DEFAULT_REDIS_URL: &str = "redis://redis:6379/0";
const DB_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone)]
pub struct AppState {
    pub(crate) postgres: PostgresRepository,
    pub(crate) redis: RedisRepository,
}

impl AppState {
    /// Returns whether the pricing database accepts a bounded probe query.
    pub async fn database_ready(&self) -> bool {
        self.postgres.is_ready().await
    }

    pub fn redis_available(&self) -> bool {
        self.redis.is_available()
    }
}

#[derive(Clone)]
pub(crate) struct PostgresRepository {
    pool: Pool,
}

impl PostgresRepository {
    pub(crate) async fn pricing_version(&self, tenant_id: &str) -> Result<String, Status> {
        let mut client = tokio::time::timeout(DB_TIMEOUT, self.pool.get())
            .await
            .map_err(|_| Status::deadline_exceeded("pricing database pool timeout"))?
            .map_err(|error| {
                Status::unavailable(format!("pricing database unavailable: {error}"))
            })?;
        let transaction = client
            .build_transaction()
            .isolation_level(IsolationLevel::RepeatableRead)
            .start()
            .await
            .map_err(|error| {
                Status::unavailable(format!("pricing transaction start failed: {error}"))
            })?;
        let version = pricing_version_in(&transaction, tenant_id).await?;
        transaction.commit().await.map_err(|error| {
            Status::unavailable(format!("pricing version transaction failed: {error}"))
        })?;
        Ok(version)
    }

    async fn is_ready(&self) -> bool {
        let Ok(client) = tokio::time::timeout(DB_TIMEOUT, self.pool.get()).await else {
            return false;
        };
        let Ok(client) = client else {
            return false;
        };
        client.query_one("SELECT 1", &[]).await.is_ok()
    }

    pub(crate) async fn calculate_quote(
        &self,
        request: &QuoteRequest,
    ) -> Result<CachedQuote, Status> {
        let mut client = tokio::time::timeout(DB_TIMEOUT, self.pool.get())
            .await
            .map_err(|_| Status::deadline_exceeded("pricing database pool timeout"))?
            .map_err(|error| {
                Status::unavailable(format!("pricing database unavailable: {error}"))
            })?;
        let transaction = client
            .build_transaction()
            .isolation_level(IsolationLevel::RepeatableRead)
            .start()
            .await
            .map_err(|error| {
                Status::unavailable(format!("pricing transaction start failed: {error}"))
            })?;
        let pricing_version = pricing_version_in(&transaction, &request.tenant_id).await?;
        let promotion_code = request
            .context
            .get("promotion_code")
            .map(String::as_str)
            .unwrap_or("");
        let strategy = request
            .context
            .get("pricing_strategy")
            .map(String::as_str)
            .unwrap_or("standard");
        let skus = request
            .items
            .iter()
            .map(|item| item.sku.clone())
            .collect::<Vec<_>>();
        let selected_rows = transaction
            .query(
                "SELECT DISTINCT ON (v.sku)
                    v.sku,
                    product.id,
                    p.amount_minor,
                    p.currency,
                    'default'
                 FROM unnest($1::text[]) AS requested(sku)
                 JOIN product_variants v
                   ON v.tenant_id = $2
                  AND v.sku = requested.sku
                  AND v.status = 'active'
                 JOIN products product
                   ON product.tenant_id = v.tenant_id
                  AND product.id = v.product_id
                  AND product.status = 'active'
                 JOIN customers customer
                   ON customer.tenant_id = v.tenant_id
                  AND customer.id = $3
                  AND customer.status = 'active'
                 JOIN prices p
                   ON p.tenant_id = v.tenant_id
                  AND p.variant_id = v.id
                  AND p.currency = $4
                  AND p.valid_from <= CURRENT_TIMESTAMP
                  AND (p.valid_to IS NULL OR p.valid_to > CURRENT_TIMESTAMP)
                  AND p.is_default
                 WHERE v.tenant_id = $2
                 ORDER BY
                    v.sku,
                    p.valid_from DESC,
                    p.id",
                &[
                    &skus,
                    &request.tenant_id,
                    &request.customer_id,
                    &request.currency_code,
                ],
            )
            .await
            .map_err(|error| Status::unavailable(format!("pricing batch query failed: {error}")))?;
        let selected_prices = selected_rows
            .into_iter()
            .map(|row| {
                (
                    row.get::<_, String>(0),
                    SelectedPrice {
                        product_id: row.get(1),
                        unit_minor: i64::from(row.get::<_, i32>(2)),
                        currency: row.get(3),
                        price_source: row.get(4),
                    },
                )
            })
            .collect::<HashMap<_, _>>();
        let mut lines = Vec::with_capacity(request.items.len());
        let mut selected_price_source = None;
        for item in &request.items {
            let Some(price) = selected_prices.get(&item.sku) else {
                return Err(Status::not_found(format!(
                    "unknown priced SKU: {}",
                    item.sku
                )));
            };
            let unit_minor = price.unit_minor;
            if unit_minor < 0 {
                return Err(Status::internal("pricing source returned a negative price"));
            }
            if price.currency != request.currency_code {
                return Err(Status::invalid_argument(
                    "currency is not supported for the SKU",
                ));
            }
            let line_minor = unit_minor
                .checked_mul(i64::from(item.quantity))
                .ok_or_else(|| Status::out_of_range("quote line amount overflowed"))?;
            lines.push(CachedLine {
                product_id: price.product_id.clone(),
                sku: item.sku.clone(),
                quantity: item.quantity,
                unit_minor,
                line_minor,
            });
            if selected_price_source.is_none() {
                selected_price_source = Some(price.price_source.clone());
            }
        }
        let subtotal_minor = lines.iter().try_fold(0_i64, |subtotal, line| {
            subtotal
                .checked_add(line.line_minor)
                .ok_or_else(|| Status::out_of_range("quote subtotal overflowed"))
        })?;
        let discount_minor = if strategy == "promotional" && !promotion_code.is_empty() {
            promotion_discount(
                &transaction,
                request,
                promotion_code,
                &lines,
                subtotal_minor,
            )
            .await?
        } else {
            0
        };
        let grand_total_minor = subtotal_minor
            .checked_sub(discount_minor)
            .ok_or_else(|| Status::internal("quote total arithmetic underflowed"))?;
        let quote_id = request_id(request);
        let expires_at_unix_ms = quote_expiry_from(SystemTime::now())?;
        let quote = CachedQuote {
            quote_id,
            lines,
            subtotal_minor,
            discount_minor,
            grand_total_minor,
            currency: request.currency_code.clone(),
            pricing_version,
            price_source: selected_price_source
                .ok_or_else(|| Status::internal("pricing batch query returned no price source"))?,
            expires_at_unix_ms,
        };
        tracing::info!(
            pricing_strategy = strategy,
            promotion_code = %promotion_code,
            price_source = %quote.price_source,
            subtotal_minor,
            discount_minor,
            "pricing rules applied"
        );
        transaction.commit().await.map_err(|error| {
            Status::unavailable(format!("pricing quote transaction failed: {error}"))
        })?;
        Ok(quote)
    }
}

#[derive(Debug)]
struct SelectedPrice {
    product_id: String,
    unit_minor: i64,
    currency: String,
    price_source: String,
}

async fn pricing_version_in<C>(client: &C, tenant_id: &str) -> Result<String, Status>
where
    C: GenericClient + Sync,
{
    let row = client
        .query_one(
            "SELECT COALESCE(MAX(sequence), 0)::BIGINT FROM price_change_events WHERE tenant_id = $1",
            &[&tenant_id],
        )
        .await
        .map_err(|error| Status::unavailable(format!("pricing version query failed: {error}")))?;
    let sequence: i64 = row.get(0);
    Ok(format!("price-change-{sequence}"))
}

async fn promotion_discount<C>(
    client: &C,
    request: &QuoteRequest,
    code: &str,
    lines: &[CachedLine],
    subtotal_minor: i64,
) -> Result<i64, Status>
where
    C: GenericClient + Sync,
{
    let row = client.query_opt(
            "SELECT id, discount_type, discount_value::double precision, currency FROM promotions WHERE tenant_id = $1 AND code = $2 AND active AND starts_at <= now() AND (ends_at IS NULL OR ends_at > now()) AND (max_redemptions IS NULL OR redemption_count < max_redemptions) AND (currency IS NULL OR currency = $3) AND minimum_subtotal_minor <= $4",
            &[&request.tenant_id, &code, &request.currency_code, &subtotal_minor],
        ).await.map_err(|error| Status::unavailable(format!("promotion query failed: {error}")))?;
    let Some(row) = row else {
        return Ok(0);
    };
    let promotion_id: String = row.get(0);
    let product_ids = client
        .query(
            "SELECT product_id FROM promotion_products WHERE tenant_id=$1 AND promotion_id=$2",
            &[&request.tenant_id, &promotion_id],
        )
        .await
        .map_err(|error| Status::unavailable(format!("promotion scope query failed: {error}")))?
        .into_iter()
        .map(|scope| scope.get::<_, String>(0))
        .collect::<HashSet<_>>();
    if product_ids.is_empty() {
        return Ok(0);
    }
    let eligible_subtotal = lines
        .iter()
        .filter(|line| product_ids.contains(&line.product_id))
        .try_fold(0_i64, |subtotal, line| {
            subtotal
                .checked_add(line.line_minor)
                .ok_or_else(|| Status::out_of_range("promotion subtotal overflowed"))
        })?;
    if eligible_subtotal == 0 {
        return Ok(0);
    }
    let kind: &str = row.get(1);
    let value: f64 = row.get(2);
    if !value.is_finite() || value <= 0.0 {
        return Err(Status::internal("promotion value is invalid"));
    }
    let raw_discount = if kind == "percentage" {
        if value > 100.0 {
            return Err(Status::internal("promotion percentage is invalid"));
        }
        eligible_subtotal as f64 * value / 100.0
    } else if kind == "fixed" {
        value
    } else {
        return Err(Status::internal("promotion type is invalid"));
    };
    if !raw_discount.is_finite() || raw_discount > i64::MAX as f64 {
        return Err(Status::out_of_range("promotion discount overflowed"));
    }
    Ok((raw_discount.round() as i64).min(eligible_subtotal))
}

#[derive(Clone)]
pub(crate) struct RedisRepository {
    client: Option<redis::Client>,
    state: Arc<Mutex<RedisConnectionState>>,
    available: Arc<AtomicBool>,
}

struct RedisConnectionState {
    manager: Option<ConnectionManager>,
    generation: u64,
}

impl RedisRepository {
    fn new(client: Option<redis::Client>, manager: Option<ConnectionManager>) -> Self {
        let available = manager.is_some();
        Self {
            client,
            state: Arc::new(Mutex::new(RedisConnectionState {
                manager,
                generation: u64::from(available),
            })),
            available: Arc::new(AtomicBool::new(available)),
        }
    }

    pub(crate) fn is_available(&self) -> bool {
        self.available.load(Ordering::Acquire)
    }

    pub(crate) async fn get(&self, key: &str) -> Option<Result<Option<String>, redis::RedisError>> {
        let (mut connection, generation) = self.connection().await?;
        let result = match tokio::time::timeout(
            DB_TIMEOUT,
            connection.get::<_, Option<String>>(key),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => Err(redis::RedisError::from((
                redis::ErrorKind::IoError,
                "pricing Redis get timed out",
            ))),
        };
        if result.is_err() {
            self.clear_failed(generation).await;
        }
        Some(result)
    }

    pub(crate) async fn set(
        &self,
        key: &str,
        quote: &CachedQuote,
    ) -> Option<Result<(), redis::RedisError>> {
        let value = serde_json::to_string(quote).ok()?;
        let (mut connection, generation) = self.connection().await?;
        let result = match tokio::time::timeout(
            DB_TIMEOUT,
            connection.set_ex::<_, _, ()>(key, value, QUOTE_TTL_SECONDS),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => Err(redis::RedisError::from((
                redis::ErrorKind::IoError,
                "pricing Redis set timed out",
            ))),
        };
        if result.is_err() {
            self.clear_failed(generation).await;
        }
        Some(result)
    }

    async fn connection(&self) -> Option<(ConnectionManager, u64)> {
        let client = self.client.clone()?;
        let mut state = match tokio::time::timeout(DB_TIMEOUT, self.state.lock()).await {
            Ok(state) => state,
            Err(_) => {
                tracing::warn!("pricing Redis state lock timed out; using postgres");
                return None;
            }
        };
        if let Some(manager) = state.manager.as_ref() {
            return Some((manager.clone(), state.generation));
        }

        let manager = match tokio::time::timeout(DB_TIMEOUT, client.get_connection_manager()).await
        {
            Ok(Ok(manager)) => manager,
            Ok(Err(error)) => {
                tracing::warn!(error = %error, "redis reconnect failed; using postgres");
                return None;
            }
            Err(_) => {
                tracing::warn!("redis reconnect timed out; using postgres");
                return None;
            }
        };
        state.generation = state.generation.wrapping_add(1);
        let generation = state.generation;
        state.manager = Some(manager.clone());
        self.available.store(true, Ordering::Release);
        Some((manager, generation))
    }

    async fn clear_failed(&self, generation: u64) {
        let Ok(mut state) = tokio::time::timeout(DB_TIMEOUT, self.state.lock()).await else {
            return;
        };
        if state.generation == generation {
            state.manager = None;
            state.generation = state.generation.wrapping_add(1);
            self.available.store(false, Ordering::Release);
        }
    }
}

fn postgres_pool(url: &str) -> anyhow::Result<Pool> {
    let mut config = Config::new();
    config.url = Some(url.to_owned());
    config.manager = Some(ManagerConfig {
        recycling_method: RecyclingMethod::Fast,
    });
    config.pool = Some(PoolConfig {
        max_size: 8,
        ..Default::default()
    });
    config
        .create_pool(Some(Runtime::Tokio1), tokio_postgres::NoTls)
        .context("create pricing postgres pool")
}

pub async fn init_state() -> anyhow::Result<AppState> {
    let url = std::env::var("DATABASE_URL").unwrap_or_else(|_| DEFAULT_DATABASE_URL.to_owned());
    let pool = postgres_pool(&url)?;
    let client = tokio::time::timeout(DB_TIMEOUT, pool.get())
        .await
        .context("pricing postgres timeout")??;
    client
        .query_one("SELECT count(*) FROM prices", &[])
        .await
        .context("pricing schema missing; run mise run infra:postgres_migrate")?;
    let redis_url = std::env::var("REDIS_URL").unwrap_or_else(|_| DEFAULT_REDIS_URL.to_owned());
    let redis = match redis::Client::open(redis_url) {
        Ok(client) => {
            let redis = RedisRepository::new(Some(client), None);
            let _ = redis.connection().await;
            redis
        }
        Err(error) => {
            tracing::warn!(error = %error, "invalid redis URL; pricing will use postgres");
            RedisRepository::new(None, None)
        }
    };
    Ok(AppState {
        postgres: PostgresRepository { pool },
        redis,
    })
}
