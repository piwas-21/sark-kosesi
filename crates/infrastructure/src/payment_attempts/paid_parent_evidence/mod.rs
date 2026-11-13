//! Private, single-use paid-parent aggregate GET evidence. No financial effects.
use super::evidence::{context, fresh_id, input, limits, now};
use super::*;
use crate::mollie::{ReversalTotals, VerifiedReconciliationClient};
use commerce_application::request_context::{Clock, Identifiers, ObservedClock};
use sqlx::PgPool;
mod rows;
mod set;
mod source;
mod transport;
pub use set::{SetInspection, inspect_set};
use source::{claim_attempt, paid_identity, validate_source};
pub use transport::{Fetched, Permit, fetch};

#[derive(Clone, PartialEq, Eq)]
pub struct Operation {
    pub id: String,
    pub order_id: String,
    pub attempt_id: String,
    pub effect_key: String,
    pub recognition_id: String,
    pub through_observation: i64,
    pub profile_id: String,
    pub payment_id: String,
    pub credential_fingerprint: String,
    pub started_at: i64,
    pub fetch_before: i64,
}
#[derive(Clone, PartialEq, Eq)]
pub struct Receipt {
    pub operation: Operation,
    pub totals: ReversalTotals,
    pub fetched_at: i64,
    pub recorded_at: i64,
    pub event_id: String,
    pub replayed: bool,
}
pub enum Start {
    Fetch(Permit),
    Pending(Operation),
    Complete(Receipt),
}
pub enum Status {
    Pending(Operation),
    Complete(Receipt),
}

/// Reserve one GET before any network access. A replay can never grant another permit.
pub async fn start(
    pool: &PgPool,
    clock: &dyn Clock,
    ids: &dyn Identifiers,
    attempt_id: &str,
    effect: &str,
    client: &VerifiedReconciliationClient,
) -> Result<Start, PaymentStoreError> {
    input(attempt_id, effect)?;
    let mut tx = pool.begin().await?;
    limits(&mut tx).await?;
    claim_attempt(&mut tx, attempt_id).await?;
    if let Some(op) = rows::by_effect(&mut tx, attempt_id, effect).await? {
        validate_source(&mut tx, &op).await?;
        if (op.profile_id.as_str(), op.credential_fingerprint.as_str())
            != (client.profile_id(), client.credential_fingerprint())
        {
            return Err(PaymentStoreError::Conflict);
        }
        let receipt = rows::receipt(&mut tx, &op).await?;
        tx.rollback().await?;
        return Ok(match receipt {
            Some(receipt) => Start::Complete(receipt),
            None => Start::Pending(op),
        });
    }
    let (frozen, recognition) = recognition::current_in_transaction(&mut tx, attempt_id).await?;
    let recognition = recognition.ok_or(PaymentStoreError::Unavailable)?;
    let payment_id = paid_identity(&mut tx, &frozen, &recognition).await?;
    if frozen.snapshot().binding.profile.id != client.profile_id() {
        return Err(PaymentStoreError::Conflict);
    }
    let initial = clock
        .unix_seconds()
        .map_err(|_| PaymentStoreError::ClockUnavailable)?;
    let observed = ObservedClock::new(clock, initial);
    let at = now(&observed)?;
    let (count, last) = rows::history(&mut tx, attempt_id).await?;
    if count >= 128 {
        return Err(PaymentStoreError::Unavailable);
    }
    let minimum_started_at = last
        .unwrap_or(recognition.recorded_at)
        .max(recognition.recorded_at);
    if at < minimum_started_at {
        return Err(PaymentStoreError::ClockUnavailable);
    }
    let fetch_before = at
        .checked_add(60)
        .filter(|v| *v <= commerce_domain::checkout_quotes::MAX_QUOTE_UNIX_SECONDS)
        .ok_or(PaymentStoreError::ClockUnavailable)?;
    let op = Operation {
        id: fresh_id(ids)?,
        order_id: frozen.snapshot().binding.order_id.clone(),
        attempt_id: attempt_id.into(),
        effect_key: effect.into(),
        recognition_id: recognition.id,
        through_observation: recognition.through_observation,
        profile_id: client.profile_id().into(),
        payment_id,
        credential_fingerprint: client.credential_fingerprint().into(),
        started_at: at,
        fetch_before,
    };
    rows::insert_operation(&mut tx, &op).await?;
    sqlx::query("SET CONSTRAINTS require_paid_parent_total_operation IMMEDIATE")
        .execute(&mut *tx)
        .await?;
    let (saved, own) = rows::by_effect_with_xid(&mut tx, attempt_id, effect)
        .await?
        .ok_or(PaymentStoreError::InvalidStoredData)?;
    if (saved, own) != (op.clone(), true) {
        return Err(PaymentStoreError::InvalidStoredData);
    }
    let final_at = now(&observed)?;
    if final_at >= fetch_before {
        return Err(PaymentStoreError::Unavailable);
    }
    tx.commit().await?;
    Ok(Start::Fetch(Permit {
        operation: op,
        attempt: frozen.snapshot().clone(),
    }))
}

/// Append exactly the response produced by the reserved GET; no new GET is made here.
pub async fn record(
    pool: &PgPool,
    clock: &dyn Clock,
    ids: &dyn Identifiers,
    fetched: &Fetched,
) -> Result<Receipt, PaymentStoreError> {
    let op = &fetched.operation;
    let mut tx = pool.begin().await?;
    limits(&mut tx).await?;
    claim_attempt(&mut tx, &op.attempt_id).await?;
    let (stored, _) = rows::by_effect_with_xid(&mut tx, &op.attempt_id, &op.effect_key)
        .await?
        .ok_or(PaymentStoreError::NotFound)?;
    if stored != *op {
        return Err(PaymentStoreError::InvalidStoredData);
    }
    let (frozen, _) = validate_source(&mut tx, op).await?;
    // Fetched is sealed by transport: the permit fixed the payment ID and
    // credential, and its completion clock cannot precede the admitted start.
    if fetched.verified.attempt() != frozen.snapshot() {
        return Err(PaymentStoreError::InvalidStoredData);
    }
    if let Some(receipt) = rows::receipt(&mut tx, op).await? {
        if (receipt.totals, receipt.fetched_at) != (fetched.verified.totals(), fetched.fetched_at) {
            return Err(PaymentStoreError::Conflict);
        }
        tx.rollback().await?;
        return Ok(receipt);
    }
    let initial = clock
        .unix_seconds()
        .map_err(|_| PaymentStoreError::ClockUnavailable)?;
    let observed = ObservedClock::new(clock, initial);
    let at = now(&observed)?;
    if at < fetched.fetched_at {
        return Err(PaymentStoreError::ClockUnavailable);
    }
    let receipt = Receipt {
        operation: op.clone(),
        totals: fetched.verified.totals(),
        fetched_at: fetched.fetched_at,
        recorded_at: at,
        event_id: fresh_id(ids)?,
        replayed: false,
    };
    rows::insert_receipt(&mut tx, &receipt).await?;
    sqlx::query("SET CONSTRAINTS require_paid_parent_total_receipt IMMEDIATE")
        .execute(&mut *tx)
        .await?;
    let mut saved = rows::receipt(&mut tx, op)
        .await?
        .ok_or(PaymentStoreError::InvalidStoredData)?;
    saved.replayed = false;
    let own = rows::receipt_current_transaction(&mut tx, &op.id).await?;
    if (saved, own) != (receipt.clone(), true) {
        return Err(PaymentStoreError::InvalidStoredData);
    }
    now(&observed)?;
    tx.commit().await?;
    Ok(receipt)
}

/// Inspect an existing operation without issuing network requests or a new permit.
pub async fn lookup(
    pool: &PgPool,
    attempt_id: &str,
    effect: &str,
) -> Result<Option<Status>, PaymentStoreError> {
    input(attempt_id, effect)?;
    let mut tx = pool.begin().await?;
    limits(&mut tx).await?;
    claim_attempt(&mut tx, attempt_id).await?;
    let Some(op) = rows::by_effect(&mut tx, attempt_id, effect).await? else {
        context(&mut tx, attempt_id, true).await?;
        tx.rollback().await?;
        return Ok(None);
    };
    validate_source(&mut tx, &op).await?;
    let receipt = rows::receipt(&mut tx, &op).await?;
    tx.rollback().await?;
    Ok(Some(match receipt {
        Some(receipt) => Status::Complete(receipt),
        None => Status::Pending(op),
    }))
}
