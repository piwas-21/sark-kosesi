use super::*;
#[derive(sqlx::FromRow)]
struct Row {
    attempt_id: String,
    source_event_id: String,
    created_at: i64,
    updated_at: i64,
    due_at: i64,
    generation: i64,
    lease_until: Option<i64>,
    phase: String,
    page_id: Option<String>,
    candidate_index: i32,
    operation: i64,
    polls: i32,
    failures: i32,
    review_reason: Option<String>,
    source_present: bool,
}
pub(crate) async fn load(
    tx: &mut Transaction<'_, Postgres>,
    id: &str,
) -> Result<Option<Job>, RecoveryError> {
    let row=sqlx::query_as::<_,Row>(r#"SELECT left(j.attempt_id,33) AS attempt_id,left(source_event_id,33) AS source_event_id,
 j.created_at,j.updated_at,due_at,generation,lease_until,left(phase,32) AS phase,left(page_id,33) AS page_id,
 candidate_index,operation,polls,failures,left(review_reason,32) AS review_reason,
 EXISTS(SELECT 1 FROM commerce_outbox e JOIN payment_dispatch_starts s ON s.attempt_id=j.attempt_id JOIN payment_attempts a ON a.id=s.attempt_id
 WHERE e.id=j.source_event_id AND e.kind='payment.reconciliation_required.v1' AND e.aggregate_id=a.order_id AND e.created_at=s.started_at AND j.created_at=s.started_at
 AND e.payload=jsonb_build_object('schema_version',1,'order_id',a.order_id,'attempt_id',a.id,'dispatch_id',s.id)) AS source_present
 FROM payment_reconciliation_jobs j WHERE j.attempt_id=$1"#).bind(id).fetch_optional(&mut **tx).await?;
    row.map(Row::decode).transpose()
}
impl Row {
    fn decode(self) -> Result<Job, RecoveryError> {
        let invalid = || RecoveryError::InvalidStoredData;
        let phase: Phase =
            serde_json::from_value(serde_json::Value::String(self.phase)).map_err(|_| invalid())?;
        let review_reason: Option<ReviewReason> = self
            .review_reason
            .map(|v| serde_json::from_value(serde_json::Value::String(v)))
            .transpose()
            .map_err(|_| invalid())?;
        if !self.source_present
            || !identifier(&self.attempt_id)
            || !identifier(&self.source_event_id)
            || !(0..=9_007_199_254_740_991).contains(&self.created_at)
            || !(self.created_at..=9_007_199_254_740_991).contains(&self.updated_at)
            || !(self.created_at..=9_007_199_254_740_991).contains(&self.due_at)
            || !(0..=1500).contains(&self.generation)
            || self.lease_until.is_some_and(|v| {
                self.generation == 0
                    || v <= self.updated_at
                    || v > self.updated_at + 60
                    || v > 9_007_199_254_740_991
            })
            || self.page_id.as_ref().is_some_and(|v| !identifier(v))
            || !(0..=10).contains(&self.candidate_index)
            || !(0..=1500).contains(&self.operation)
            || self.operation > self.generation
            || !(0..=10).contains(&self.polls)
            || (self.polls == 10 && phase != Phase::NeedsReview)
            || !(0..=5).contains(&self.failures)
            || (self.failures == 5 && phase != Phase::NeedsReview)
            || (phase == Phase::NeedsReview) != review_reason.is_some()
            || (matches!(phase, Phase::Candidates | Phase::Poll) && self.page_id.is_none())
            || (phase == Phase::NeedsReview && self.lease_until.is_some())
        {
            return Err(invalid());
        }
        Ok(Job {
            attempt_id: self.attempt_id,
            source_event_id: self.source_event_id,
            created_at: self.created_at,
            updated_at: self.updated_at,
            due_at: self.due_at,
            generation: self.generation,
            lease_until: self.lease_until,
            phase,
            page_id: self.page_id,
            candidate_index: self.candidate_index,
            operation: self.operation,
            polls: self.polls,
            failures: self.failures,
            review_reason,
        })
    }
}
pub(crate) async fn enqueue(
    pool: &PgPool,
    clock: &dyn Clock,
    profile: &str,
) -> Result<(), RecoveryError> {
    let mut tx = pool.begin().await?;
    limits(&mut tx).await?;
    let at = now(clock)?;
    sqlx::query(r#"INSERT INTO payment_reconciliation_jobs(attempt_id,source_event_id,created_at,updated_at,due_at)
 SELECT a.id,e.id,s.started_at,$1,$1 FROM payment_attempts a JOIN payment_dispatch_starts s ON s.attempt_id=a.id
 JOIN commerce_outbox e ON e.aggregate_id=a.order_id AND e.kind='payment.reconciliation_required.v1'
 WHERE a.snapshot->'binding'->'profile'->>'id'=$2 AND s.started_at<=$1
 AND e.created_at=s.started_at AND e.payload=jsonb_build_object('schema_version',1,'order_id',a.order_id,'attempt_id',a.id,'dispatch_id',s.id)
 AND NOT EXISTS(SELECT 1 FROM payment_reconciliation_jobs j WHERE j.attempt_id=a.id)
 ORDER BY s.started_at,a.id LIMIT 100 ON CONFLICT(attempt_id) DO NOTHING"#).bind(at).bind(profile).execute(&mut *tx).await?;
    if now(clock)? < at {
        return Err(RecoveryError::ClockUnavailable);
    }
    tx.commit().await?;
    Ok(())
}
pub(crate) async fn sweep(
    pool: &PgPool,
    clock: &dyn Clock,
    profile: &str,
) -> Result<u16, RecoveryError> {
    let mut tx = pool.begin().await?;
    limits(&mut tx).await?;
    let at = now(clock)?;
    let changed=sqlx::query(r#"WITH stop AS (SELECT j.attempt_id FROM payment_reconciliation_jobs j JOIN payment_attempts a ON a.id=j.attempt_id
 WHERE j.phase<>'needs_review' AND j.generation=1500 AND j.updated_at<=$1 AND (j.lease_until IS NULL OR j.lease_until<=$1)
 AND a.snapshot->'binding'->'profile'->>'id'=$2 ORDER BY j.attempt_id FOR UPDATE OF j SKIP LOCKED LIMIT 25)
 UPDATE payment_reconciliation_jobs j SET phase='needs_review',review_reason='step_limit',lease_until=NULL,updated_at=$1
 FROM stop WHERE j.attempt_id=stop.attempt_id"#).bind(at).bind(profile).execute(&mut *tx).await?.rows_affected();
    if now(clock)? < at {
        return Err(RecoveryError::ClockUnavailable);
    }
    tx.commit().await?;
    u16::try_from(changed).map_err(|_| RecoveryError::InvalidStoredData)
}
pub(crate) async fn claim(
    pool: &PgPool,
    clock: &dyn Clock,
    profile: &str,
) -> Result<Option<Claim>, RecoveryError> {
    let mut tx = pool.begin().await?;
    limits(&mut tx).await?;
    let at = now(clock)?;
    let until = at
        .checked_add(60)
        .filter(|v| *v <= 9_007_199_254_740_991)
        .ok_or(RecoveryError::ClockUnavailable)?;
    let id:Option<String>=sqlx::query_scalar(r#"SELECT j.attempt_id FROM payment_reconciliation_jobs j JOIN payment_attempts a ON a.id=j.attempt_id
 WHERE a.snapshot->'binding'->'profile'->>'id'=$1 AND j.phase<>'needs_review' AND j.generation<1500
 AND j.due_at<=$2 AND j.updated_at<=$2 AND (j.lease_until IS NULL OR j.lease_until<=$2)
 ORDER BY j.due_at,j.attempt_id FOR UPDATE OF j SKIP LOCKED LIMIT 1"#).bind(profile).bind(at).fetch_optional(&mut *tx).await?;
    let Some(id) = id else {
        tx.rollback().await?;
        return Ok(None);
    };
    let job = load(&mut tx, &id)
        .await?
        .ok_or(RecoveryError::InvalidStoredData)?;
    sqlx::query("UPDATE payment_reconciliation_jobs SET generation=generation+1,lease_until=$1,updated_at=$2 WHERE attempt_id=$3").bind(until).bind(at).bind(&id).execute(&mut *tx).await?;
    let final_at = now(clock)?;
    if final_at < at || final_at >= until {
        return Err(RecoveryError::ClockUnavailable);
    }
    tx.commit().await?;
    let mut job = job;
    job.generation += 1;
    job.lease_until = Some(until);
    job.updated_at = at;
    Ok(Some(Claim { job, until }))
}
pub(crate) async fn finish(
    pool: &PgPool,
    clock: &dyn Clock,
    claim: &Claim,
    next: &Job,
) -> Result<(), RecoveryError> {
    let mut tx = pool.begin().await?;
    limits(&mut tx).await?;
    let at = now(clock)?;
    if at < claim.job.updated_at {
        return Err(RecoveryError::ClockUnavailable);
    }
    let delay = if next.phase == Phase::NeedsReview {
        0
    } else if next.failures > 0 {
        60i64
            .checked_shl(
                u32::try_from(next.failures - 1).map_err(|_| RecoveryError::InvalidStoredData)?,
            )
            .ok_or(RecoveryError::InvalidStoredData)?
    } else if next.phase == Phase::Poll {
        60
    } else {
        0
    };
    let due = at
        .checked_add(delay)
        .filter(|v| *v <= 9_007_199_254_740_991)
        .ok_or(RecoveryError::ClockUnavailable)?;
    let phase = serde_json::to_value(next.phase).map_err(|_| RecoveryError::InvalidStoredData)?;
    let reason =
        serde_json::to_value(&next.review_reason).map_err(|_| RecoveryError::InvalidStoredData)?;
    let changed=sqlx::query(r#"UPDATE payment_reconciliation_jobs SET phase=$1,page_id=$2,candidate_index=$3,
 operation=operation+1,polls=$4,failures=$5,review_reason=$6,lease_until=NULL,updated_at=$7,due_at=$8
 WHERE attempt_id=$9 AND generation=$10 AND lease_until=$11 AND lease_until>$7"#)
 .bind(phase.as_str()).bind(&next.page_id).bind(next.candidate_index).bind(next.polls).bind(next.failures).bind(reason.as_str()).bind(at).bind(due).bind(&claim.job.attempt_id).bind(claim.job.generation).bind(claim.until).execute(&mut *tx).await?.rows_affected();
    if changed != 1 {
        return Err(RecoveryError::LeaseLost);
    }
    let final_at = now(clock)?;
    if final_at < at || final_at >= claim.until {
        return Err(RecoveryError::LeaseLost);
    }
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
#[path = "../../../tests/support/reconciliation_rows.rs"]
mod tests;
