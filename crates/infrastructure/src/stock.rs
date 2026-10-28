//! Trusted warehouse primitive; no preview/admin mutation endpoint is exposed.

use commerce_domain::inventory::{StockBalance, StockError, WarehouseChange};
use sqlx::PgPool;

#[derive(Debug)]
pub enum StockWriteError {
    InvalidRequest,
    InvalidStock(StockError),
    IdempotencyConflict,
    Database(sqlx::Error),
}

impl From<sqlx::Error> for StockWriteError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}
impl From<StockError> for StockWriteError {
    fn from(error: StockError) -> Self {
        Self::InvalidStock(error)
    }
}

pub struct WarehouseRequest<'a> {
    pub effect_key: &'a str,
    pub component_code: &'a str,
    pub change: WarehouseChange,
    pub actor: &'a str,
    pub reason: &'a str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MovementReceipt {
    pub balance: StockBalance,
    pub replayed: bool,
}

#[derive(sqlx::FromRow)]
struct BalanceRow {
    on_hand: i64,
    quarantine: i64,
    reserved: i64,
}

impl BalanceRow {
    fn balance(self) -> Result<StockBalance, StockError> {
        StockBalance::new(self.on_hand, self.quarantine, self.reserved)
    }
}

#[derive(sqlx::FromRow)]
struct StoredMovement {
    component_code: String,
    action: String,
    quantity: i64,
    actor: String,
    reason: String,
    on_hand_after: i64,
    quarantine_after: i64,
    reserved_after: i64,
}

fn bounded(value: &str, maximum: usize) -> bool {
    !value.trim().is_empty() && value.chars().count() <= maximum
}

pub async fn warehouse_movement(
    pool: &PgPool,
    request: WarehouseRequest<'_>,
) -> Result<MovementReceipt, StockWriteError> {
    if !bounded(request.effect_key, 128)
        || !bounded(request.actor, 128)
        || !bounded(request.reason, 500)
        || !bounded(request.component_code, 65)
    {
        return Err(StockWriteError::InvalidRequest);
    }
    let (action, quantity) = request.change.descriptor();
    let mut tx = pool.begin().await?;
    sqlx::query("SET LOCAL lock_timeout='1s'")
        .execute(&mut *tx)
        .await?;
    sqlx::query("SET LOCAL statement_timeout='2s'")
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO stock_balances (component_code) VALUES ($1) ON CONFLICT DO NOTHING")
        .bind(request.component_code)
        .execute(&mut *tx)
        .await?;
    let before = sqlx::query_as::<_, BalanceRow>(
        "SELECT on_hand,quarantine,reserved FROM stock_balances WHERE component_code=$1 FOR UPDATE",
    )
    .bind(request.component_code)
    .fetch_one(&mut *tx)
    .await?
    .balance()?;
    let existing = sqlx::query_as::<_,StoredMovement>("SELECT component_code,action,quantity,actor,reason,on_hand_after,quarantine_after,reserved_after FROM stock_movements WHERE effect_key=$1")
        .bind(request.effect_key).fetch_optional(&mut *tx).await?;
    if let Some(existing) = existing {
        if existing.component_code != request.component_code
            || existing.action != action
            || existing.quantity != quantity
            || existing.actor != request.actor
            || existing.reason != request.reason
        {
            return Err(StockWriteError::IdempotencyConflict);
        }
        let balance = StockBalance::new(
            existing.on_hand_after,
            existing.quarantine_after,
            existing.reserved_after,
        )?;
        tx.rollback().await?;
        return Ok(MovementReceipt {
            balance,
            replayed: true,
        });
    }
    let after = before.warehouse_change(request.change)?;
    sqlx::query(
        "UPDATE stock_balances SET on_hand=$2,quarantine=$3,reserved=$4 WHERE component_code=$1",
    )
    .bind(request.component_code)
    .bind(after.on_hand())
    .bind(after.quarantine())
    .bind(after.reserved())
    .execute(&mut *tx)
    .await?;
    let recorded = sqlx::query("INSERT INTO stock_movements (effect_key,component_code,action,quantity,actor,reason,on_hand_delta,quarantine_delta,reserved_delta,on_hand_after,quarantine_after,reserved_after) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,0,$9,$10,$11)")
        .bind(request.effect_key).bind(request.component_code).bind(action).bind(quantity)
        .bind(request.actor).bind(request.reason)
        .bind(after.on_hand()-before.on_hand()).bind(after.quarantine()-before.quarantine())
        .bind(after.on_hand()).bind(after.quarantine()).bind(after.reserved())
        .execute(&mut *tx).await;
    if let Err(error) = recorded {
        tx.rollback().await?;
        if error
            .as_database_error()
            .is_some_and(|error| error.is_unique_violation())
        {
            return Err(StockWriteError::IdempotencyConflict);
        }
        return Err(error.into());
    }
    tx.commit().await?;
    Ok(MovementReceipt {
        balance: after,
        replayed: false,
    })
}
