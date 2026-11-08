use super::*;
use commerce_domain::payment_attempts::recognition::{Recognition, State};

/// Serialize with observation and recognition writers before checking an effect.
pub(super) async fn claim_attempt(
    tx: &mut Transaction<'_, Postgres>,
    attempt_id: &str,
) -> Result<(), PaymentStoreError> {
    sqlx::query_scalar::<_, i32>("SELECT 1 FROM payment_attempts WHERE id=$1 FOR UPDATE")
        .bind(attempt_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(PaymentStoreError::NotFound)?;
    Ok(())
}

pub(super) async fn paid_identity(
    tx: &mut Transaction<'_, Postgres>,
    frozen: &FrozenAttempt,
    recognition: &Recognition,
) -> Result<String, PaymentStoreError> {
    if recognition.state != State::Received {
        return Err(PaymentStoreError::Unavailable);
    }
    // Current/historical recognition restoration already checked the dispatch source.
    let started: i64 =
        sqlx::query_scalar("SELECT started_at FROM payment_dispatch_starts WHERE attempt_id=$1")
            .bind(&recognition.attempt_id)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or(PaymentStoreError::InvalidStoredData)?;
    let resolution = identities::rows::by_id(tx, frozen, started, &recognition.resolution_id)
        .await?
        .ok_or(PaymentStoreError::InvalidStoredData)?;
    resolution
        .summary
        .paid_provider_ids
        .into_iter()
        .next()
        .ok_or(PaymentStoreError::InvalidStoredData)
}

pub(super) async fn validate_source(
    tx: &mut Transaction<'_, Postgres>,
    op: &Operation,
) -> Result<(FrozenAttempt, Recognition), PaymentStoreError> {
    let (frozen, recognition) =
        recognition::historical_in_transaction(tx, &op.attempt_id, &op.recognition_id).await?;
    let paid = paid_identity(tx, &frozen, &recognition).await?;
    if (
        recognition.through_observation,
        &frozen.snapshot().binding.order_id,
        &frozen.snapshot().binding.profile.id,
        &paid,
    ) != (
        op.through_observation,
        &op.order_id,
        &op.profile_id,
        &op.payment_id,
    ) || op.started_at < recognition.recorded_at
    {
        return Err(PaymentStoreError::InvalidStoredData);
    }
    Ok((frozen, recognition))
}
