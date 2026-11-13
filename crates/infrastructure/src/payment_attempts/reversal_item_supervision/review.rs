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
    // The native checkpoint guarantees these at write time. Revalidate them
    // before projecting a historical review, including after a damaged import.
    let exact: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM reversal_item_operations o WHERE o.job_id=$1 AND o.generation=$2 AND o.outcome=$3 AND o.finished_at=$4 AND o.started_at >= $5 AND o.lease_until=o.started_at+60 AND o.start_xid IS NOT NULL AND o.finish_xid IS NOT NULL AND o.start_xid<>o.finish_xid AND o.receipt_id IS NULL AND (($3='uncertain' AND o.finished_at>=o.lease_until) OR ($3<>'uncertain' AND o.finished_at<o.lease_until)))")
        .bind(&job.id).bind(job.generation).bind(outcome).bind(job.updated_at).bind(job.created_at)
        .fetch_one(&mut **tx).await?;
    if !exact
        || job.generation == 0
        || (reason == Reason::RetryExhausted && (job.failures != 3 || job.generation != 3))
        || (reason != Reason::RetryExhausted && job.failures >= job.generation)
    {
        return Err(RecoveryError::InvalidStoredData);
    }
    if reason == Reason::ObservationLimit {
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM (SELECT 1 FROM payment_reversal_observations WHERE attempt_id=$1 LIMIT 128) bounded")
            .bind(&job.attempt_id).fetch_one(&mut **tx).await?;
        if count != 128 {
            return Err(RecoveryError::InvalidStoredData);
        }
    }
    let late_receipt_present = if reason == Reason::Uncertain {
        validate::late_receipt(tx, &job, &frozen, started)
            .await?
            .is_some()
    } else {
        false
    };
    Ok(Some(Summary {
        reason,
        generation: job.generation,
        late_receipt_present,
    }))
}
