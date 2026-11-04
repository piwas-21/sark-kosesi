//! Private authenticated reversal evidence; no financial or inventory authority.
use super::evidence::{context, fresh_id, input, limits, now};
use super::*;
use crate::mollie::{ReversalPhase, VerifiedReversal};
use commerce_application::request_context::{Clock, Identifiers, ObservedClock};
use sqlx::PgPool;
mod rows;

/// Immutable historical evidence. Identifiers and credential provenance are private.
#[derive(PartialEq, Eq)]
pub struct ReversalReceipt {
    pub id: String,
    pub order_id: String,
    pub attempt_id: String,
    pub effect_key: String,
    pub payment_id: String,
    pub child_id: String,
    pub phase: ReversalPhase,
    pub amount_minor: i64,
    pub credential_fingerprint: String,
    pub source_created_seconds: i64,
    pub source_created_nanos: i32,
    pub source_reversed_seconds: Option<i64>,
    pub source_reversed_nanos: Option<i32>,
    pub recorded_at: i64,
    pub event_id: String,
    pub replayed: bool,
}
impl ReversalReceipt {
    fn matches(&self, v: &VerifiedReversal) -> bool {
        let reversed = v
            .reversed_at()
            .map(|t| (t.unix_timestamp(), t.nanosecond() as i32));
        self.payment_id == v.payment_id()
            && self.child_id == v.child_id()
            && self.phase == v.phase()
            && self.amount_minor == v.amount_minor()
            && self.credential_fingerprint == v.credential_fingerprint()
            && self.source_created_seconds == v.created_at().unix_timestamp()
            && self.source_created_nanos == v.created_at().nanosecond() as i32
            && self.source_reversed_seconds == reversed.map(|v| v.0)
            && self.source_reversed_nanos == reversed.map(|v| v.1)
    }
}
/// Append a previously authenticated exact-item fetch. Call lookup before retrying I/O.
pub async fn record(
    pool: &PgPool,
    clock: &dyn Clock,
    ids: &dyn Identifiers,
    effect: &str,
    verified: &VerifiedReversal,
) -> Result<ReversalReceipt, PaymentStoreError> {
    let attempt_id = &verified.attempt().binding.id;
    input(attempt_id, effect)?;
    let mut tx = pool.begin().await?;
    limits(&mut tx).await?;
    let (frozen, started_at) = context(&mut tx, attempt_id, true).await?;
    if verified.attempt() != frozen.snapshot() {
        return Err(PaymentStoreError::Conflict);
    }
    if let Some(previous) = rows::load(&mut tx, &frozen, started_at, effect).await? {
        if !previous.matches(verified) {
            return Err(PaymentStoreError::Conflict);
        }
        tx.rollback().await?;
        return Ok(previous);
    }
    let initial = clock
        .unix_seconds()
        .map_err(|_| PaymentStoreError::ClockUnavailable)?;
    let clock = ObservedClock::new(clock, initial);
    let at = now(&clock)?;
    let (count, last_at) = rows::history(&mut tx, attempt_id).await?;
    if count >= 128 {
        return Err(PaymentStoreError::Unavailable);
    }
    if at < started_at || last_at.is_some_and(|last| at < last) {
        return Err(PaymentStoreError::ClockUnavailable);
    }
    let reversed = verified
        .reversed_at()
        .map(|t| (t.unix_timestamp(), t.nanosecond() as i32));
    let receipt = ReversalReceipt {
        id: fresh_id(ids)?,
        order_id: frozen.snapshot().binding.order_id.clone(),
        attempt_id: attempt_id.clone(),
        effect_key: effect.into(),
        payment_id: verified.payment_id().into(),
        child_id: verified.child_id().into(),
        phase: verified.phase(),
        amount_minor: verified.amount_minor(),
        credential_fingerprint: verified.credential_fingerprint().into(),
        source_created_seconds: verified.created_at().unix_timestamp(),
        source_created_nanos: verified.created_at().nanosecond() as i32,
        source_reversed_seconds: reversed.map(|v| v.0),
        source_reversed_nanos: reversed.map(|v| v.1),
        recorded_at: at,
        event_id: fresh_id(ids)?,
        replayed: false,
    };
    rows::validate(&receipt, &frozen, started_at)?;
    rows::insert(&mut tx, &receipt).await?;
    sqlx::query("SET CONSTRAINTS require_complete_reversal_observation IMMEDIATE")
        .execute(&mut *tx)
        .await?;
    // Verify the saved row and exact event even if a trigger suppresses a write.
    let mut saved = rows::load(&mut tx, &frozen, started_at, effect)
        .await?
        .ok_or(PaymentStoreError::InvalidStoredData)?;
    saved.replayed = false;
    if saved != receipt {
        return Err(PaymentStoreError::InvalidStoredData);
    }
    now(&clock)?;
    tx.commit().await?;
    Ok(receipt)
}
/// Recover a committed receipt before attempting another provider request.
pub async fn lookup(
    pool: &PgPool,
    attempt_id: &str,
    effect: &str,
) -> Result<Option<ReversalReceipt>, PaymentStoreError> {
    input(attempt_id, effect)?;
    let mut tx = pool.begin().await?;
    limits(&mut tx).await?;
    let (frozen, started_at) = context(&mut tx, attempt_id, false).await?;
    let result = rows::load(&mut tx, &frozen, started_at, effect).await?;
    tx.rollback().await?;
    Ok(result)
}
