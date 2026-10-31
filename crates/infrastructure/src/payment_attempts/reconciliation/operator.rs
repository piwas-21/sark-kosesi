//! Private finance decision: resume exhausted transient GETs, never a new charge.
use super::*;
use crate::staff_access::{SessionToken, StaffError, lock_staff};
use commerce_application::staff_access::Capability;

pub struct ResumeRequest<'a> {
    pub attempt_id: &'a str,
    pub profile_id: &'a str,
    pub expected_generation: i64,
    pub effect_key: &'a str,
    pub reason: &'a str,
    pub confirmed: bool,
}
#[derive(Debug, PartialEq, Eq)]
pub enum OperatorError {
    Denied,
    InvalidRequest,
    Conflict,
    NotFound,
    InvalidStoredData,
    ClockUnavailable,
    Storage,
}
impl From<StaffError> for OperatorError {
    fn from(v: StaffError) -> Self {
        match v {
            StaffError::Denied => Self::Denied,
            StaffError::ClockUnavailable => Self::ClockUnavailable,
            _ => Self::Storage,
        }
    }
}
impl From<sqlx::Error> for OperatorError {
    fn from(_: sqlx::Error) -> Self {
        Self::Storage
    }
}
impl From<RecoveryError> for OperatorError {
    fn from(v: RecoveryError) -> Self {
        match v {
            RecoveryError::ClockUnavailable => Self::ClockUnavailable,
            RecoveryError::Storage => Self::Storage,
            _ => Self::InvalidStoredData,
        }
    }
}
pub struct ResumeReceipt {
    pub revision: i32,
    pub before: Job,
    pub after: Job,
    pub replayed: bool,
}
fn text(s: &str) -> bool {
    !s.trim().is_empty() && s.chars().count() <= 500 && !s.chars().any(char::is_control)
}
fn resumed(before: &Job, at: i64) -> Result<Job, OperatorError> {
    if before.phase != Phase::NeedsReview
        || before.review_reason != Some(ReviewReason::RetryExhausted)
        || before.failures != 5
        || before.polls >= 10
        || before.generation >= 1500
        || at < before.updated_at
        || at > 9_007_199_254_740_991
    {
        return Err(OperatorError::Conflict);
    }
    let mut after = before.clone();
    after.phase = if before.page_id.is_none() {
        Phase::Search
    } else {
        Phase::Candidates
    };
    after.failures = 0;
    after.review_reason = None;
    after.updated_at = at;
    after.due_at = at;
    Ok(after)
}
#[derive(sqlx::FromRow)]
struct Stored {
    attempt_id: String,
    revision: i32,
    actor: String,
    reason: String,
    profile_id: String,
    before_state: Option<serde_json::Value>,
    after_state: Option<serde_json::Value>,
}
impl Stored {
    fn decode(self) -> Result<ResumeReceipt, OperatorError> {
        let invalid = || OperatorError::InvalidStoredData;
        let before: Job = serde_json::from_value(self.before_state.ok_or_else(invalid)?)
            .map_err(|_| invalid())?;
        let after: Job =
            serde_json::from_value(self.after_state.ok_or_else(invalid)?).map_err(|_| invalid())?;
        jobs::validate(&before)?;
        jobs::validate(&after)?;
        let expected = resumed(&before, after.updated_at).map_err(|_| invalid())?;
        if !(1..=3).contains(&self.revision)
            || before.attempt_id != self.attempt_id
            || serde_json::to_value(expected).map_err(|_| invalid())?
                != serde_json::to_value(&after).map_err(|_| invalid())?
        {
            return Err(invalid());
        }
        Ok(ResumeReceipt {
            revision: self.revision,
            before,
            after,
            replayed: true,
        })
    }
}
/// No HTTP route or staff grant is installed. Fresh session/MFA is required on
/// every call, including historical retries. Existing global work budgets survive.
pub async fn resume_transient(
    pool: &PgPool,
    token: &SessionToken,
    clock: &dyn Clock,
    request: ResumeRequest<'_>,
) -> Result<ResumeReceipt, OperatorError> {
    let observed = ObservedClock::new(clock, 0);
    let clock: &dyn Clock = &observed;
    let mut access = lock_staff(pool, token, clock, Capability::PaymentReconcile).await?;
    if !identifier(request.attempt_id)
        || !valid_command_id(request.effect_key)
        || !(0..1500).contains(&request.expected_generation)
        || !text(request.reason)
        || !text(request.profile_id)
        || !request.confirmed
    {
        return Err(OperatorError::InvalidRequest);
    }
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!("payment-recovery-resume:{}", request.effect_key))
        .execute(&mut *access.tx)
        .await?;
    let exists: Option<String> = sqlx::query_scalar(
        "SELECT attempt_id FROM payment_reconciliation_jobs WHERE attempt_id=$1 FOR UPDATE",
    )
    .bind(request.attempt_id)
    .fetch_optional(&mut *access.tx)
    .await?;
    exists.ok_or(OperatorError::NotFound)?;
    let current = jobs::load(&mut access.tx, request.attempt_id)
        .await?
        .ok_or(OperatorError::InvalidStoredData)?;
    let (frozen, _) = context(&mut access.tx, request.attempt_id, false)
        .await
        .map_err(|_| OperatorError::InvalidStoredData)?;
    if frozen.snapshot().binding.profile.id != request.profile_id {
        return Err(OperatorError::Conflict);
    }
    let prior = sqlx::query_as::<_, Stored>("SELECT left(attempt_id,33) AS attempt_id,revision,left(actor,129) AS actor,left(reason,501) AS reason,left(profile_id,129) AS profile_id,CASE WHEN octet_length(before_state::text)<=4096 THEN before_state END AS before_state,CASE WHEN octet_length(after_state::text)<=4096 THEN after_state END AS after_state FROM payment_recovery_resumes WHERE effect_key=$1")
        .bind(request.effect_key).fetch_optional(&mut *access.tx).await?;
    if let Some(prior) = prior {
        if prior.attempt_id != request.attempt_id
            || prior.actor != access.actor
            || prior.reason != request.reason
            || prior.profile_id != request.profile_id
        {
            return Err(OperatorError::Conflict);
        }
        let receipt = prior.decode()?;
        if receipt.before.source_event_id != current.source_event_id
            || receipt.before.created_at != current.created_at
            || receipt.after.generation > current.generation
            || receipt.after.updated_at > current.updated_at
        {
            return Err(OperatorError::InvalidStoredData);
        }
        if receipt.before.generation != request.expected_generation {
            return Err(OperatorError::Conflict);
        }
        access.check_time(clock)?;
        access.tx.rollback().await?;
        return Ok(receipt);
    }
    if current.generation != request.expected_generation {
        return Err(OperatorError::Conflict);
    }
    let revision: i32 = sqlx::query_scalar(
        "SELECT coalesce(max(revision),0)+1 FROM payment_recovery_resumes WHERE attempt_id=$1",
    )
    .bind(request.attempt_id)
    .fetch_one(&mut *access.tx)
    .await?;
    if revision > 3 {
        return Err(OperatorError::Conflict);
    }
    let at = access.authorized_at(clock)?;
    let after = resumed(&current, at)?;
    let before_state =
        serde_json::to_value(&current).map_err(|_| OperatorError::InvalidStoredData)?;
    let after_state = serde_json::to_value(&after).map_err(|_| OperatorError::InvalidStoredData)?;
    sqlx::query("INSERT INTO payment_recovery_resumes(attempt_id,revision,effect_key,actor,reason,profile_id,before_state,after_state) VALUES($1,$2,$3,$4,$5,$6,$7,$8)")
        .bind(request.attempt_id).bind(revision).bind(request.effect_key).bind(&access.actor)
        .bind(request.reason).bind(request.profile_id).bind(before_state).bind(after_state)
        .execute(&mut *access.tx).await?;
    sqlx::query("UPDATE payment_reconciliation_jobs SET phase=$1,failures=0,review_reason=NULL,updated_at=$2,due_at=$2 WHERE attempt_id=$3")
        .bind(if after.phase==Phase::Search { "search" } else { "candidates" })
        .bind(at).bind(request.attempt_id).execute(&mut *access.tx).await?;
    access.check_time(clock)?;
    access.tx.commit().await?;
    Ok(ResumeReceipt {
        revision,
        before: current,
        after,
        replayed: false,
    })
}

#[cfg(test)]
#[path = "../../../tests/support/payment_operator_rows.rs"]
mod tests;
