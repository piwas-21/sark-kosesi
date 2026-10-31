//! Trusted worker evidence custody only. No HTTP, network or financial transition.
use super::*;
use crate::mollie::VerifiedPayment;
use commerce_application::request_context::{Clock, Identifiers, ObservedClock};
use commerce_domain::{
    checkout_quotes::MAX_QUOTE_UNIX_SECONDS,
    payment_attempts::{
        DispatchState,
        observations::{MAX_OBSERVATION_REVISION, ObservationIntent, ObservationSnapshot},
    },
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
fn input(attempt_id: &str, effect: &str) -> Result<(), PaymentStoreError> {
    if !identifier(attempt_id) || !valid_command_id(effect) {
        return Err(PaymentStoreError::InvalidRequest);
    }
    Ok(())
}
async fn limits(tx: &mut Transaction<'_, Postgres>) -> Result<(), PaymentStoreError> {
    sqlx::query("SET LOCAL lock_timeout='1s';")
        .execute(&mut **tx)
        .await?;
    sqlx::query("SET LOCAL statement_timeout='2s';")
        .execute(&mut **tx)
        .await?;
    Ok(())
}
async fn context(
    tx: &mut Transaction<'_, Postgres>,
    id: &str,
    claim: bool,
) -> Result<(FrozenAttempt, i64), PaymentStoreError> {
    let owner: Option<String> = if claim {
        sqlx::query_scalar("SELECT left(basket_id,33) FROM payment_attempts WHERE id=$1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut **tx)
            .await?
    } else {
        sqlx::query_scalar("SELECT left(basket_id,33) FROM payment_attempts WHERE id=$1")
            .bind(id)
            .fetch_optional(&mut **tx)
            .await?
    };
    let frozen = stored::by_id(tx, &owner.ok_or(PaymentStoreError::NotFound)?, id).await?;
    let (start_id, started_at): (String, i64) = sqlx::query_as(
        "SELECT left(id,33),started_at FROM payment_dispatch_starts WHERE attempt_id=$1",
    )
    .bind(id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(PaymentStoreError::Unavailable)?;
    if !identifier(&start_id)
        || (DispatchState::Started { started_at })
            .validate(frozen.snapshot())
            .is_err()
    {
        return Err(PaymentStoreError::InvalidStoredData);
    }
    Ok((frozen, started_at))
}
fn now(clock: &dyn Clock) -> Result<i64, PaymentStoreError> {
    let at = i64::try_from(
        clock
            .unix_seconds()
            .map_err(|_| PaymentStoreError::ClockUnavailable)?,
    )
    .map_err(|_| PaymentStoreError::ClockUnavailable)?;
    if at > MAX_QUOTE_UNIX_SECONDS {
        return Err(PaymentStoreError::ClockUnavailable);
    }
    Ok(at)
}
fn fresh_id(ids: &dyn Identifiers) -> Result<String, PaymentStoreError> {
    ids.next()
        .map(|v| format!("{:032x}", u128::from_be_bytes(v)))
        .map_err(|_| {
            PaymentStoreError::Boundary(checkout_orders::OrderStoreError::EntropyUnavailable)
        })
}
