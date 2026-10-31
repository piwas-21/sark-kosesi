//! Trusted worker evidence custody only. No HTTP, network or financial transition.
use super::evidence::{context, fresh_id, input, limits, now};
use super::*;
use crate::mollie::VerifiedPayment;
use commerce_application::request_context::{Clock, Identifiers, ObservedClock};
use commerce_domain::payment_attempts::observations::{
    MAX_OBSERVATION_REVISION, ObservationIntent, ObservationSnapshot,
};
use sqlx::PgPool;
mod rows;
/// A durable fetched fact, never payment/stock/fulfilment authority.
pub struct ObservationReceipt {
    pub snapshot: ObservationSnapshot,
    pub replayed: bool,
}
/// Only the opaque authenticated-fetch result enters this boundary, never callback JSON.
/// Changed facts need a new observation effect; exact retries retain original history.
pub async fn record_verified(
    pool: &PgPool,
    clock: &dyn Clock,
    identifiers: &dyn Identifiers,
    attempt_id: &str,
    effect: &str,
    verified: &VerifiedPayment,
) -> Result<ObservationReceipt, PaymentStoreError> {
    input(attempt_id, effect)?;
    let intent = ObservationIntent {
        payment: verified.payment().clone(),
        credential_fingerprint: verified.credential_fingerprint().to_owned(),
    };
    let mut tx = pool.begin().await?;
    limits(&mut tx).await?;
    let (frozen, started_at) = context(&mut tx, attempt_id, true).await?;
    intent
        .validate(frozen.snapshot())
        .map_err(|_| PaymentStoreError::Conflict)?;
    if let Some(previous) = rows::by_effect(&mut tx, &frozen, started_at, effect).await? {
        if previous.intent != intent {
            return Err(PaymentStoreError::Conflict);
        }
        tx.rollback().await?;
        return Ok(ObservationReceipt {
            snapshot: previous,
            replayed: true,
        });
    }
    let initial = clock
        .unix_seconds()
        .map_err(|_| PaymentStoreError::ClockUnavailable)?;
    let clock = ObservedClock::new(clock, initial);
    let at = now(&clock)?;
    let previous = rows::latest(&mut tx, &frozen, started_at).await?;
    if at < started_at || previous.as_ref().is_some_and(|p| at < p.recorded_at) {
        return Err(PaymentStoreError::ClockUnavailable);
    }
    let revision = previous
        .map_or(Some(1), |p| p.revision.checked_add(1))
        .filter(|v| *v <= MAX_OBSERVATION_REVISION)
        .ok_or(PaymentStoreError::Unavailable)?;
    let snapshot = ObservationSnapshot {
        schema_version: 1,
        id: fresh_id(identifiers)?,
        attempt_id: attempt_id.into(),
        effect_key: effect.into(),
        revision,
        recorded_at: at,
        intent,
    };
    snapshot
        .validate(frozen.snapshot(), started_at)
        .map_err(|_| PaymentStoreError::InvalidStoredData)?;
    rows::insert(
        &mut tx,
        &snapshot,
        &frozen.snapshot().binding.order_id,
        &fresh_id(identifiers)?,
    )
    .await?;
    now(&clock)?;
    tx.commit().await?;
    Ok(ObservationReceipt {
        snapshot,
        replayed: false,
    })
}
/// Trusted receipt lookup before retrying a fetch operation. Not a customer route.
pub async fn read_recorded(
    pool: &PgPool,
    attempt_id: &str,
    effect: &str,
) -> Result<Option<ObservationSnapshot>, PaymentStoreError> {
    input(attempt_id, effect)?;
    let mut tx = pool.begin().await?;
    limits(&mut tx).await?;
    let (frozen, started_at) = context(&mut tx, attempt_id, false).await?;
    let result = rows::by_effect(&mut tx, &frozen, started_at, effect).await?;
    tx.rollback().await?;
    Ok(result)
}
/// Trusted outbox-consumer read; returns only revalidated minimal historical facts.
pub async fn read_observation(
    pool: &PgPool,
    attempt_id: &str,
    observation_id: &str,
) -> Result<ObservationSnapshot, PaymentStoreError> {
    if !identifier(attempt_id) || !identifier(observation_id) {
        return Err(PaymentStoreError::InvalidRequest);
    }
    let mut tx = pool.begin().await?;
    limits(&mut tx).await?;
    let (frozen, started_at) = context(&mut tx, attempt_id, false).await?;
    let result = rows::by_id(&mut tx, &frozen, started_at, observation_id).await?;
    tx.rollback().await?;
    Ok(result)
}
