//! Staff-only exact review-stop inspection; no provider calls or recovery authority.
use super::*;
use crate::staff_access::{SessionToken, StaffError, lock_staff};
use commerce_application::{request_context::ObservedClock, staff_access::Capability};

#[derive(Debug, PartialEq, Eq)]
pub enum ReviewError {
    InvalidRequest,
    Denied,
    Unavailable,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    Uncertain,
    Invalid,
    RetryExhausted,
    ObservationLimit,
}
#[derive(Debug, PartialEq, Eq)]
pub struct Summary {
    pub reason: Reason,
    pub generation: i32,
    pub late_receipt_present: bool,
}
#[derive(sqlx::FromRow)]
struct Operation {
    id: String,
    generation: i32,
    started_at: i64,
    lease_until: i64,
    outcome: Option<String>,
    finished_at: Option<i64>,
    transactions_valid: bool,
    receipt_absent: bool,
}

fn staff_error(error: StaffError) -> ReviewError {
    match error {
        StaffError::Denied => ReviewError::Denied,
        _ => ReviewError::Unavailable,
    }
}

/// Validate exactly one review stop while holding fresh staff and attempt claims.
/// None means the requested job is absent or not in review. The result is detached
/// and grants no provider, financial, retry or state-transition authority.
pub async fn inspect(
    pool: &PgPool,
    token: &SessionToken,
    clock: &dyn Clock,
    id: &str,
) -> Result<Option<Summary>, ReviewError> {
    if !identifier(id) {
        return Err(ReviewError::InvalidRequest);
    }
    let observed = ObservedClock::new(clock, 0);
    let mut access = lock_staff(pool, token, &observed, Capability::ReversalReview)
        .await
        .map_err(staff_error)?;
    // Delay all state-dependent results until staff authority is rechecked.
    let result = validated(&mut access.tx, id).await;
    access.check_time(&observed).map_err(staff_error)?;
    let result = result.map_err(|_| ReviewError::Unavailable)?;
    access
        .tx
        .rollback()
        .await
        .map_err(|_| ReviewError::Unavailable)?;
    Ok(result)
}

async fn validated(
    tx: &mut Transaction<'_, Postgres>,
    id: &str,
) -> Result<Option<Summary>, RecoveryError> {
    let Some((job, frozen, started)) = inspected(tx, id).await? else {
        return Ok(None);
    };
    if job.state != "review" {
        return Ok(None);
    }
    let (reason, outcome) = match job.review_reason.as_deref() {
        Some("uncertain") => (Reason::Uncertain, "uncertain"),
        Some("invalid") => (Reason::Invalid, "invalid"),
        Some("retry_exhausted") => (Reason::RetryExhausted, "transient"),
        Some("observation_limit") => (Reason::ObservationLimit, "capped"),
        _ => return Err(RecoveryError::InvalidStoredData),
    };
    // Reconstruct the entire bounded history, not just its terminal checkpoint.
    let history: Vec<Operation> = sqlx::query_as("SELECT left(id,33) id,generation,started_at,lease_until,left(outcome,20) outcome,finished_at,COALESCE(start_xid IS NOT NULL AND finish_xid IS NOT NULL AND start_xid<>finish_xid,false) transactions_valid,receipt_id IS NULL AS receipt_absent FROM reversal_item_operations WHERE job_id=$1 ORDER BY generation LIMIT 4")
        .bind(&job.id).fetch_all(&mut **tx).await?;
    if job.generation == 0 || history.len() != job.generation as usize {
        return Err(RecoveryError::InvalidStoredData);
    }
    let expected_failures = if reason == Reason::RetryExhausted {
        job.generation
    } else {
        job.generation - 1
    };
    if job.failures != expected_failures
        || (reason == Reason::RetryExhausted && job.generation != 3)
    {
        return Err(RecoveryError::InvalidStoredData);
    }
    let mut previous_finished = None;
    for (index, operation) in history.iter().enumerate() {
        let last = index + 1 == history.len();
        let expected_outcome = if last { outcome } else { "transient" };
        let finished = operation
            .finished_at
            .ok_or(RecoveryError::InvalidStoredData)?;
        if !identifier(&operation.id)
            || operation.generation != (index + 1) as i32
            || operation.started_at < job.created_at
            || previous_finished.is_some_and(|previous: i64| {
                previous
                    .checked_add(60)
                    .is_none_or(|due| operation.started_at < due)
            })
            || operation.started_at.checked_add(60) != Some(operation.lease_until)
            || finished < operation.started_at
            || finished > 9_007_199_254_740_991
            || operation.outcome.as_deref() != Some(expected_outcome)
            || !operation.transactions_valid
            || !operation.receipt_absent
            || (expected_outcome == "uncertain" && finished < operation.lease_until)
            || (expected_outcome != "uncertain" && finished >= operation.lease_until)
            || (last && finished != job.updated_at)
        {
            return Err(RecoveryError::InvalidStoredData);
        }
        previous_finished = Some(finished);
    }
    let (count, _) = super::super::reversals::rows::history(tx, &job.attempt_id).await?;
    if count > 128 || (reason == Reason::ObservationLimit && count != 128) {
        return Err(RecoveryError::InvalidStoredData);
    }
    let mut late_receipt_present = false;
    for operation in &history {
        late_receipt_present |= validate::exact_receipt(
            tx,
            &job,
            &frozen,
            started,
            &operation.id,
            operation.started_at,
        )
        .await?
        .is_some();
    }
    Ok(Some(Summary {
        reason,
        generation: job.generation,
        late_receipt_present,
    }))
}
