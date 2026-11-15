//! Offline one-attempt lookup custody. No GET, POST or financial authority.
use super::*;
use crate::payment_attempts::{evidence::context, observations};
use commerce_application::payment_requests::FrozenAttempt;
use commerce_domain::payment_attempts::observations::ObservationSnapshot;
use sqlx::{Postgres, Transaction};

// Confine the continuation's inherited shared readers to the intended objects,
// even when the pooled session carries a caller-supplied search path.
async fn trusted_path(tx: &mut Transaction<'_, Postgres>) -> Result<(), PaymentStoreError> {
    sqlx::query("SET LOCAL search_path=pg_catalog,public,pg_temp")
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Redacted immutable source binding. It is not proof of provider activity.
#[derive(Debug, PartialEq, Eq)]
pub struct ContinuationReceipt {
    pub attempt_id: String,
    pub operation_id: String,
    pub replayed: bool,
}

impl ContinuationReceipt {
    fn from_operation(
        attempt_id: &str,
        operation: LookupOperationSnapshot,
        replayed: bool,
    ) -> Self {
        Self {
            attempt_id: attempt_id.into(),
            operation_id: operation.id,
            replayed,
        }
    }
}

async fn source(
    tx: &mut Transaction<'_, Postgres>,
    attempt_id: &str,
) -> Result<(FrozenAttempt, ObservationSnapshot), PaymentStoreError> {
    let (frozen, started_at) = context(tx, attempt_id, true).await?;
    let observation = observations::initial::read_source(tx, &frozen, started_at)
        .await?
        .ok_or(PaymentStoreError::Unavailable)?;
    Ok((frozen, observation))
}

async fn existing(
    tx: &mut Transaction<'_, Postgres>,
    attempt_id: &str,
    frozen: &FrozenAttempt,
    observation: &ObservationSnapshot,
    lock: bool,
) -> Result<Option<LookupOperationSnapshot>, PaymentStoreError> {
    let row: Option<(String, bool)> = sqlx::query_as("SELECT pg_catalog.left(c.operation_id,33),public.payment_test_continuation_exact(c) FROM public.payment_test_continuations c WHERE c.attempt_id OPERATOR(pg_catalog.=) $1")
        .bind(attempt_id).fetch_optional(&mut **tx).await?;
    let Some((operation_id, exact)) = row else {
        let orphan: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM public.callback_lookup_operations WHERE continuation_attempt_id OPERATOR(pg_catalog.=) $1)")
            .bind(attempt_id).fetch_one(&mut **tx).await?;
        return if orphan {
            Err(PaymentStoreError::InvalidStoredData)
        } else {
            Ok(None)
        };
    };
    if !exact {
        return Err(PaymentStoreError::InvalidStoredData);
    }
    let operation =
        rows::operation(tx, &operation_id, lock)
            .await
            .map_err(|error| match error {
                PaymentStoreError::NotFound => PaymentStoreError::InvalidStoredData,
                other => other,
            })?;
    require_operation_source(
        &operation,
        &frozen.snapshot().binding.profile.id,
        &observation.intent.payment.id,
        observation.recorded_at,
    )?;
    Ok(Some(operation))
}

fn require_operation_source(
    operation: &LookupOperationSnapshot,
    profile_id: &str,
    provider_id: &str,
    observed_at: i64,
) -> Result<(), PaymentStoreError> {
    if operation.profile_id != profile_id
        || operation.provider_id != provider_id
        || operation.created_at < observed_at
    {
        return Err(PaymentStoreError::InvalidStoredData);
    }
    Ok(())
}

/// Validate one original source and return its operation, if present.
pub async fn read(
    pool: &PgPool,
    attempt_id: &str,
) -> Result<Option<ContinuationReceipt>, PaymentStoreError> {
    if !identifier(attempt_id) {
        return Err(PaymentStoreError::InvalidRequest);
    }
    let mut tx = pool.begin().await?;
    limits(&mut tx).await?;
    trusted_path(&mut tx).await?;
    let (frozen, observation) = source(&mut tx, attempt_id).await?;
    let operation = existing(&mut tx, attempt_id, &frozen, &observation, false).await?;
    tx.rollback().await?;
    Ok(operation.map(|operation| ContinuationReceipt::from_operation(attempt_id, operation, true)))
}

/// Atomically acquire at most one source-derived operation for this attempt.
/// Existing custody is returned without clock, entropy or a new operation.
pub async fn acquire(
    pool: &PgPool,
    clock: &dyn Clock,
    ids: &dyn Identifiers,
    attempt_id: &str,
) -> Result<ContinuationReceipt, PaymentStoreError> {
    if !identifier(attempt_id) {
        return Err(PaymentStoreError::InvalidRequest);
    }
    let mut tx = pool.begin().await?;
    limits(&mut tx).await?;
    trusted_path(&mut tx).await?;
    let (frozen, observation) = source(&mut tx, attempt_id).await?;
    if let Some(operation) = existing(&mut tx, attempt_id, &frozen, &observation, false).await? {
        tx.rollback().await?;
        return Ok(ContinuationReceipt::from_operation(
            attempt_id, operation, true,
        ));
    }
    let initial = clock
        .unix_seconds()
        .map_err(|_| PaymentStoreError::ClockUnavailable)?;
    let clock = ObservedClock::new(clock, initial);
    if now(&clock)? < observation.recorded_at {
        return Err(PaymentStoreError::ClockUnavailable);
    }
    let operation = prepare_with_source(
        &mut tx,
        &clock,
        ids,
        &frozen.snapshot().binding.profile.id,
        &observation.intent.payment.id,
        Some(attempt_id),
    )
    .await?;
    trusted_path(&mut tx).await?;
    let inserted = sqlx::query(
        "INSERT INTO public.payment_test_continuations(attempt_id,operation_id) VALUES($1,$2)",
    )
    .bind(attempt_id)
    .bind(&operation.id)
    .execute(&mut *tx)
    .await?;
    if inserted.rows_affected() != 1 {
        return Err(PaymentStoreError::InvalidStoredData);
    }
    sqlx::query("SET CONSTRAINTS public.require_payment_test_continuation,public.require_callback_test_continuation IMMEDIATE")
        .execute(&mut *tx).await?;
    trusted_path(&mut tx).await?;
    let saved = existing(&mut tx, attempt_id, &frozen, &observation, false)
        .await?
        .ok_or(PaymentStoreError::InvalidStoredData)?;
    if saved != operation {
        return Err(PaymentStoreError::InvalidStoredData);
    }
    now(&clock)?;
    tx.commit().await?;
    Ok(ContinuationReceipt::from_operation(
        attempt_id, saved, false,
    ))
}

// A successful marked start is a one-use external-I/O boundary. Both it and
// its receipt must force a local WAL flush before returning to the caller.
async fn set_durable(tx: &mut Transaction<'_, Postgres>) -> Result<(), PaymentStoreError> {
    sqlx::query("SET LOCAL synchronous_commit='on'")
        .execute(&mut **tx)
        .await?;
    Ok(())
}
async fn require_durable(tx: &mut Transaction<'_, Postgres>) -> Result<(), PaymentStoreError> {
    let (fsync, synchronous_commit): (String, String) = sqlx::query_as("SELECT pg_catalog.current_setting('fsync'),pg_catalog.current_setting('synchronous_commit')")
        .fetch_one(&mut **tx).await?;
    if fsync != "on" || synchronous_commit != "on" {
        return Err(PaymentStoreError::Unavailable);
    }
    Ok(())
}

/// A committed marked start. It cannot be cloned, deserialized or passed to
/// the generic transport. There is deliberately no transport consumer here.
pub struct ContinuationStart {
    operation: LookupOperationSnapshot,
    original: commerce_domain::payment_attempts::AttemptSnapshot,
    started_at: i64,
}
impl ContinuationStart {
    pub fn operation_id(&self) -> &str {
        &self.operation.id
    }
    pub fn started_at(&self) -> i64 {
        self.started_at
    }
    pub(crate) fn into_parts(
        self,
    ) -> (
        LookupOperationSnapshot,
        commerce_domain::payment_attempts::AttemptSnapshot,
        i64,
    ) {
        (self.operation, self.original, self.started_at)
    }
}

/// Claim the exact source and commit its one marked start. A lost capability
/// remains started; this function never restores one or performs network I/O.
pub async fn start(
    pool: &PgPool,
    clock: &dyn Clock,
    attempt_id: &str,
) -> Result<ContinuationStart, PaymentStoreError> {
    if !identifier(attempt_id) {
        return Err(PaymentStoreError::InvalidRequest);
    }
    let initial = clock
        .unix_seconds()
        .map_err(|_| PaymentStoreError::ClockUnavailable)?;
    let clock = ObservedClock::new(clock, initial);
    let mut tx = pool.begin().await?;
    limits(&mut tx).await?;
    trusted_path(&mut tx).await?;
    set_durable(&mut tx).await?;
    let (frozen, observation) = source(&mut tx, attempt_id).await?;
    // Same order as the native start guard: attempt, dispatch, operation.
    sqlx::query("SELECT attempt_id FROM public.payment_dispatch_starts WHERE attempt_id OPERATOR(pg_catalog.=) $1 FOR UPDATE")
        .bind(attempt_id)
        .fetch_one(&mut *tx)
        .await?;
    let operation = existing(&mut tx, attempt_id, &frozen, &observation, true)
        .await?
        .ok_or(PaymentStoreError::Unavailable)?;
    let started_at = now(&clock)?;
    if started_at < operation.created_at {
        return Err(PaymentStoreError::ClockUnavailable);
    }
    let inserted = sqlx::query("INSERT INTO public.callback_lookup_fetch_starts(operation_id,started_at) VALUES($1,$2) ON CONFLICT(operation_id) DO NOTHING")
        .bind(&operation.id).bind(started_at).execute(&mut *tx).await?.rows_affected();
    if inserted != 1 {
        return Err(PaymentStoreError::Unavailable);
    }
    now(&clock)?;
    require_durable(&mut tx).await?;
    tx.commit().await?;
    Ok(ContinuationStart {
        operation,
        original: frozen.snapshot().clone(),
        started_at,
    })
}

mod execution;
#[cfg(test)]
pub(crate) use execution::record_fsync_test;
pub use execution::{ContinuationOutcome, execute, inspect};

#[cfg(test)]
#[path = "../../../../tests/support/continuation_operation_source.rs"]
mod tests;
