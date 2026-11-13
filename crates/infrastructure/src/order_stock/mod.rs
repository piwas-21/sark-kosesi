//! Private atomic consumption of the still-intact original order reservation.
use crate::payment_attempts::{PaymentStoreError, recognition};
use commerce_application::request_context::{Clock, Identifiers, ObservedClock};
pub use commerce_domain::order_stock::{
    Balance, Component, Footprint, Member, Quantity, Settlement,
};
use commerce_domain::{checkout_orders::OrderSnapshot, order_stock::*};
use sqlx::{PgPool, Postgres, Transaction};
mod effects;
pub(crate) mod rows;
pub(crate) mod source;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettlementError {
    InvalidRequest,
    NotFound,
    Unavailable,
    InvalidStoredData,
    RecoveryRequired,
    ClockUnavailable,
    EntropyUnavailable,
    Database,
}
impl From<sqlx::Error> for SettlementError {
    fn from(_: sqlx::Error) -> Self {
        Self::Database
    }
}
impl From<SettlementPolicyError> for SettlementError {
    fn from(error: SettlementPolicyError) -> Self {
        match error {
            SettlementPolicyError::InvalidSnapshot => Self::InvalidStoredData,
            SettlementPolicyError::InvalidClock => Self::ClockUnavailable,
            SettlementPolicyError::RecoveryRequired => Self::RecoveryRequired,
        }
    }
}
impl From<PaymentStoreError> for SettlementError {
    fn from(error: PaymentStoreError) -> Self {
        match error {
            PaymentStoreError::InvalidRequest => Self::InvalidRequest,
            PaymentStoreError::NotFound => Self::NotFound,
            PaymentStoreError::Unavailable => Self::Unavailable,
            PaymentStoreError::ClockUnavailable => Self::ClockUnavailable,
            PaymentStoreError::Database
            | PaymentStoreError::Boundary(crate::checkout_orders::OrderStoreError::Database) => {
                Self::Database
            }
            _ => Self::InvalidStoredData,
        }
    }
}
pub struct SettlementReceipt {
    pub settlement: Settlement,
    pub replayed: bool,
}
pub async fn settle(
    pool: &PgPool,
    clock: &dyn Clock,
    ids: &dyn Identifiers,
    order_id: &str,
) -> Result<SettlementReceipt, SettlementError> {
    validate(order_id)?;
    let mut tx = pool.begin().await?;
    limits(&mut tx).await?;
    if let Some(receipt) = cross_path(&mut tx, order_id).await? {
        tx.rollback().await?;
        return Ok(receipt);
    }
    let attempt = source::attempt(&mut tx, order_id).await?;
    source::claim_attempt(&mut tx, &attempt).await?;
    // Recheck canonical identity before financial eligibility after the wait.
    if let Some(receipt) = cross_path(&mut tx, order_id).await? {
        tx.rollback().await?;
        return Ok(receipt);
    }
    let (frozen, received) = recognition::current_in_transaction(&mut tx, &attempt).await?;
    let received = received
        .filter(|r| r.state == recognition::State::Received)
        .ok_or(SettlementError::Unavailable)?;
    let order = crate::checkout_orders::snapshot_in_transaction(
        &mut tx,
        &frozen.snapshot().binding.basket_id,
        order_id,
    )
    .await
    .map_err(|error| match error {
        crate::checkout_orders::OrderStoreError::Database => SettlementError::Database,
        _ => SettlementError::InvalidStoredData,
    })?;
    let members = source::members(&mut tx, &order, true).await?;
    let initial = clock
        .unix_seconds()
        .map_err(|_| SettlementError::ClockUnavailable)?;
    let clock = ObservedClock::new(clock, initial);
    let components = source::balances(&mut tx, &members.members, &members.active_members).await?;
    if !members.active {
        return Err(SettlementError::RecoveryRequired);
    }
    let at = now(&clock)?;
    check_time(
        at,
        order.identity.created_at,
        order.identity.hold_expires_at,
        received.recorded_at,
    )?;
    let settlement = Settlement {
        id: fresh(ids)?,
        order_id: order_id.into(),
        attempt_id: attempt,
        recognition_id: received.id,
        group_id: order.identity.stock_group_id.clone(),
        actor: ACTOR.into(),
        settled_at: at,
        footprint: Footprint {
            schema_version: 1,
            members: members.members,
            components,
        },
    };
    settlement.validate()?;
    rows::insert(&mut tx, &settlement).await?;
    for member in &settlement.footprint.members {
        crate::holds::consume_claimed(&mut tx, &member.hold_id, order_id, at)
            .await
            .map_err(|error| match error {
                crate::holds::HoldWriteError::Database(_) => SettlementError::Database,
                _ => SettlementError::InvalidStoredData,
            })?;
    }
    rows::event(&mut tx, &settlement.id, &fresh(ids)?).await?;
    check_time(
        now(&clock)?,
        order.identity.created_at,
        order.identity.hold_expires_at,
        received.recorded_at,
    )?;
    tx.commit().await?;
    Ok(SettlementReceipt {
        settlement,
        replayed: false,
    })
}
pub async fn read_settlement(
    pool: &PgPool,
    order_id: &str,
) -> Result<Option<Settlement>, SettlementError> {
    validate(order_id)?;
    let mut tx = pool.begin().await?;
    limits(&mut tx).await?;
    let settlement = rows::load(&mut tx, order_id).await?;
    tx.rollback().await?;
    Ok(settlement)
}
fn validate(id: &str) -> Result<(), SettlementError> {
    if !identifier(id) {
        return Err(SettlementError::InvalidRequest);
    }
    Ok(())
}
async fn limits(tx: &mut Transaction<'_, Postgres>) -> Result<(), SettlementError> {
    sqlx::query("SET TRANSACTION ISOLATION LEVEL READ COMMITTED")
        .execute(&mut **tx)
        .await?;
    sqlx::query("SET LOCAL lock_timeout='1s'")
        .execute(&mut **tx)
        .await?;
    sqlx::query("SET LOCAL statement_timeout='2s'")
        .execute(&mut **tx)
        .await?;
    #[cfg(test)]
    crate::late_stock_wait_budget::apply(tx).await?;
    Ok(())
}
fn now(clock: &dyn Clock) -> Result<i64, SettlementError> {
    i64::try_from(
        clock
            .unix_seconds()
            .map_err(|_| SettlementError::ClockUnavailable)?,
    )
    .map_err(|_| SettlementError::ClockUnavailable)
}
fn fresh(ids: &dyn Identifiers) -> Result<String, SettlementError> {
    ids.next()
        .map(|id| format!("{:032x}", u128::from_be_bytes(id)))
        .map_err(|_| SettlementError::EntropyUnavailable)
}

/// Same caller-owned RC transaction; historical receipt validation keeps its claim.
pub(crate) async fn read_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    order: &str,
) -> Result<Option<Settlement>, SettlementError> {
    rows::load(tx, order).await
}

async fn cross_path(
    tx: &mut Transaction<'_, Postgres>,
    order: &str,
) -> Result<Option<SettlementReceipt>, SettlementError> {
    let recorded = crate::late_stock::read_in_transaction(tx, order)
        .await
        .map_err(|error| match error {
            crate::late_stock::LateStockError::Database => SettlementError::Database,
            _ => SettlementError::InvalidStoredData,
        })?;
    match recorded {
        Some(crate::late_stock::Outcome::Original(settlement)) => Ok(Some(SettlementReceipt {
            settlement,
            replayed: true,
        })),
        Some(_) => Err(SettlementError::Unavailable),
        None => Ok(None),
    }
}

#[cfg(test)]
mod wait_budget_tests {
    use super::limits;
    use crate::{MIGRATOR, late_stock_wait_budget};
    use sqlx::PgPool;

    #[sqlx::test(migrator = "MIGRATOR")]
    async fn wait_budget_is_explicit_task_scoped_and_transaction_local(pool: PgPool) {
        let mut tx = pool.begin().await.expect("default transaction");
        limits(&mut tx).await.expect("default limits");
        assert_eq!(
            late_stock_wait_budget::settings(&mut tx).await,
            ("1s".into(), "2s".into())
        );
        tx.rollback().await.expect("release");
        late_stock_wait_budget::scope(async {
            let mut tx = pool.begin().await.expect("scoped transaction");
            limits(&mut tx).await.expect("scoped limits");
            assert_eq!(
                late_stock_wait_budget::settings(&mut tx).await,
                ("10s".into(), "20s".into())
            );
            tx.rollback().await.expect("release");
        })
        .await;
        let mut tx = pool.begin().await.expect("post-scope transaction");
        limits(&mut tx).await.expect("post-scope limits");
        assert_eq!(
            late_stock_wait_budget::settings(&mut tx).await,
            ("1s".into(), "2s".into())
        );
        tx.rollback().await.expect("release");
    }
}
