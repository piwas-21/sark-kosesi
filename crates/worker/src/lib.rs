//! Bounded worker use cases with explicit clock inputs and redacted errors.
#![forbid(unsafe_code)]
use commerce_application::request_context::Clock;
use commerce_infrastructure::holds::{ExpiryBatch, expire_due_holds};
use sqlx::PgPool;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkerError {
    ClockUnavailable,
    ExpiryUnavailable,
}

pub async fn expire_batch(
    pool: &PgPool,
    clock: &dyn Clock,
    limit: u16,
) -> Result<ExpiryBatch, WorkerError> {
    let seconds = clock
        .unix_seconds()
        .map_err(|_| WorkerError::ClockUnavailable)?;
    let now = i64::try_from(seconds).map_err(|_| WorkerError::ClockUnavailable)?;
    expire_due_holds(pool, now, limit)
        .await
        .map_err(|_| WorkerError::ExpiryUnavailable)
}
