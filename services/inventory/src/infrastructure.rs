use anyhow::{Context, anyhow};
use deadpool_postgres::{Config, GenericClient, ManagerConfig, Pool, RecyclingMethod, Runtime};
use opentelemetry::global;
use opentelemetry::{KeyValue, metrics::Histogram};
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use tokio::sync::watch;
use tokio_postgres::NoTls;
use tracing::Instrument;

use crate::domain::{
    ConsumeBody, ConsumeOutcome, ReleaseBody, ReleaseOutcome, ReservationConflict, ReserveOutcome,
    ReserveQuery,
};

const DEFAULT_DATABASE_URL: &str = "postgres://postgres:playground@postgres:5432/playground";
const DB_MAX_CONNECTIONS: usize = 8;
const DB_WAIT_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_DB_N1: u32 = 50;
const MAX_HOLD_MS: u64 = 10_000;
const RESERVATION_HOLD: Duration = Duration::from_secs(15 * 60);
const REAPER_BATCH_SIZE: i64 = 100;
const REAPER_INTERVAL: Duration = Duration::from_secs(1);

async fn lock_current_checkout_fence<C>(
    db: &C,
    tenant_id: &str,
    request_id: &str,
    lease_token: &str,
) -> anyhow::Result<()>
where
    C: GenericClient + Sync,
{
    let current = db
        .query_opt(
            "SELECT a.status
             FROM checkout_attempts a
             WHERE a.tenant_id=$1
               AND a.request_id=$2
               AND a.lease_token=$3
               AND (
                   (a.status='started' AND a.lease_expires_at > CURRENT_TIMESTAMP)
                   OR a.status='failed'
                   OR (
                       a.status='pending'
                       AND EXISTS (
                           SELECT 1
                           FROM checkout_payment_reconciliations r
                           WHERE r.tenant_id=a.tenant_id
                             AND r.request_id=a.request_id
                             AND r.checkout_lease_token=a.lease_token
                             AND r.status='processing'
                             AND r.lease_token IS NOT NULL
                             AND r.lease_expires_at > CURRENT_TIMESTAMP
                       )
                   )
               )
             FOR UPDATE",
            &[&tenant_id, &request_id, &lease_token],
        )
        .await?;
    if current.is_some() {
        return Ok(());
    }
    Err(anyhow::Error::new(ReservationConflict {
        code: "checkout_lease_lost",
        message: "authoritative checkout fence is no longer current".to_owned(),
    }))
}

pub(crate) async fn reserve(
    pool: &Pool,
    params: &ReserveQuery,
) -> anyhow::Result<Option<ReserveOutcome>> {
    let started = Instant::now();
    let span = playground_telemetry::db_span(
        "UPDATE",
        "reserve inventory row",
        "INSERT inventory_reservations and UPDATE inventory by reservation_id",
    );
    let result = async {
        if params.hold_ms > 0 {
            let client = acquire(pool).await?;
            tracing::info!(
                hold_ms = params.hold_ms.min(MAX_HOLD_MS),
                "holding postgres connection"
            );
            tokio::time::sleep(Duration::from_millis(params.hold_ms.min(MAX_HOLD_MS))).await;
            drop(client);
        }
        if params.slow > 0 {
            let client = acquire(pool).await?;
            client
                .query_one(
                    "SELECT pg_sleep($1::double precision)",
                    &[&(params.slow.min(30_000) as f64 / 1000.0)],
                )
                .await?;
        }
        for _ in 0..params.db_n1.min(MAX_DB_N1) {
            let client = acquire(pool).await?;
            let _ = client
                .query_opt("SELECT i.available_quantity FROM inventory i JOIN product_variants v ON v.tenant_id = i.tenant_id AND v.id = i.variant_id WHERE i.tenant_id = $1 AND v.sku = $2", &[&params.tenant_id, &params.sku])
                .await?;
        }
        if params.fail {
            return Err(anyhow!("fault injection: reservation rejected"));
        }

        let mut client = acquire(pool).await?;
        let transaction = client.transaction().await?;
        let request_id = params.checkout_request_id.as_deref().ok_or_else(|| {
            anyhow::Error::new(ReservationConflict {
                code: "checkout_lease_lost",
                message: "authoritative checkout fence is required".to_owned(),
            })
        })?;
        let lease_token = params.checkout_lease_token.as_deref().ok_or_else(|| {
            anyhow::Error::new(ReservationConflict {
                code: "checkout_lease_lost",
                message: "authoritative checkout fence is required".to_owned(),
            })
        })?;
        // Lock the parent before any reservation/stock row. Lease reclaim uses
        // the same parent lock, so a stale generation cannot mutate inventory
        // after reclaim linearizes.
        lock_current_checkout_fence(&transaction, &params.tenant_id, request_id, lease_token)
            .await?;
        transaction
            .query_one(
                "SELECT pg_advisory_xact_lock(hashtextextended($1 || ':' || $2, 0))",
                &[&params.tenant_id, &params.reservation_id],
            )
            .await?;
        if let Some(row) = transaction
            .query_opt(
                "SELECT r.sku, r.quantity, r.variant_id, r.location_id, r.status, r.expires_at <= CURRENT_TIMESTAMP, r.owner_request_id, r.owner_lease_token, i.available_quantity FROM inventory_reservations r JOIN inventory i ON i.tenant_id = r.tenant_id AND i.variant_id = r.variant_id AND i.location_id = r.location_id WHERE r.tenant_id = $1 AND r.reservation_id = $2 FOR UPDATE OF r, i",
                &[&params.tenant_id, &params.reservation_id],
            )
            .await?
        {
            let stored_sku: String = row.get(0);
            let stored_quantity: i32 = row.get(1);
            if stored_sku != params.sku
                || stored_quantity != i32::try_from(params.quantity).unwrap_or(i32::MAX)
            {
                return Err(anyhow::Error::new(ReservationConflict {
                    code: "reservation_conflict",
                    message: "reservation identity does not match the reserve request".to_owned(),
                }));
            }
            let variant_id: String = row.get(2);
            let location_id: String = row.get(3);
            let status: String = row.get(4);
            let expired: bool = row.get(5);
            let owner_request_id: Option<String> = row.get(6);
            let owner_lease_token: Option<String> = row.get(7);
            let owner_matches = owner_request_id.as_deref() == Some(request_id);
            if !owner_matches {
                return Err(anyhow::Error::new(ReservationConflict {
                    code: "reservation_owner_conflict",
                    message: "reservation belongs to a different checkout request".to_owned(),
                }));
            }

            match status.as_str() {
                "released" => {
                    let row = transaction
                        .query_opt(
                            "UPDATE inventory SET reserved_quantity = reserved_quantity + $1, updated_at = CURRENT_TIMESTAMP WHERE tenant_id = $2 AND variant_id = $3 AND location_id = $4 AND available_quantity >= $1 RETURNING available_quantity",
                            &[
                                &stored_quantity,
                                &params.tenant_id,
                                &variant_id,
                                &location_id,
                            ],
                        )
                        .await?
                        .ok_or_else(|| anyhow!("inventory is no longer available for reservation reuse"))?;
                    let changed = transaction
                        .execute(
                            "UPDATE inventory_reservations SET status='reserved', released_at=NULL, consumed_at=NULL, expires_at=CURRENT_TIMESTAMP + ($5::double precision * INTERVAL '1 second'), owner_request_id=$3, owner_lease_token=$4, updated_at=CURRENT_TIMESTAMP WHERE tenant_id=$1 AND reservation_id=$2 AND status='released'",
                            &[
                                &params.tenant_id,
                                &params.reservation_id,
                                &request_id,
                                &lease_token,
                                &(RESERVATION_HOLD.as_secs() as f64),
                            ],
                        )
                        .await?;
                    if changed != 1 {
                        return Err(anyhow!("reservation state changed during reuse"));
                    }
                    let outcome = ReserveOutcome {
                        location_id,
                        remaining: row.get(0),
                        status: "reserved",
                    };
                    transaction.commit().await?;
                    return Ok(Some(outcome));
                }
                "consumed" if owner_matches => {
                    let outcome = ReserveOutcome {
                        location_id,
                        remaining: row.get::<_, Option<i32>>(8).unwrap_or_default(),
                        status: "already_consumed",
                    };
                    transaction.commit().await?;
                    return Ok(Some(outcome));
                }
                "consumed" => {
                    return Err(anyhow::Error::new(ReservationConflict {
                        code: "reservation_owner_conflict",
                        message: "consumed reservation belongs to a different checkout request".to_owned(),
                    }));
                }
                "reserved" => {
                    if expired || owner_lease_token.as_deref() != Some(lease_token) {
                        let changed = transaction
                            .execute(
                                "UPDATE inventory_reservations SET owner_lease_token=$3, expires_at=CURRENT_TIMESTAMP + ($4::double precision * INTERVAL '1 second'), updated_at=CURRENT_TIMESTAMP WHERE tenant_id=$1 AND reservation_id=$2 AND status='reserved' AND owner_request_id=$5",
                                &[
                                    &params.tenant_id,
                                    &params.reservation_id,
                                    &lease_token,
                                    &(RESERVATION_HOLD.as_secs() as f64),
                                    &request_id,
                                ],
                            )
                            .await?;
                        if changed != 1 {
                            return Err(anyhow!("reservation state changed during renewal"));
                        }
                    }
                    let outcome = ReserveOutcome {
                        location_id,
                        remaining: row.get::<_, Option<i32>>(8).unwrap_or_default(),
                        status: "already_reserved",
                    };
                    transaction.commit().await?;
                    return Ok(Some(outcome));
                }
                _ => {
                    return Err(anyhow!("unknown inventory reservation status: {status}"));
                }
            }
        }
        let row = transaction
            .query_opt(
                "WITH candidate AS (SELECT i.id, i.variant_id, i.location_id FROM inventory i JOIN product_variants v ON v.tenant_id = i.tenant_id AND v.id = i.variant_id JOIN inventory_locations l ON l.tenant_id = i.tenant_id AND l.id = i.location_id WHERE i.tenant_id = $1 AND v.sku = $2 AND v.status = 'active' AND l.is_active AND i.available_quantity >= $3 ORDER BY l.code, i.id LIMIT 1 FOR UPDATE) UPDATE inventory i SET reserved_quantity = i.reserved_quantity + $3, updated_at = CURRENT_TIMESTAMP FROM candidate c WHERE i.id = c.id AND i.available_quantity >= $3 RETURNING c.variant_id, c.location_id, i.available_quantity",
                &[&params.tenant_id, &params.sku, &(params.quantity as i32)],
            )
            .await?;
        let Some(row) = row else {
            transaction.commit().await?;
            return Ok(None);
        };
        let variant_id: String = row.get(0);
        let location_id: String = row.get(1);
        let remaining: i32 = row.get(2);
        let _ = transaction
            .execute(
                "INSERT INTO inventory_reservations (reservation_id, tenant_id, sku, variant_id, location_id, quantity, status, expires_at, owner_request_id, owner_lease_token) VALUES ($1, $2, $3, $4, $5, $6, 'reserved', CURRENT_TIMESTAMP + ($7::double precision * INTERVAL '1 second'), $8, $9)",
                &[
                    &params.reservation_id,
                    &params.tenant_id,
                    &params.sku,
                    &variant_id,
                    &location_id,
                    &(params.quantity as i32),
                    &(RESERVATION_HOLD.as_secs() as f64),
                    &params.checkout_request_id,
                    &params.checkout_lease_token,
                ],
            )
            .await?;
        transaction.commit().await?;
        Ok(Some(ReserveOutcome {
            location_id,
            remaining,
            status: "reserved",
        }))
    }
    .instrument(span)
    .await;
    record_db_duration(started);
    result
}

pub(crate) async fn release(
    pool: &Pool,
    body: &ReleaseBody,
) -> anyhow::Result<Option<ReleaseOutcome>> {
    let started = Instant::now();
    let span = playground_telemetry::db_span(
        "UPDATE",
        "release inventory reservation",
        "UPDATE inventory_reservations and inventory by tenant_id and reservation_id",
    );
    let result = async {
        let mut client = acquire(pool).await?;
        let transaction = client.transaction().await?;
        let request_id = body.checkout_request_id.as_deref().ok_or_else(|| {
            anyhow::Error::new(ReservationConflict {
                code: "checkout_lease_lost",
                message: "authoritative checkout fence is required".to_owned(),
            })
        })?;
        let lease_token = body.checkout_lease_token.as_deref().ok_or_else(|| {
            anyhow::Error::new(ReservationConflict {
                code: "checkout_lease_lost",
                message: "authoritative checkout fence is required".to_owned(),
            })
        })?;
        lock_current_checkout_fence(&transaction, &body.tenant_id, request_id, lease_token)
            .await?;
        transaction
            .query_one(
                "SELECT pg_advisory_xact_lock(hashtextextended($1 || ':' || $2, 0))",
                &[&body.tenant_id, &body.reservation_id],
            )
            .await?;
        let row = transaction
            .query_opt(
                "SELECT r.sku, r.variant_id, r.location_id, r.quantity, r.status, i.reserved_quantity, i.available_quantity, r.owner_request_id, r.owner_lease_token FROM inventory_reservations r JOIN inventory i ON i.tenant_id = r.tenant_id AND i.variant_id = r.variant_id AND i.location_id = r.location_id WHERE r.tenant_id = $1 AND r.reservation_id = $2 FOR UPDATE OF r, i",
                &[&body.tenant_id, &body.reservation_id],
            )
            .await?;
        let Some(row) = row else {
            return Ok(None);
        };

        let stored_sku: String = row.get(0);
        let variant_id: String = row.get(1);
        let location_id: String = row.get(2);
        let stored_quantity: i32 = row.get(3);
        let status: String = row.get(4);
        if stored_sku != body.sku
            || stored_quantity != i32::try_from(body.quantity).unwrap_or(i32::MAX)
            || body
                .location_id
                .as_deref()
                .is_some_and(|requested| requested != location_id)
        {
            return Err(anyhow::Error::new(ReservationConflict {
                code: "reservation_conflict",
                message: "reservation identity does not match the release request".to_owned(),
            }));
        }

        let owner_request_id: Option<String> = row.get(7);
        let owner_lease_token: Option<String> = row.get(8);
        if owner_request_id.as_deref() != Some(request_id) {
            return Err(anyhow::Error::new(ReservationConflict {
                code: "reservation_owner_conflict",
                message: "checkout fence does not own the reservation".to_owned(),
            }));
        }

        if status == "reserved" && owner_lease_token.as_deref() != Some(lease_token) {
            let rebound = transaction
                .execute(
                    "UPDATE inventory_reservations SET owner_lease_token=$3, updated_at=CURRENT_TIMESTAMP WHERE tenant_id=$1 AND reservation_id=$2 AND status='reserved' AND owner_request_id=$4",
                    &[&body.tenant_id, &body.reservation_id, &lease_token, &request_id],
                )
                .await?;
            if rebound != 1 {
                return Err(anyhow!("reservation owner changed during release"));
            }
        }

        if status == "released" {
            let outcome = ReleaseOutcome {
                location_id,
                released: 0,
                reserved_remaining: row.get(5),
                available: row.get(6),
                status: "already_released",
            };
            transaction.commit().await?;
            return Ok(Some(outcome));
        }

        if status == "consumed" {
            return Err(anyhow::Error::new(ReservationConflict {
                code: "reservation_consumed",
                message: "a consumed reservation cannot be released".to_owned(),
            }));
        }

        let released_row = transaction
            .query_opt(
                "UPDATE inventory SET reserved_quantity = reserved_quantity - $1, updated_at = CURRENT_TIMESTAMP WHERE tenant_id = $2 AND variant_id = $3 AND location_id = $4 AND reserved_quantity >= $1 RETURNING reserved_quantity, available_quantity",
                &[&stored_quantity, &body.tenant_id, &variant_id, &location_id],
            )
            .await?
            .ok_or_else(|| anyhow!("inventory reservation ledger is inconsistent"))?;
        let updated = transaction
            .execute(
                "UPDATE inventory_reservations SET status = 'released', released_at = CURRENT_TIMESTAMP, consumed_at = NULL, updated_at = CURRENT_TIMESTAMP WHERE tenant_id = $1 AND reservation_id = $2 AND status = 'reserved' AND owner_request_id = $3",
                &[&body.tenant_id, &body.reservation_id, &request_id],
            )
            .await?;
        if updated != 1 {
            return Err(anyhow!("reservation state changed during release"));
        }
        let outcome = ReleaseOutcome {
            location_id,
            released: stored_quantity,
            reserved_remaining: released_row.get(0),
            available: released_row.get(1),
            status: "released",
        };
        transaction.commit().await?;
        Ok(Some(outcome))
    }
    .instrument(span)
    .await;
    record_db_duration(started);
    result
}

pub(crate) async fn consume(pool: &Pool, body: &ConsumeBody) -> anyhow::Result<ConsumeOutcome> {
    let started = Instant::now();
    let span = playground_telemetry::db_span(
        "UPDATE",
        "consume inventory reservations",
        "atomically consume all checkout reservations and decrement on-hand stock",
    );
    let result = async {
        let mut reservations = body.reservations.clone();
        reservations.sort_by(|left, right| left.reservation_id.cmp(&right.reservation_id));
        let mut client = acquire(pool).await?;
        let transaction = client.transaction().await?;
        let request_id = body.checkout_request_id.as_deref().ok_or_else(|| {
            anyhow::Error::new(ReservationConflict {
                code: "checkout_lease_lost",
                message: "authoritative checkout fence is required".to_owned(),
            })
        })?;
        let lease_token = body.checkout_lease_token.as_deref().ok_or_else(|| {
            anyhow::Error::new(ReservationConflict {
                code: "checkout_lease_lost",
                message: "authoritative checkout fence is required".to_owned(),
            })
        })?;
        lock_current_checkout_fence(&transaction, &body.tenant_id, request_id, lease_token)
            .await?;
        for reservation in &reservations {
            transaction
                .query_one(
                    "SELECT pg_advisory_xact_lock(hashtextextended($1 || ':' || $2, 0))",
                    &[&body.tenant_id, &reservation.reservation_id],
                )
                .await?;
        }

        let mut consumed = 0_u32;
        for reservation in &reservations {
            let row = transaction
                .query_opt(
                    "SELECT r.sku, r.variant_id, r.location_id, r.quantity, r.status, r.expires_at <= CURRENT_TIMESTAMP, r.owner_request_id, r.owner_lease_token, i.reserved_quantity, i.on_hand_quantity FROM inventory_reservations r JOIN inventory i ON i.tenant_id = r.tenant_id AND i.variant_id = r.variant_id AND i.location_id = r.location_id WHERE r.tenant_id = $1 AND r.reservation_id = $2 FOR UPDATE OF r, i",
                    &[&body.tenant_id, &reservation.reservation_id],
                )
                .await?
                .ok_or_else(|| {
                    anyhow::Error::new(ReservationConflict {
                        code: "reservation_not_found",
                        message: "reservation does not exist for the tenant".to_owned(),
                    })
                })?;
            let stored_sku: String = row.get(0);
            let variant_id: String = row.get(1);
            let location_id: String = row.get(2);
            let stored_quantity: i32 = row.get(3);
            let status: String = row.get(4);
            let expired: bool = row.get(5);
            let owner_request_id: Option<String> = row.get(6);
            let owner_lease_token: Option<String> = row.get(7);
            if stored_sku != reservation.sku
                || stored_quantity != i32::try_from(reservation.quantity).unwrap_or(i32::MAX)
                || reservation
                    .location_id
                    .as_deref()
                    .is_some_and(|requested| requested != location_id)
            {
                return Err(anyhow::Error::new(ReservationConflict {
                    code: "reservation_conflict",
                    message: "reservation identity does not match the consume request".to_owned(),
                }));
            }
            if owner_request_id.as_deref() != Some(request_id) {
                return Err(anyhow::Error::new(ReservationConflict {
                    code: "reservation_owner_conflict",
                    message: "checkout fence does not own the reservation".to_owned(),
                }));
            }
            match status.as_str() {
                // A newer reconciliation lease may safely observe the
                // idempotent terminal state created by an older lease. It
                // cannot alter the consumed row or stock ledger.
                "consumed"
                    if owner_request_id.as_deref() == Some(request_id) =>
                {
                    continue;
                }
                "consumed" => {
                    return Err(anyhow::Error::new(ReservationConflict {
                        code: "reservation_owner_conflict",
                        message: "checkout fence does not own the consumed reservation".to_owned(),
                    }));
                }
                "released" => {
                    // Reclaim an expired/reaped hold only for the same
                    // checkout request, while the authoritative parent fence
                    // is locked above. This preserves idempotency without
                    // allowing another request to steal released stock.
                    let reacquired = transaction
                        .query_opt(
                            "UPDATE inventory SET reserved_quantity = reserved_quantity + $1, updated_at = CURRENT_TIMESTAMP WHERE tenant_id=$2 AND variant_id=$3 AND location_id=$4 AND available_quantity >= $1 RETURNING 1",
                            &[&stored_quantity, &body.tenant_id, &variant_id, &location_id],
                        )
                        .await?;
                    if reacquired.is_none() {
                        return Err(anyhow!("inventory is no longer available for reservation recovery"));
                    }
                    let changed = transaction
                        .execute(
                            "UPDATE inventory_reservations SET status='reserved', released_at=NULL, consumed_at=NULL, expires_at=CURRENT_TIMESTAMP + ($4::double precision * INTERVAL '1 second'), owner_lease_token=$3, updated_at=CURRENT_TIMESTAMP WHERE tenant_id=$1 AND reservation_id=$2 AND status='released' AND owner_request_id=$5",
                            &[
                                &body.tenant_id,
                                &reservation.reservation_id,
                                &lease_token,
                                &(RESERVATION_HOLD.as_secs() as f64),
                                &request_id,
                            ],
                        )
                        .await?;
                    if changed != 1 {
                        return Err(anyhow!("reservation state changed during recovery"));
                    }
                }
                "reserved" if expired => {
                    let changed = transaction
                        .execute(
                            "UPDATE inventory_reservations SET expires_at=CURRENT_TIMESTAMP + ($3::double precision * INTERVAL '1 second'), owner_lease_token=$4, updated_at=CURRENT_TIMESTAMP WHERE tenant_id=$1 AND reservation_id=$2 AND status='reserved' AND owner_request_id=$5",
                            &[
                                &body.tenant_id,
                                &reservation.reservation_id,
                                &(RESERVATION_HOLD.as_secs() as f64),
                                &lease_token,
                                &request_id,
                            ],
                        )
                        .await?;
                    if changed != 1 {
                        return Err(anyhow!("reservation state changed during renewal"));
                    }
                }
                "reserved" => {
                    if owner_lease_token.as_deref() != Some(lease_token) {
                        let changed = transaction
                            .execute(
                                "UPDATE inventory_reservations SET owner_lease_token=$3, updated_at=CURRENT_TIMESTAMP WHERE tenant_id=$1 AND reservation_id=$2 AND status='reserved' AND owner_request_id=$4",
                                &[
                                    &body.tenant_id,
                                    &reservation.reservation_id,
                                    &lease_token,
                                    &request_id,
                                ],
                            )
                            .await?;
                        if changed != 1 {
                            return Err(anyhow!("reservation owner changed during consumption"));
                        }
                    }
                }
                _ => return Err(anyhow!("unknown inventory reservation status: {status}")),
            }

            let quantity = stored_quantity;
            let updated = transaction
                .execute(
                    "UPDATE inventory SET on_hand_quantity = on_hand_quantity - $1, reserved_quantity = reserved_quantity - $1, updated_at = CURRENT_TIMESTAMP WHERE tenant_id = $2 AND variant_id = $3 AND location_id = $4 AND on_hand_quantity >= $1 AND reserved_quantity >= $1",
                    &[&quantity, &body.tenant_id, &variant_id, &location_id],
                )
                .await?;
            if updated != 1 {
                return Err(anyhow!("inventory reservation ledger is inconsistent"));
            }
            let updated = transaction
                .execute(
                    "UPDATE inventory_reservations SET status='consumed', released_at=NULL, consumed_at=CURRENT_TIMESTAMP, updated_at=CURRENT_TIMESTAMP WHERE tenant_id=$1 AND reservation_id=$2 AND status='reserved' AND owner_request_id=$3",
                    &[&body.tenant_id, &reservation.reservation_id, &request_id],
                )
                .await?;
            if updated != 1 {
                return Err(anyhow!("inventory reservation state changed during consumption"));
            }
            consumed = consumed
                .checked_add(u32::try_from(quantity).map_err(|_| anyhow!("quantity overflow"))?)
                .ok_or_else(|| anyhow!("consumed quantity overflow"))?;
        }
        transaction.commit().await?;
        let status = if consumed == 0 {
            "already_consumed"
        } else {
            "consumed"
        };
        Ok(ConsumeOutcome { consumed, status })
    }
    .instrument(span)
    .await;
    record_db_duration(started);
    result
}

pub(crate) async fn reap_expired_reservations(pool: &Pool) -> anyhow::Result<u32> {
    let mut client = acquire(pool).await?;
    let transaction = client.transaction().await?;
    let rows = transaction
        .query(
            "SELECT reservation_id, tenant_id, variant_id, location_id, quantity FROM inventory_reservations WHERE status='reserved' AND expires_at <= CURRENT_TIMESTAMP ORDER BY expires_at, tenant_id, reservation_id FOR UPDATE SKIP LOCKED LIMIT $1",
            &[&REAPER_BATCH_SIZE],
        )
        .await?;
    let mut released = 0_u32;
    for row in rows {
        let reservation_id: String = row.get(0);
        let tenant_id: String = row.get(1);
        let variant_id: String = row.get(2);
        let location_id: String = row.get(3);
        let quantity: i32 = row.get(4);
        let updated = transaction
            .execute(
                "UPDATE inventory SET reserved_quantity = reserved_quantity - $1, updated_at = CURRENT_TIMESTAMP WHERE tenant_id=$2 AND variant_id=$3 AND location_id=$4 AND reserved_quantity >= $1",
                &[&quantity, &tenant_id, &variant_id, &location_id],
            )
            .await?;
        if updated != 1 {
            return Err(anyhow!(
                "expired reservation {tenant_id}/{reservation_id} has inconsistent inventory"
            ));
        }
        let updated = transaction
            .execute(
                "UPDATE inventory_reservations SET status='released', released_at=CURRENT_TIMESTAMP, consumed_at=NULL, updated_at=CURRENT_TIMESTAMP WHERE tenant_id=$1 AND reservation_id=$2 AND status='reserved' AND expires_at <= CURRENT_TIMESTAMP",
                &[&tenant_id, &reservation_id],
            )
            .await?;
        if updated != 1 {
            return Err(anyhow!(
                "expired reservation {tenant_id}/{reservation_id} changed during reclamation"
            ));
        }
        released = released
            .checked_add(1)
            .ok_or_else(|| anyhow!("expired reservation count overflow"))?;
    }
    transaction.commit().await?;
    Ok(released)
}

pub fn spawn_expired_reservation_reclaimer(
    pool: Pool,
    mut shutdown: watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            if *shutdown.borrow() {
                break;
            }
            match reap_expired_reservations(&pool).await {
                Ok(released) if released > 0 => {
                    tracing::info!(released, "expired inventory reservations reclaimed");
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::error!(error = %error, "expired inventory reservation reclaimer failed");
                }
            }
            if wait_for_reaper_or_shutdown(&mut shutdown).await {
                break;
            }
        }
    })
}

async fn wait_for_reaper_or_shutdown(shutdown: &mut watch::Receiver<bool>) -> bool {
    if *shutdown.borrow() {
        return true;
    }
    tokio::select! {
        changed = shutdown.changed() => changed.is_err() || *shutdown.borrow(),
        _ = tokio::time::sleep(REAPER_INTERVAL) => false,
    }
}

pub(crate) async fn health_check(pool: &Pool) -> Result<(), String> {
    match tokio::time::timeout(DB_WAIT_TIMEOUT, pool.get()).await {
        Ok(Ok(client)) => match client.query_one("SELECT 1", &[]).await {
            Ok(_) => Ok(()),
            Err(error) => Err(error.to_string()),
        },
        Ok(Err(error)) => Err(error.to_string()),
        Err(_) => Err("database pool timeout".to_owned()),
    }
}

async fn acquire(pool: &Pool) -> Result<deadpool_postgres::Client, anyhow::Error> {
    let started = Instant::now();
    let result = tokio::time::timeout(DB_WAIT_TIMEOUT, pool.get())
        .await
        .map_err(|_| {
            anyhow!(
                "postgres pool wait exceeded {} ms",
                DB_WAIT_TIMEOUT.as_millis()
            )
        })?
        .map_err(|error| anyhow!(error))?;
    let waited_ms = started.elapsed().as_secs_f64() * 1000.0;
    global::meter("playground.inventory")
        .f64_histogram("db.client.connection.wait_time")
        .with_unit("ms")
        .with_description("Postgres pool wait before inventory work")
        .build()
        .record(
            waited_ms,
            &[
                KeyValue::new("db.system.name", "postgresql"),
                KeyValue::new("db.namespace", "playground"),
            ],
        );
    Ok(result)
}

fn record_db_duration(started: Instant) {
    static HISTOGRAM: OnceLock<Histogram<f64>> = OnceLock::new();
    HISTOGRAM
        .get_or_init(|| {
            global::meter("playground.inventory")
                .f64_histogram("db.client.operation.duration")
                .with_unit("ms")
                .with_description("Inventory PostgreSQL operation duration")
                .build()
        })
        .record(
            started.elapsed().as_secs_f64() * 1000.0,
            &[
                KeyValue::new("db.system.name", "postgresql"),
                KeyValue::new("db.namespace", "playground"),
            ],
        );
}

fn postgres_pool(database_url: &str) -> anyhow::Result<Pool> {
    let mut config = Config::new();
    config.url = Some(database_url.to_owned());
    config.manager = Some(ManagerConfig {
        recycling_method: RecyclingMethod::Fast,
    });
    config.pool = Some(deadpool_postgres::PoolConfig {
        max_size: DB_MAX_CONNECTIONS,
        timeouts: deadpool_postgres::Timeouts {
            wait: Some(DB_WAIT_TIMEOUT),
            create: Some(DB_WAIT_TIMEOUT),
            recycle: Some(DB_WAIT_TIMEOUT),
        },
        ..Default::default()
    });
    config
        .create_pool(Some(Runtime::Tokio1), NoTls)
        .context("create tokio-postgres pool")
}

pub async fn init_db() -> anyhow::Result<Pool> {
    let url = std::env::var("DATABASE_URL").unwrap_or_else(|_| DEFAULT_DATABASE_URL.to_owned());
    let pool = postgres_pool(&url)?;
    let client = tokio::time::timeout(DB_WAIT_TIMEOUT, pool.get())
        .await
        .context("inventory postgres connection timeout")??;
    client
        .query_one("SELECT 1", &[])
        .await
        .context("inventory postgres readiness")?;
    client
        .query_one("SELECT count(*) FROM inventory", &[])
        .await
        .context("inventory schema missing; run deploy/postgres/migrate.sh")?;
    tracing::info!("inventory connected to required postgres store");
    Ok(pool)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{ConsumeReservation, ReleaseBody, ReserveQuery};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn postgres_pool_uses_the_rust_postgres_stack() {
        let pool = postgres_pool(DEFAULT_DATABASE_URL).expect("pool config");
        assert_eq!(pool.status().max_size, DB_MAX_CONNECTIONS);
    }

    #[tokio::test]
    async fn reservation_reclaimer_stops_before_polling_when_cancelled() {
        let pool = postgres_pool(DEFAULT_DATABASE_URL).expect("pool config");
        let (_shutdown_sender, shutdown_receiver) = watch::channel(true);
        let reclaimer = spawn_expired_reservation_reclaimer(pool, shutdown_receiver);

        tokio::time::timeout(Duration::from_secs(1), reclaimer)
            .await
            .expect("cancelled reclaimer joins")
            .expect("reclaimer task joins");
    }

    #[tokio::test]
    async fn fenced_reserve_rejects_stale_token_and_preserves_current_release_consume()
    -> anyhow::Result<()> {
        let Ok(database_url) = std::env::var("INVENTORY_INTEGRATION_DATABASE_URL") else {
            eprintln!(
                "skipping fenced inventory integration test: INVENTORY_INTEGRATION_DATABASE_URL is unset"
            );
            return Ok(());
        };

        let pool = postgres_pool(&database_url).expect("inventory integration pool");
        let client = acquire(&pool)
            .await
            .expect("inventory integration connection");
        let tenant_id = "tenant-acme";
        let fixture = client
            .query_one(
                "SELECT v.sku, i.id, i.on_hand_quantity, i.reserved_quantity FROM inventory i JOIN product_variants v ON v.tenant_id=i.tenant_id AND v.id=i.variant_id JOIN inventory_locations l ON l.tenant_id=i.tenant_id AND l.id=i.location_id WHERE i.tenant_id=$1 AND v.status='active' AND l.is_active AND i.available_quantity >= 1 ORDER BY i.id LIMIT 1",
                &[&tenant_id],
            )
            .await
            .expect("inventory integration fixture");
        let sku: String = fixture.get(0);
        let inventory_id: String = fixture.get(1);
        let initial_on_hand: i32 = fixture.get(2);
        let initial_reserved: i32 = fixture.get(3);
        let unique_id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock is after unix epoch")
            .as_nanos();
        let request_id = format!("inventory-lease-test-{}-{unique_id}", std::process::id());
        let reservation_id = format!("{request_id}:reservation");
        let old_token = format!("{request_id}:old");
        let current_token = format!("{request_id}:current");

        client
            .execute(
                "INSERT INTO checkout_attempts (tenant_id, request_id, request_fingerprint, status, lease_token) VALUES ($1,$2,$3,'started',$4)",
                &[&tenant_id, &request_id, &request_id, &old_token],
            )
            .await
            .expect("create checkout lease fixture");

        fn reserve_request(
            tenant_id: &str,
            reservation_id: &str,
            sku: &str,
            request_id: &str,
            lease_token: &str,
        ) -> ReserveQuery {
            ReserveQuery {
                tenant_id: tenant_id.to_owned(),
                reservation_id: reservation_id.to_owned(),
                sku: sku.to_owned(),
                quantity: 1,
                slow: 0,
                db_n1: 0,
                hold_ms: 0,
                fail: false,
                checkout_request_id: Some(request_id.to_owned()),
                checkout_lease_token: Some(lease_token.to_owned()),
            }
        }

        let test_result: anyhow::Result<()> = async {
            let initial = reserve(
                &pool,
                &reserve_request(
                    tenant_id,
                    &reservation_id,
                    &sku,
                    &request_id,
                    &old_token,
                ),
            )
            .await?
            .ok_or_else(|| anyhow!("inventory fixture unexpectedly had no available stock"))?;
            if initial.status != "reserved" {
                return Err(anyhow!(
                    "initial fenced reserve returned unexpected status {}",
                    initial.status
                ));
            }

            let rotated = client
                .execute(
                    "UPDATE checkout_attempts SET lease_token=$3, updated_at=CURRENT_TIMESTAMP WHERE tenant_id=$1 AND request_id=$2 AND lease_token=$4 AND status='started'",
                    &[&tenant_id, &request_id, &current_token, &old_token],
                )
                .await?;
            if rotated != 1 {
                return Err(anyhow!("checkout lease fixture did not rotate"));
            }

            let stale = reserve(
                &pool,
                &reserve_request(
                    tenant_id,
                    &reservation_id,
                    &sku,
                    &request_id,
                    &old_token,
                ),
            )
            .await;
            let stale_error = stale
                .err()
                .ok_or_else(|| anyhow!("stale reserve unexpectedly succeeded"))?;
            let stale_conflict = stale_error
                .downcast_ref::<ReservationConflict>()
                .ok_or_else(|| anyhow!("stale reserve returned an unexpected error"))?;
            if stale_conflict.code != "checkout_lease_lost" {
                return Err(anyhow!(
                    "stale reserve returned unexpected conflict {}",
                    stale_conflict.code
                ));
            }

            let owner: Option<String> = client
                .query_one(
                    "SELECT owner_lease_token FROM inventory_reservations WHERE tenant_id=$1 AND reservation_id=$2",
                    &[&tenant_id, &reservation_id],
                )
                .await?
                .get(0);
            if owner.as_deref() != Some(old_token.as_str()) {
                return Err(anyhow!(
                    "stale reserve changed reservation owner from old token"
                ));
            }

            let current = reserve(
                &pool,
                &reserve_request(
                    tenant_id,
                    &reservation_id,
                    &sku,
                    &request_id,
                    &current_token,
                ),
            )
            .await?
            .ok_or_else(|| anyhow!("current fenced reserve unexpectedly had no result"))?;
            if current.status != "already_reserved" {
                return Err(anyhow!(
                    "current reserve returned unexpected status {}",
                    current.status
                ));
            }

            let owner: Option<String> = client
                .query_one(
                    "SELECT owner_lease_token FROM inventory_reservations WHERE tenant_id=$1 AND reservation_id=$2",
                    &[&tenant_id, &reservation_id],
                )
                .await?
                .get(0);
            if owner.as_deref() != Some(current_token.as_str()) {
                return Err(anyhow!(
                    "current reserve did not transfer reservation ownership"
                ));
            }

            let released = release(
                &pool,
                &ReleaseBody {
                    tenant_id: tenant_id.to_owned(),
                    reservation_id: reservation_id.clone(),
                    sku: sku.clone(),
                    quantity: 1,
                    location_id: Some(initial.location_id.clone()),
                    checkout_request_id: Some(request_id.clone()),
                    checkout_lease_token: Some(current_token.clone()),
                },
            )
            .await?
            .ok_or_else(|| anyhow!("current release did not find reservation"))?;
            if released.status != "released" || released.released != 1 {
                return Err(anyhow!("current release did not release one unit"));
            }

            let reserved_again = reserve(
                &pool,
                &reserve_request(
                    tenant_id,
                    &reservation_id,
                    &sku,
                    &request_id,
                    &current_token,
                ),
            )
            .await?
            .ok_or_else(|| anyhow!("current reserve after release unexpectedly failed"))?;
            if reserved_again.status != "reserved" {
                return Err(anyhow!(
                    "current reserve after release returned unexpected status {}",
                    reserved_again.status
                ));
            }

            let consumed = consume(
                &pool,
                &ConsumeBody {
                    tenant_id: tenant_id.to_owned(),
                    checkout_request_id: Some(request_id.clone()),
                    checkout_lease_token: Some(current_token.clone()),
                    reservations: vec![ConsumeReservation {
                        reservation_id: reservation_id.clone(),
                        sku: sku.clone(),
                        quantity: 1,
                        location_id: Some(initial.location_id),
                    }],
                },
            )
            .await?;
            if consumed.status != "consumed" || consumed.consumed != 1 {
                return Err(anyhow!("current consume did not consume one unit"));
            }

            let replay = consume(
                &pool,
                &ConsumeBody {
                    tenant_id: tenant_id.to_owned(),
                    checkout_request_id: Some(request_id.clone()),
                    checkout_lease_token: Some(current_token.clone()),
                    reservations: vec![ConsumeReservation {
                        reservation_id: reservation_id.clone(),
                        sku: sku.clone(),
                        quantity: 1,
                        location_id: None,
                    }],
                },
            )
            .await?;
            if replay.status != "already_consumed" || replay.consumed != 0 {
                return Err(anyhow!("consume replay was not idempotent"));
            }
            Ok(())
        }
        .await;

        let cleanup_result: anyhow::Result<()> = async {
            client
                .execute(
                    "DELETE FROM inventory_reservations WHERE tenant_id=$1 AND reservation_id=$2",
                    &[&tenant_id, &reservation_id],
                )
                .await?;
            client
                .execute(
                    "UPDATE inventory SET on_hand_quantity=$2, reserved_quantity=$3, updated_at=CURRENT_TIMESTAMP WHERE tenant_id=$1 AND id=$4",
                    &[&tenant_id, &initial_on_hand, &initial_reserved, &inventory_id],
                )
                .await?;
            client
                .execute(
                    "DELETE FROM checkout_attempts WHERE tenant_id=$1 AND request_id=$2",
                    &[&tenant_id, &request_id],
                )
                .await?;
            Ok(())
        }
        .await;

        match (test_result, cleanup_result) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(test_error), Ok(())) => Err(test_error),
            (Ok(()), Err(cleanup_error)) => Err(cleanup_error),
            (Err(test_error), Err(cleanup_error)) => Err(anyhow!(
                "inventory integration failed: {test_error}; cleanup failed: {cleanup_error}"
            )),
        }
    }
}
