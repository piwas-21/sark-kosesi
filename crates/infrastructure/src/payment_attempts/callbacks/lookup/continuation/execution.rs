//! One source-derived Test GET and receipt; no observation or financial effect.
use super::*;
use commerce_domain::payment_attempts::match_provider;

#[derive(Debug, PartialEq, Eq)]
pub enum ContinuationOutcome {
    Recorded { replayed: bool },
    Unknown,
}
enum Begin {
    Started(ContinuationStart),
    Recorded,
    Unknown,
}

async fn claim<'a>(
    tx: &mut Transaction<'a, Postgres>,
    attempt_id: &str,
) -> Result<(FrozenAttempt, LookupOperationSnapshot), PaymentStoreError> {
    let (frozen, observation) = source(tx, attempt_id).await?;
    sqlx::query("SELECT attempt_id FROM public.payment_dispatch_starts WHERE attempt_id OPERATOR(pg_catalog.=) $1 FOR UPDATE")
        .bind(attempt_id)
        .fetch_one(&mut **tx)
        .await?;
    let operation = existing(tx, attempt_id, &frozen, &observation, true)
        .await?
        .ok_or(PaymentStoreError::Unavailable)?;
    let job: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM public.payment_reconciliation_jobs WHERE attempt_id OPERATOR(pg_catalog.=) $1)",
    )
    .bind(attempt_id)
    .fetch_one(&mut **tx)
    .await?;
    if job {
        return Err(PaymentStoreError::InvalidStoredData);
    }
    Ok((frozen, operation))
}

fn validate_receipt(
    frozen: &FrozenAttempt,
    receipt: &LookupReceipt,
) -> Result<(), PaymentStoreError> {
    // The receipt decoder already binds the payment ID to the locked operation.
    match_provider(frozen.snapshot(), &receipt.snapshot.payment)
        .map_err(|_| PaymentStoreError::InvalidStoredData)
}

async fn status(
    tx: &mut Transaction<'_, Postgres>,
    frozen: &FrozenAttempt,
    operation: &LookupOperationSnapshot,
) -> Result<Option<ContinuationOutcome>, PaymentStoreError> {
    // Observe the start time and native arbitration proof in the same snapshot.
    let started: Option<bool> = sqlx::query_scalar(
        "SELECT started_at OPERATOR(pg_catalog.>=) $2
            AND started_at OPERATOR(pg_catalog.<=) $3
            AND public.payment_test_continuation_fenced($4)
         FROM public.callback_lookup_fetch_starts
         WHERE operation_id OPERATOR(pg_catalog.=) $1",
    )
    .bind(&operation.id)
    .bind(operation.created_at)
    .bind(commerce_domain::checkout_quotes::MAX_QUOTE_UNIX_SECONDS)
    .bind(&frozen.snapshot().binding.id)
    .fetch_optional(&mut **tx)
    .await?;
    if started == Some(false) {
        return Err(PaymentStoreError::InvalidStoredData);
    }
    if let Some(receipt) = rows::receipt(tx, operation).await? {
        validate_receipt(frozen, &receipt)?;
        return Ok(Some(ContinuationOutcome::Recorded { replayed: true }));
    }
    Ok(started.map(|_| ContinuationOutcome::Unknown))
}

/// Offline source-bound history inspection, with no credential, clock or I/O.
pub async fn inspect(
    pool: &PgPool,
    attempt_id: &str,
) -> Result<Option<ContinuationOutcome>, PaymentStoreError> {
    if !identifier(attempt_id) {
        return Err(PaymentStoreError::InvalidRequest);
    }
    let mut tx = pool.begin().await?;
    limits(&mut tx).await?;
    trusted_path(&mut tx).await?;
    let (frozen, operation) = claim(&mut tx, attempt_id).await?;
    let result = status(&mut tx, &frozen, &operation).await?;
    tx.rollback().await?;
    Ok(result)
}

async fn begin(
    pool: &PgPool,
    clock: &dyn Clock,
    attempt_id: &str,
    client: &VerifiedReconciliationClient,
) -> Result<Begin, MollieError> {
    if !identifier(attempt_id) {
        return Err(MollieError::Custody(PaymentStoreError::InvalidRequest));
    }
    let mut tx = pool
        .begin()
        .await
        .map_err(PaymentStoreError::from)
        .map_err(MollieError::Custody)?;
    limits(&mut tx).await.map_err(MollieError::Custody)?;
    trusted_path(&mut tx).await.map_err(MollieError::Custody)?;
    let (frozen, operation) = claim(&mut tx, attempt_id)
        .await
        .map_err(MollieError::Custody)?;
    match status(&mut tx, &frozen, &operation)
        .await
        .map_err(MollieError::Custody)?
    {
        Some(ContinuationOutcome::Recorded { .. }) => {
            tx.rollback()
                .await
                .map_err(PaymentStoreError::from)
                .map_err(MollieError::Custody)?;
            return Ok(Begin::Recorded);
        }
        Some(ContinuationOutcome::Unknown) => {
            tx.rollback()
                .await
                .map_err(PaymentStoreError::from)
                .map_err(MollieError::Custody)?;
            return Ok(Begin::Unknown);
        }
        None => {}
    }
    if operation.profile_id != client.profile_id() {
        return Err(MollieError::ProfileMismatch);
    }
    set_durable(&mut tx).await.map_err(MollieError::Custody)?;
    let initial = clock
        .unix_seconds()
        .map_err(|_| MollieError::Custody(PaymentStoreError::ClockUnavailable))?;
    let clock = ObservedClock::new(clock, initial);
    let started_at = now(&clock).map_err(MollieError::Custody)?;
    if started_at < operation.created_at {
        return Err(MollieError::Custody(PaymentStoreError::ClockUnavailable));
    }
    let changed = sqlx::query("INSERT INTO public.callback_lookup_fetch_starts(operation_id,started_at) VALUES($1,$2) ON CONFLICT(operation_id) DO NOTHING")
        .bind(&operation.id).bind(started_at).execute(&mut *tx).await.map_err(PaymentStoreError::from).map_err(MollieError::Custody)?.rows_affected();
    if changed != 1 {
        return Err(MollieError::Custody(PaymentStoreError::Unavailable));
    }
    now(&clock).map_err(MollieError::Custody)?;
    require_durable(&mut tx)
        .await
        .map_err(MollieError::Custody)?;
    // A failed COMMIT can have taken effect; never issue a GET or claim absence.
    if tx.commit().await.is_err() {
        return Ok(Begin::Unknown);
    }
    Ok(Begin::Started(ContinuationStart {
        operation,
        original: frozen.snapshot().clone(),
        started_at,
    }))
}

async fn record(
    pool: &PgPool,
    clock: &dyn Clock,
    ids: &dyn Identifiers,
    attempt_id: &str,
    fetched: &CallbackLookupPayment,
) -> Result<(), PaymentStoreError> {
    let mut tx = pool.begin().await?;
    limits(&mut tx).await?;
    trusted_path(&mut tx).await?;
    let (frozen, operation) = claim(&mut tx, attempt_id).await?;
    if fetched.operation() != &operation {
        return Err(PaymentStoreError::InvalidStoredData);
    }
    fetched
        .copy_payment()
        .bind(frozen.snapshot())
        .map_err(|_| PaymentStoreError::InvalidStoredData)?;
    if let Some(previous) = rows::receipt(&mut tx, &operation).await? {
        validate_receipt(&frozen, &previous)?;
        if !fetched.matches(&previous.snapshot) {
            return Err(PaymentStoreError::InvalidStoredData);
        }
        tx.rollback().await?;
        return Ok(());
    }
    set_durable(&mut tx).await?;
    let initial = clock
        .unix_seconds()
        .map_err(|_| PaymentStoreError::ClockUnavailable)?;
    let clock = ObservedClock::new(clock, initial);
    let snapshot = fetched.snapshot(fresh_id(ids)?, now(&clock)?);
    snapshot
        .validate()
        .map_err(|_| PaymentStoreError::InvalidStoredData)?;
    rows::validate_start(&mut tx, &snapshot).await?;
    rows::insert(&mut tx, &snapshot, &fresh_id(ids)?).await?;
    sqlx::query("SET CONSTRAINTS public.require_complete_callback_lookup IMMEDIATE")
        .execute(&mut *tx)
        .await?;
    trusted_path(&mut tx).await?;
    let saved = rows::receipt(&mut tx, &operation)
        .await?
        .ok_or(PaymentStoreError::InvalidStoredData)?;
    validate_receipt(&frozen, &saved)?;
    if saved.snapshot != snapshot {
        return Err(PaymentStoreError::InvalidStoredData);
    }
    now(&clock)?;
    require_durable(&mut tx).await?;
    tx.commit().await?;
    Ok(())
}

// The exclusive fsync-off control uses a genuine one-use loopback fetch before
// toggling its isolated cluster, then calls the actual post-I/O recorder.
#[cfg(test)]
pub(crate) async fn record_fsync_test(
    pool: &PgPool,
    clock: &dyn Clock,
    ids: &dyn Identifiers,
    attempt_id: &str,
    fetched: &CallbackLookupPayment,
) -> Result<(), PaymentStoreError> {
    record(pool, clock, ids, attempt_id, fetched).await
}

/// Explicit private invocation; any failure after the durable start is unknown.
pub async fn execute(
    pool: &PgPool,
    clock: &dyn Clock,
    ids: &dyn Identifiers,
    attempt_id: &str,
    client: &VerifiedReconciliationClient,
) -> Result<ContinuationOutcome, MollieError> {
    match begin(pool, clock, attempt_id, client).await? {
        Begin::Recorded => Ok(ContinuationOutcome::Recorded { replayed: true }),
        Begin::Unknown => Ok(ContinuationOutcome::Unknown),
        Begin::Started(permit) => {
            let Ok(fetched) = client.fetch_continuation(permit).await else {
                return Ok(ContinuationOutcome::Unknown);
            };
            if record(pool, clock, ids, attempt_id, &fetched)
                .await
                .is_err()
            {
                return Ok(ContinuationOutcome::Unknown);
            }
            Ok(ContinuationOutcome::Recorded { replayed: false })
        }
    }
}
