use super::*;
use commerce_domain::payment_attempts::callback_inbox::{
    LEASE_SECONDS, MAX_GENERATIONS, retry_delay,
};
pub(crate) struct Claim {
    pub(crate) job: CallbackJob,
    pub(crate) operation_id: String,
    pub(crate) until: i64,
    permit: Option<lookup::LookupFetchPermit>,
}
impl Claim {
    pub(crate) fn take_permit(&mut self) -> Result<lookup::LookupFetchPermit, PaymentStoreError> {
        self.permit.take().ok_or(PaymentStoreError::Unavailable)
    }
}
pub(crate) async fn claim(
    pool: &PgPool,
    clock: &dyn Clock,
    ids: &dyn Identifiers,
    profile: &str,
) -> Result<Option<Claim>, PaymentStoreError> {
    let initial_clock = clock
        .unix_seconds()
        .map_err(|_| PaymentStoreError::ClockUnavailable)?;
    let observed = ObservedClock::new(clock, initial_clock);
    let clock: &dyn Clock = &observed;
    active::audit(pool).await?;
    let mut tx = pool.begin().await?;
    limits(&mut tx).await?;
    let initial = now(clock)?;
    let id:Option<String>=sqlx::query_scalar(r#"SELECT j.intake_id FROM callback_inbox_active a JOIN callback_inbox_jobs j ON j.intake_id=a.intake_id JOIN callback_intakes i ON i.id=j.intake_id
      WHERE a.profile_id=$1 AND j.state IN ('pending','leased') AND j.generation<5 AND j.due_at<=$2 AND j.updated_at<=$2
      AND (j.lease_until IS NULL OR j.lease_until<=$2) AND NOT EXISTS(SELECT 1 FROM callback_inbox_generations old JOIN callback_lookup_receipts r ON r.operation_id=old.operation_id WHERE old.intake_id=j.intake_id AND old.generation=j.generation) ORDER BY j.due_at,j.intake_id FOR UPDATE OF j SKIP LOCKED LIMIT 1"#)
      .bind(profile).bind(initial).fetch_optional(&mut *tx).await?;
    let Some(id) = id else {
        tx.rollback().await?;
        return Ok(None);
    };
    let mut job = rows::load(&mut tx, &id)
        .await?
        .ok_or(PaymentStoreError::InvalidStoredData)?;
    let provider: String =
        sqlx::query_scalar("SELECT left(provider_id,257) FROM callback_intakes WHERE id=$1")
            .bind(&id)
            .fetch_one(&mut *tx)
            .await?;
    let operation = lookup::prepare_in_transaction(&mut tx, clock, ids, profile, &provider).await?;
    let at = operation.created_at;
    let until = at
        .checked_add(LEASE_SECONDS)
        .filter(|v| *v <= 9_007_199_254_740_991)
        .ok_or(PaymentStoreError::ClockUnavailable)?;
    sqlx::query("INSERT INTO callback_lookup_fetch_starts(operation_id,started_at) VALUES($1,$2)")
        .bind(&operation.id)
        .bind(at)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO callback_inbox_generations(intake_id,generation,operation_id,created_at,lease_until) VALUES($1,$2,$3,$4,$5)")
        .bind(&id).bind(job.generation+1).bind(&operation.id).bind(at).bind(until).execute(&mut *tx).await?;
    sqlx::query("UPDATE callback_inbox_jobs SET generation=generation+1,lease_until=$1,updated_at=$2,state='leased',review_reason=NULL WHERE intake_id=$3")
        .bind(until).bind(at).bind(&id).execute(&mut *tx).await?;
    let operation_id = operation.id.clone();
    let permit = lookup::commit_started(tx, clock, operation, at, until).await?;
    job.generation += 1;
    job.updated_at = at;
    job.lease_until = Some(until);
    job.state = JobState::Leased;
    job.review_reason = None;
    Ok(Some(Claim {
        job,
        operation_id,
        until,
        permit: Some(permit),
    }))
}
pub(crate) async fn finish(
    pool: &PgPool,
    clock: &dyn Clock,
    claim: &Claim,
    state: JobState,
    reason: Option<ReviewReason>,
) -> Result<(), PaymentStoreError> {
    let initial_clock = clock
        .unix_seconds()
        .map_err(|_| PaymentStoreError::ClockUnavailable)?;
    let observed = ObservedClock::new(clock, initial_clock);
    let clock: &dyn Clock = &observed;
    let mut tx = pool.begin().await?;
    limits(&mut tx).await?;
    let at = now(clock)?;
    if at < claim.job.updated_at {
        return Err(PaymentStoreError::ClockUnavailable);
    }
    let delay = if state == JobState::Pending {
        retry_delay(claim.job.generation).map_err(|_| PaymentStoreError::InvalidStoredData)?
    } else {
        0
    };
    let due = at
        .checked_add(delay)
        .filter(|v| *v <= 9_007_199_254_740_991)
        .ok_or(PaymentStoreError::ClockUnavailable)?;
    let changed=sqlx::query("UPDATE callback_inbox_jobs SET state=$1,review_reason=$2,lease_until=NULL,updated_at=$3,due_at=$4 WHERE intake_id=$5 AND generation=$6 AND lease_until=$7 AND lease_until>$3")
        .bind(rows::state(state)?).bind(rows::reason(reason)?).bind(at).bind(due).bind(&claim.job.intake_id).bind(claim.job.generation).bind(claim.until).execute(&mut *tx).await?.rows_affected();
    let final_at = now(clock)?;
    // ObservedClock rejects a backward sample before yielding this value.
    if changed != 1 || final_at >= claim.until {
        return Err(PaymentStoreError::Conflict);
    }
    tx.commit().await?;
    Ok(())
}
/// Expired leased jobs with committed dispositions settle without another GET;
/// otherwise the immutable five-generation budget ends in visible review.
pub(crate) async fn sweep(
    pool: &PgPool,
    clock: &dyn Clock,
    profile: &str,
) -> Result<(), PaymentStoreError> {
    let initial_clock = clock
        .unix_seconds()
        .map_err(|_| PaymentStoreError::ClockUnavailable)?;
    let observed = ObservedClock::new(clock, initial_clock);
    let clock: &dyn Clock = &observed;
    active::audit(pool).await?;
    let mut tx = pool.begin().await?;
    limits(&mut tx).await?;
    let at = now(clock)?;
    let ids:Vec<String>=sqlx::query_scalar("SELECT j.intake_id FROM callback_inbox_active a JOIN callback_inbox_jobs j ON j.intake_id=a.intake_id JOIN callback_intakes i ON i.id=j.intake_id WHERE a.profile_id=$1 AND j.state IN ('pending','leased') AND j.updated_at<=$2 AND (j.lease_until IS NULL OR j.lease_until<=$2) ORDER BY j.intake_id FOR UPDATE OF j SKIP LOCKED LIMIT 25")
        .bind(profile).bind(at).fetch_all(&mut *tx).await?;
    for id in ids {
        let job = rows::load(&mut tx, &id)
            .await?
            .ok_or(PaymentStoreError::InvalidStoredData)?;
        let operation:Option<String>=sqlx::query_scalar("SELECT operation_id FROM callback_inbox_generations WHERE intake_id=$1 AND generation=$2")
            .bind(&id).bind(job.generation).fetch_optional(&mut *tx).await?;
        let outcome = match operation {
            Some(op) => drain::read_disposition(pool, &op).await?,
            None => None,
        };
        let (state, reason) = if let Some(outcome) = outcome {
            match outcome {
                drain::Disposition::Observed => ("done", None),
                drain::Disposition::PaidObserved => ("needs_review", Some("paid_observed")),
                drain::Disposition::Ambiguous => ("needs_review", Some("ambiguous")),
                drain::Disposition::Unmatched => ("needs_review", Some("unmatched")),
                drain::Disposition::CustodyInvalid => ("needs_review", Some("custody_invalid")),
            }
        } else if job.generation == MAX_GENERATIONS {
            ("needs_review", Some("generation_limit"))
        } else {
            continue;
        };
        sqlx::query("UPDATE callback_inbox_jobs SET state=$1,review_reason=$2,lease_until=NULL,updated_at=$3,due_at=$3 WHERE intake_id=$4 AND generation=$5")
            .bind(state).bind(reason).bind(at).bind(&id).bind(job.generation).execute(&mut *tx).await?;
    }
    // Settlement is separate from completion: old in-flight generations or review
    // dispositions retain their bounded admission slot even after newer OPEN.
    let complete:Vec<String>=sqlx::query_scalar("SELECT j.intake_id FROM callback_inbox_active a JOIN callback_inbox_jobs j ON j.intake_id=a.intake_id JOIN callback_intakes i ON i.id=j.intake_id WHERE a.profile_id=$1 AND j.state='done' ORDER BY j.intake_id FOR UPDATE OF j SKIP LOCKED LIMIT 200").bind(profile).fetch_all(&mut *tx).await?;
    for id in complete {
        let operations:Vec<String>=sqlx::query_scalar("SELECT operation_id FROM callback_inbox_generations WHERE intake_id=$1 ORDER BY generation LIMIT 6").bind(&id).fetch_all(&mut *tx).await?;
        let operations = rows::bounded_operations(operations)?;
        let mut ready = true;
        for op in operations {
            if drain::read_disposition(pool, &op).await? != Some(drain::Disposition::Observed) {
                ready = false;
            }
        }
        if ready {
            sqlx::query("UPDATE callback_inbox_jobs SET settled=true,updated_at=$1,due_at=$1 WHERE intake_id=$2 AND callback_intake_settleable(intake_id)").bind(at).bind(&id).execute(&mut *tx).await?;
            sqlx::query("DELETE FROM callback_inbox_active WHERE intake_id=$1")
                .bind(&id)
                .execute(&mut *tx)
                .await?;
        }
    }
    // The shared ObservedClock enforces the final nondecreasing sample.
    now(clock)?;
    tx.commit().await?;
    Ok(())
}
pub(crate) async fn outstanding(
    pool: &PgPool,
    profile: &str,
) -> Result<(u64, u64), PaymentStoreError> {
    active::audit(pool).await?;
    let ids:Vec<String>=sqlx::query_scalar("SELECT j.intake_id FROM callback_inbox_active a JOIN callback_inbox_jobs j ON j.intake_id=a.intake_id JOIN callback_intakes i ON i.id=j.intake_id WHERE a.profile_id=$1 ORDER BY j.intake_id LIMIT 201").bind(profile).fetch_all(pool).await?;
    let ids = rows::bounded_intakes(ids)?;
    let (mut review, mut retry) = (0, 0);
    for id in ids {
        let job = super::read_callback_job(pool, &id)
            .await?
            .ok_or(PaymentStoreError::InvalidStoredData)?;
        let operations:Vec<String>=sqlx::query_scalar("SELECT left(operation_id,33) FROM callback_inbox_generations WHERE intake_id=$1 ORDER BY generation LIMIT 6").bind(&id).fetch_all(pool).await?;
        let operations = rows::bounded_operations(operations)?;
        let mut needs_review = job.state == JobState::NeedsReview;
        for op in operations {
            if drain::read_disposition(pool, &op)
                .await?
                .is_some_and(|v| v != drain::Disposition::Observed)
            {
                needs_review = true;
            }
        }
        if needs_review {
            review += 1;
        } else {
            retry += 1;
        }
    }
    Ok((review, retry))
}
