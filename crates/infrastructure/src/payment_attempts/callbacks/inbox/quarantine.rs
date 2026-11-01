//! Source-only quarantine for an immutable disposition whose financial proof broke.
//! Original dispositions and payment facts remain untouched; no capability is rebuilt.
use super::drain::{self, Disposition};
use super::*;
/// Missing referenced financial history is deterministic invalid custody. Storage
/// timeouts/connections and clock failures retain their retryable classification.
pub(super) fn proof_error(error: PaymentStoreError) -> PaymentStoreError {
    match error {
        PaymentStoreError::NotFound
        | PaymentStoreError::Conflict
        | PaymentStoreError::Unavailable => PaymentStoreError::InvalidStoredData,
        error => error,
    }
}
pub(crate) async fn read(pool: &PgPool, operation: &str) -> Result<bool, PaymentStoreError> {
    if !identifier(operation) {
        return Err(PaymentStoreError::InvalidRequest);
    }
    let recorded: Option<i64> = sqlx::query_scalar(
        "SELECT recorded_at FROM callback_processing_quarantine WHERE operation_id=$1",
    )
    .bind(operation)
    .fetch_optional(pool)
    .await?;
    let Some(recorded) = recorded else {
        return Ok(false);
    };
    let source_at = processing_source::identity(pool, operation).await?;
    let original_present:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM callback_receipt_processing p JOIN callback_inbox_generations g ON g.operation_id=p.operation_id JOIN callback_inbox_jobs j ON j.intake_id=g.intake_id JOIN callback_inbox_active a ON a.intake_id=j.intake_id JOIN callback_intakes i ON i.id=j.intake_id WHERE p.operation_id=$1 AND NOT j.settled AND a.profile_id=i.profile_id AND a.created_at=i.received_at)")
        .bind(operation).fetch_one(pool).await?;
    if !original_present || !(source_at..=9_007_199_254_740_991).contains(&recorded) {
        return Err(PaymentStoreError::InvalidStoredData);
    }
    Ok(true)
}
/// Supervisors use this checked read so a damaged saved proof cannot hold up
/// unrelated safe work. The exception never overwrites the original result.
pub(crate) async fn checked_disposition(
    pool: &PgPool,
    clock: &dyn Clock,
    operation: &str,
) -> Result<Option<Disposition>, PaymentStoreError> {
    match drain::read_disposition(pool, operation).await {
        Err(PaymentStoreError::InvalidStoredData) => {
            persist(pool, clock, operation).await?;
            Ok(Some(Disposition::CustodyInvalid))
        }
        outcome => outcome,
    }
}
pub(crate) async fn persist(
    pool: &PgPool,
    clock: &dyn Clock,
    operation: &str,
) -> Result<(), PaymentStoreError> {
    if !identifier(operation) {
        return Err(PaymentStoreError::InvalidRequest);
    }
    let source_at = processing_source::identity(pool, operation).await?;
    let lower = match lookup::read_lookup(pool, operation).await {
        Ok(Some(receipt)) => receipt.snapshot.recorded_at,
        Err(PaymentStoreError::InvalidStoredData) => source_at,
        Ok(None) => return Err(PaymentStoreError::InvalidStoredData),
        Err(error) => return Err(error),
    };
    let initial = clock
        .unix_seconds()
        .map_err(|_| PaymentStoreError::ClockUnavailable)?;
    let clock = ObservedClock::new(clock, initial);
    let mut tx = pool.begin().await?;
    limits(&mut tx).await?;
    let at = now(&clock)?;
    if at < lower {
        return Err(PaymentStoreError::ClockUnavailable);
    }
    sqlx::query("INSERT INTO callback_processing_quarantine(operation_id,recorded_at) VALUES($1,$2) ON CONFLICT(operation_id) DO NOTHING")
        .bind(operation).bind(at).execute(&mut *tx).await?;
    now(&clock)?;
    tx.commit().await?;
    if !read(pool, operation).await? {
        return Err(PaymentStoreError::InvalidStoredData);
    }
    Ok(())
}

/// At most the fixed active capacity times five generations are inspected. No
/// job lock is held while a bad saved result becomes a new immutable quarantine.
pub(crate) async fn inspect_profile(
    pool: &PgPool,
    clock: &dyn Clock,
    profile: &str,
) -> Result<(), PaymentStoreError> {
    active::audit(pool).await?;
    let operations:Vec<String>=sqlx::query_scalar("SELECT left(g.operation_id,33) FROM callback_inbox_active a JOIN callback_inbox_generations g ON g.intake_id=a.intake_id JOIN callback_receipt_processing p ON p.operation_id=g.operation_id WHERE a.profile_id=$1 ORDER BY g.intake_id,g.generation LIMIT 1001")
        .bind(profile).fetch_all(pool).await?;
    for operation in bounded_operations(operations)? {
        checked_disposition(pool, clock, &operation).await?;
    }
    Ok(())
}
fn bounded_operations(operations: Vec<String>) -> Result<Vec<String>, PaymentStoreError> {
    if operations.len() > 1000 || operations.iter().any(|v| !identifier(v)) {
        return Err(PaymentStoreError::InvalidStoredData);
    }
    Ok(operations)
}

#[cfg(test)]
#[path = "../../../../tests/support/callback_inbox_quarantine_internal.rs"]
pub(crate) mod tests;
