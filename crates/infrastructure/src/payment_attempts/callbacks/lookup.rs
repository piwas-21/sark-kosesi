//! Durable pre-I/O operations and immutable authenticated lookup receipts.
use super::super::{
    PaymentStoreError,
    evidence::{fresh_id, limits, now},
    identifier,
};
use crate::mollie::{
    CallbackLookupPayment, CallbackPayment, MollieError, VerifiedReconciliationClient,
};
use commerce_application::request_context::{Clock, Identifiers, ObservedClock};
use commerce_domain::payment_attempts::callbacks::{LookupOperationSnapshot, LookupSnapshot};
use sqlx::PgPool;
mod rows;
/// Constructed only after an operation commits or its durable row is revalidated.
pub struct LookupOperation {
    snapshot: LookupOperationSnapshot,
}
impl LookupOperation {
    pub fn snapshot(&self) -> &LookupOperationSnapshot {
        &self.snapshot
    }
}
/// Unique committed GET-start capability. It cannot be cloned or restored.
/// A lost permit or uncertain GET leaves this operation visibly unresolved;
/// another GET requires a fresh operation, never another start of this one.
pub struct LookupFetchPermit {
    operation: LookupOperationSnapshot,
    started_at: i64,
}
impl LookupFetchPermit {
    pub(crate) fn into_parts(self) -> (LookupOperationSnapshot, i64) {
        (self.operation, self.started_at)
    }
}
pub async fn start_lookup(
    pool: &PgPool,
    clock: &dyn Clock,
    operation: &LookupOperation,
) -> Result<LookupFetchPermit, PaymentStoreError> {
    let initial = clock
        .unix_seconds()
        .map_err(|_| PaymentStoreError::ClockUnavailable)?;
    let clock = ObservedClock::new(clock, initial);
    let mut tx = pool.begin().await?;
    limits(&mut tx).await?;
    let stored = rows::operation(&mut tx, &operation.snapshot.id, true).await?;
    if stored != operation.snapshot {
        return Err(PaymentStoreError::InvalidStoredData);
    }
    let started_at = now(&clock)?;
    if started_at < stored.created_at {
        return Err(PaymentStoreError::ClockUnavailable);
    }
    let inserted = sqlx::query("INSERT INTO callback_lookup_fetch_starts(operation_id,started_at) VALUES($1,$2) ON CONFLICT(operation_id) DO NOTHING")
        .bind(&stored.id).bind(started_at).execute(&mut *tx).await?.rows_affected();
    if inserted != 1 {
        return Err(PaymentStoreError::Unavailable);
    }
    now(&clock)?;
    tx.commit().await?;
    Ok(LookupFetchPermit {
        operation: stored,
        started_at,
    })
}
/// The inbox can atomically persist a generation and one-use GET start. This
/// boundary validates the durable start, commits, and only then yields a permit.
pub(crate) async fn commit_started(
    mut tx: sqlx::Transaction<'_, sqlx::Postgres>,
    clock: &dyn Clock,
    operation: LookupOperationSnapshot,
    started_at: i64,
    until: i64,
) -> Result<LookupFetchPermit, PaymentStoreError> {
    let stored = rows::operation(&mut tx, &operation.id, true).await?;
    let start: Option<i64> = sqlx::query_scalar(
        "SELECT started_at FROM callback_lookup_fetch_starts WHERE operation_id=$1",
    )
    .bind(&operation.id)
    .fetch_optional(&mut *tx)
    .await?;
    if stored != operation || start != Some(started_at) {
        return Err(PaymentStoreError::InvalidStoredData);
    }
    let at = now(clock)?;
    if at < started_at || at >= until {
        return Err(PaymentStoreError::Conflict);
    }
    tx.commit().await?;
    Ok(LookupFetchPermit {
        operation,
        started_at,
    })
}
/// A historical fetched fact, not order/payment/stock authority. No deserialization.
pub struct LookupReceipt {
    pub snapshot: LookupSnapshot,
    pub replayed: bool,
    fetched: CallbackPayment,
}
pub async fn prepare_lookup(
    pool: &PgPool,
    clock: &dyn Clock,
    ids: &dyn Identifiers,
    profile_id: &str,
    provider_id: &str,
) -> Result<LookupOperation, PaymentStoreError> {
    let initial = clock
        .unix_seconds()
        .map_err(|_| PaymentStoreError::ClockUnavailable)?;
    let clock = ObservedClock::new(clock, initial);
    let mut tx = pool.begin().await?;
    limits(&mut tx).await?;
    let snapshot = prepare_in_transaction(&mut tx, &clock, ids, profile_id, provider_id).await?;
    now(&clock)?;
    tx.commit().await?;
    Ok(LookupOperation { snapshot })
}
/// Compose an inbox generation and operation in one caller-owned transaction.
/// The returned snapshot is data only: no usable operation/GET permit escapes.
/// Callers obtain the opaque operation by re-reading only after committing.
pub(crate) async fn prepare_in_transaction(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    clock: &dyn Clock,
    ids: &dyn Identifiers,
    profile_id: &str,
    provider_id: &str,
) -> Result<LookupOperationSnapshot, PaymentStoreError> {
    let snapshot = LookupOperationSnapshot {
        schema_version: 1,
        id: fresh_id(ids)?,
        profile_id: profile_id.into(),
        provider_id: provider_id.into(),
        created_at: now(clock)?,
    };
    snapshot
        .validate()
        .map_err(|_| PaymentStoreError::InvalidRequest)?;
    sqlx::query("INSERT INTO callback_lookup_operations(id,profile_id,provider_id,created_at) VALUES($1,$2,$3,$4)")
        .bind(&snapshot.id).bind(&snapshot.profile_id).bind(&snapshot.provider_id).bind(snapshot.created_at)
        .execute(&mut **tx).await?;
    Ok(snapshot)
}

pub async fn read_operation(pool: &PgPool, id: &str) -> Result<LookupOperation, PaymentStoreError> {
    if !identifier(id) {
        return Err(PaymentStoreError::InvalidRequest);
    }
    let mut tx = pool.begin().await?;
    limits(&mut tx).await?;
    let snapshot = rows::operation(&mut tx, id, false).await?;
    tx.rollback().await?;
    Ok(LookupOperation { snapshot })
}
/// Only a fetched capability bound to an already committed operation can enter.
pub async fn record_lookup(
    pool: &PgPool,
    clock: &dyn Clock,
    ids: &dyn Identifiers,
    operation_id: &str,
    fetched: &CallbackLookupPayment,
) -> Result<LookupReceipt, PaymentStoreError> {
    if !identifier(operation_id) {
        return Err(PaymentStoreError::InvalidRequest);
    }
    let mut tx = pool.begin().await?;
    limits(&mut tx).await?;
    let operation = rows::operation(&mut tx, operation_id, true).await?;
    if fetched.operation() != &operation {
        return Err(PaymentStoreError::Conflict);
    }
    if let Some(previous) = rows::receipt(&mut tx, &operation).await? {
        if !fetched.matches(&previous.snapshot) {
            return Err(PaymentStoreError::Conflict);
        }
        tx.rollback().await?;
        return Ok(previous);
    }
    let initial = clock
        .unix_seconds()
        .map_err(|_| PaymentStoreError::ClockUnavailable)?;
    let clock = ObservedClock::new(clock, initial);
    let snapshot = fetched.snapshot(fresh_id(ids)?, now(&clock)?);
    snapshot
        .validate()
        .map_err(|_| PaymentStoreError::Conflict)?;
    rows::validate_start(&mut tx, &snapshot).await?;
    rows::insert(&mut tx, &snapshot, &fresh_id(ids)?).await?;
    now(&clock)?;
    tx.commit().await?;
    Ok(LookupReceipt {
        fetched: fetched.copy_payment(),
        snapshot,
        replayed: false,
    })
}
pub async fn read_lookup(
    pool: &PgPool,
    operation_id: &str,
) -> Result<Option<LookupReceipt>, PaymentStoreError> {
    if !identifier(operation_id) {
        return Err(PaymentStoreError::InvalidRequest);
    }
    let mut tx = pool.begin().await?;
    limits(&mut tx).await?;
    let operation = rows::operation(&mut tx, operation_id, false).await?;
    let receipt = rows::receipt(&mut tx, &operation).await?;
    tx.rollback().await?;
    Ok(receipt)
}
/// Recovery always reads committed custody before another GET. No DB transaction
/// spans network I/O. Parallel first fetches cannot overwrite the winning receipt.
pub async fn fetch_lookup(
    pool: &PgPool,
    clock: &dyn Clock,
    ids: &dyn Identifiers,
    operation_id: &str,
    client: &VerifiedReconciliationClient,
) -> Result<LookupReceipt, MollieError> {
    let operation = read_operation(pool, operation_id)
        .await
        .map_err(MollieError::Custody)?;
    if operation.snapshot.profile_id != client.profile_id() {
        return Err(MollieError::ProfileMismatch);
    }
    if let Some(receipt) = read_lookup(pool, operation_id)
        .await
        .map_err(MollieError::Custody)?
    {
        return Ok(receipt);
    }
    let permit = start_lookup(pool, clock, &operation)
        .await
        .map_err(MollieError::Custody)?;
    let fetched = client.fetch_lookup(permit).await?;
    record_lookup(pool, clock, ids, operation_id, &fetched)
        .await
        .map_err(MollieError::Custody)
}
mod process;
pub use process::process_lookup;

#[cfg(test)]
#[path = "../../../tests/support/payment_callback_lookup_internal.rs"]
mod tests;
