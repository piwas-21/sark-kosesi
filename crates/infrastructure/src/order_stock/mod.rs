//! Private atomic consumption of the still-intact original order reservation.
use crate::payment_attempts::{PaymentStoreError, recognition};
use commerce_application::request_context::{Clock, Identifiers, ObservedClock};
pub use commerce_domain::order_stock::{
    Balance, Component, Footprint, Member, Quantity, Settlement,
};
use commerce_domain::{checkout_orders::OrderSnapshot, order_stock::*};
use sqlx::{PgPool, Postgres, Transaction};
mod effects;
mod rows;
mod source;
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
    if let Some(settlement) = rows::load(&mut tx, order_id).await? {
        tx.rollback().await?;
        return Ok(SettlementReceipt {
            settlement,
            replayed: true,
        });
    }
    let attempt = source::attempt(&mut tx, order_id).await?;
    source::claim_attempt(&mut tx, &attempt).await?;
    // Recheck canonical identity before financial eligibility after the wait.
    if let Some(settlement) = rows::load(&mut tx, order_id).await? {
        tx.rollback().await?;
        return Ok(SettlementReceipt {
            settlement,
            replayed: true,
        });
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
