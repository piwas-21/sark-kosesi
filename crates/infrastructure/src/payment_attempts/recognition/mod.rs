//! Private gross recognition only; detached receipts grant no subsequent authority.
use super::evidence::{fresh_id, limits, now};
use super::*;
use commerce_application::request_context::{Clock, Identifiers, ObservedClock};
use commerce_domain::payment_attempts::recognition::derive;
pub use commerce_domain::payment_attempts::recognition::{Recognition, State};
use sqlx::PgPool;
mod proof;
mod rows;

pub struct RecognitionReceipt {
    pub recognition: Recognition,
    pub replayed: bool,
}

/// Canonical by original order and complete observation prefix, never an effect key.
pub async fn recognize(
    pool: &PgPool,
    clock: &dyn Clock,
    ids: &dyn Identifiers,
    attempt_id: &str,
) -> Result<RecognitionReceipt, PaymentStoreError> {
    validate(attempt_id)?;
    let mut tx = pool.begin().await?;
    limits(&mut tx).await?;
    let (frozen, started) = proof::context(&mut tx, attempt_id, true).await?;
    let watermark = proof::watermark(&mut tx, &frozen, started).await?;
    if let Some(recognition) = rows::load(&mut tx, &frozen, started, Some(watermark)).await? {
        tx.rollback().await?;
        return Ok(RecognitionReceipt {
            recognition,
            replayed: true,
        });
    }
    let previous = rows::pointer(&mut tx, &frozen, started).await?;
    let resolution = super::identities::rows::load(&mut tx, &frozen, started, None)
        .await?
        .filter(|resolution| resolution.through_observation == watermark)
        .ok_or(PaymentStoreError::Unavailable)?;
    proof::ownership(&mut tx, &frozen, &resolution).await?;
    let initial = clock
        .unix_seconds()
        .map_err(|_| PaymentStoreError::ClockUnavailable)?;
    let clock = ObservedClock::new(clock, initial);
    let at = now(&clock)?;
    if at < resolution.recorded_at || previous.is_some_and(|previous| at < previous.recorded_at) {
        return Err(PaymentStoreError::ClockUnavailable);
    }
    let binding = &frozen.snapshot().binding;
    let recognition = Recognition {
        id: fresh_id(ids)?,
        order_id: binding.order_id.clone(),
        attempt_id: binding.id.clone(),
        resolution_id: resolution.id,
        through_observation: watermark,
        recorded_at: at,
        state: derive(&resolution.summary).map_err(|_| PaymentStoreError::InvalidStoredData)?,
        currency: binding.currency,
        amount_minor: binding.amount_minor,
    };
    recognition
        .validate(
            frozen.snapshot(),
            &resolution.summary,
            watermark,
            resolution.recorded_at,
        )
        .map_err(|_| PaymentStoreError::InvalidStoredData)?;
    rows::insert(&mut tx, &recognition, &fresh_id(ids)?).await?;
    now(&clock)?;
    tx.commit().await?;
    Ok(RecognitionReceipt {
        recognition,
        replayed: false,
    })
}

/// Original prefix receipt, independent of newer evidence and current clock/ports.
pub async fn read_recorded(
    pool: &PgPool,
    attempt_id: &str,
    through_observation: i64,
) -> Result<Option<Recognition>, PaymentStoreError> {
    validate(attempt_id)?;
    if !(0..=1000).contains(&through_observation) {
        return Err(PaymentStoreError::InvalidRequest);
    }
    let mut tx = pool.begin().await?;
    limits(&mut tx).await?;
    let (frozen, started) = proof::context(&mut tx, attempt_id, false).await?;
    let recognition = rows::load(&mut tx, &frozen, started, Some(through_observation)).await?;
    tx.rollback().await?;
    Ok(recognition)
}

/// Current only within this locked transaction; later appends make detached data stale.
pub async fn current(
    pool: &PgPool,
    attempt_id: &str,
) -> Result<Option<Recognition>, PaymentStoreError> {
    validate(attempt_id)?;
    let mut tx = pool.begin().await?;
    limits(&mut tx).await?;
    let (frozen, started) = proof::context(&mut tx, attempt_id, true).await?;
    let watermark = proof::watermark(&mut tx, &frozen, started).await?;
    let recognition = rows::pointer(&mut tx, &frozen, started).await?;
    if recognition
        .as_ref()
        .is_some_and(|recognition| recognition.through_observation != watermark)
    {
        return Err(PaymentStoreError::Unavailable);
    }
    tx.rollback().await?;
    Ok(recognition)
}
fn validate(attempt: &str) -> Result<(), PaymentStoreError> {
    if !identifier(attempt) {
        return Err(PaymentStoreError::InvalidRequest);
    }
    Ok(())
}

/// Caller has established RC before reads; this retains the original attempt claim.
pub(crate) async fn current_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    attempt: &str,
) -> Result<(FrozenAttempt, Option<Recognition>), PaymentStoreError> {
    let (frozen, started) = proof::context(tx, attempt, true).await?;
    let watermark = proof::watermark(tx, &frozen, started).await?;
    let recognition = rows::pointer(tx, &frozen, started).await?;
    if recognition
        .as_ref()
        .is_some_and(|r| r.through_observation != watermark)
    {
        return Err(PaymentStoreError::Unavailable);
    }
    Ok((frozen, recognition))
}
/// Original receipt only; later observations and warehouse state are not consulted.
pub(crate) async fn historical_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    attempt: &str,
    recognition_id: &str,
) -> Result<(FrozenAttempt, Recognition), PaymentStoreError> {
    let (frozen, started) = proof::context(tx, attempt, true).await?;
    let through: Option<i64> = sqlx::query_scalar(
        "SELECT through_observation FROM payment_recognitions WHERE id=$1 AND attempt_id=$2",
    )
    .bind(recognition_id)
    .bind(attempt)
    .fetch_optional(&mut **tx)
    .await?;
    let recognition = rows::load(
        tx,
        &frozen,
        started,
        Some(through.ok_or(PaymentStoreError::InvalidStoredData)?),
    )
    .await?
    .filter(|r| r.id == recognition_id)
    .ok_or(PaymentStoreError::InvalidStoredData)?;
    Ok((frozen, recognition))
}
