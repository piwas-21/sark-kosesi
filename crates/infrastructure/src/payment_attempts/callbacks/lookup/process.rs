//! Receipt chronology precedes every fresh observation; historical replay stays inert.
use super::super::super::{evidence::context, observations};
use super::*;
/// Unknown originals retain visible lookup custody. Exact historical replay needs
/// no clock or entropy; fresh facts cannot predate the authenticated receipt.
pub async fn process_lookup(
    pool: &PgPool,
    clock: &dyn Clock,
    ids: &dyn Identifiers,
    operation_id: &str,
) -> Result<observations::ObservationReceipt, PaymentStoreError> {
    let receipt = read_lookup(pool, operation_id)
        .await?
        .ok_or(PaymentStoreError::Unavailable)?;
    let mut tx = pool.begin().await?;
    limits(&mut tx).await?;
    let (_, dispatched_at) = context(&mut tx, &receipt.snapshot.payment.attempt_id, false).await?;
    if receipt.snapshot.started_at < dispatched_at {
        return Err(PaymentStoreError::Conflict);
    }
    tx.rollback().await?;
    let effect = format!("callback-lookup-{operation_id}");
    if let Some(previous) =
        observations::read_recorded(pool, &receipt.snapshot.payment.attempt_id, &effect).await?
    {
        if previous.intent.payment != receipt.snapshot.payment
            || previous.intent.credential_fingerprint != receipt.snapshot.credential_fingerprint
            || previous.recorded_at < receipt.snapshot.recorded_at
        {
            return Err(PaymentStoreError::InvalidStoredData);
        }
        return Ok(observations::ObservationReceipt {
            snapshot: previous,
            replayed: true,
        });
    }
    let initial = clock
        .unix_seconds()
        .map_err(|_| PaymentStoreError::ClockUnavailable)?;
    let clock = ObservedClock::new(clock, initial);
    if now(&clock)? < receipt.snapshot.recorded_at {
        return Err(PaymentStoreError::ClockUnavailable);
    }
    super::super::record_fetched(pool, &clock, ids, &effect, &receipt.fetched).await
}
