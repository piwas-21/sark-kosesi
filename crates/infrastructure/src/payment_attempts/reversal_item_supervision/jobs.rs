use super::super::reversals;
use super::transitions::transition;
use super::validate::{exact_receipt, lock_job, operation, source};
use super::*;

pub(super) struct Claim {
    pub(super) job: Job,
    pub(super) operation_id: String,
    pub(super) attempt: AttemptSnapshot,
    pub(super) until: i64,
}
pub(super) enum Work {
    Claim(Box<Claim>),
    Recovered,
}
pub(super) enum Outcome {
    Observed,
    Transient,
    Invalid,
    Capped,
}

fn eligible(j: &Job, attempt_id: &str, profile: &str, fp: &str, at: i64) -> bool {
    j.attempt_id == attempt_id
        && j.profile_id == profile
        && j.credential_fingerprint == fp
        && j.updated_at <= at
        && ((j.state == "pending" && j.due_at <= at)
            || (j.state == "leased" && j.lease_until.is_some_and(|v| v <= at)))
}

/// One attempt claim precedes one job claim, avoiding lock-order inversions with journal writes.
pub(super) async fn claim(
    pool: &PgPool,
    clock: &dyn Clock,
    ids: &dyn Identifiers,
    profile: &str,
    fp: &str,
) -> Result<Option<Work>, RecoveryError> {
    let mut tx = pool.begin().await?;
    limits(&mut tx).await?;
    let at = now(clock)?;
    let candidate:Option<(String,String)>=sqlx::query_as("SELECT j.id,j.attempt_id FROM reversal_item_jobs j JOIN payment_attempts a ON a.id=j.attempt_id WHERE j.profile_id=$1 AND j.credential_fingerprint=$2 AND ((j.state='pending' AND j.due_at<=$3) OR (j.state='leased' AND j.lease_until<=$3)) AND j.updated_at<=$3 ORDER BY j.due_at,j.id FOR UPDATE OF a SKIP LOCKED LIMIT 1")
        .bind(profile).bind(fp).bind(at).fetch_optional(&mut *tx).await?;
    let Some((id, attempt_id)) = candidate else {
        tx.rollback().await?;
        return Ok(None);
    };
    let Some(j) = lock_job(&mut tx, &id).await? else {
        tx.rollback().await?;
        return Ok(None);
    };
    if !eligible(&j, &attempt_id, profile, fp, at) {
        tx.rollback().await?;
        return Ok(None);
    }
    let (frozen, started) = context(&mut tx, &j.attempt_id, true).await?;
    source(&mut tx, &j, &frozen, started).await?;
    if j.state == "leased" {
        let op = operation(&mut tx, &j).await?;
        let receipt = exact_receipt(&mut tx, &j, &frozen, started, &op.id, j.updated_at).await?;
        let outcome = if receipt.is_some() {
            "observed"
        } else {
            "uncertain"
        };
        transition(
            &mut tx,
            clock,
            &j,
            &op.id,
            at,
            (outcome, receipt.as_ref(), true),
        )
        .await?;
        tx.commit().await?;
        return Ok(Some(Work::Recovered));
    }
    let until = at
        .checked_add(60)
        .filter(|v| *v <= 9_007_199_254_740_991)
        .ok_or(RecoveryError::ClockUnavailable)?;
    let op = fresh_id(ids)?;
    let generation = j
        .generation
        .checked_add(1)
        .ok_or(RecoveryError::InvalidStoredData)?;
    let count=sqlx::query("INSERT INTO reversal_item_operations(id,job_id,generation,started_at,lease_until) VALUES($1,$2,$3,$4,$5)")
        .bind(&op).bind(&j.id).bind(generation).bind(at).bind(until).execute(&mut *tx).await?.rows_affected();
    if count != 1 {
        return Err(RecoveryError::InvalidStoredData);
    }
    let count=sqlx::query("UPDATE reversal_item_jobs SET state='leased',generation=$1,updated_at=$2,lease_until=$3 WHERE id=$4 AND state='pending' AND generation=$5")
        .bind(generation).bind(at).bind(until).bind(&j.id).bind(j.generation).execute(&mut *tx).await?.rows_affected();
    if count != 1 {
        return Err(RecoveryError::InvalidStoredData);
    }
    let leased = rows::load(&mut tx, &j.id)
        .await?
        .ok_or(RecoveryError::InvalidStoredData)?;
    let check = operation(&mut tx, &leased).await?;
    let mut expected = j.clone();
    expected.state = "leased".into();
    expected.generation = generation;
    expected.updated_at = at;
    expected.lease_until = Some(until);
    if check.id != op || leased != expected {
        return Err(RecoveryError::InvalidStoredData);
    }
    let final_at = now(clock)?;
    if final_at < at || final_at >= until {
        return Err(RecoveryError::LeaseLost);
    }
    tx.commit().await?;
    Ok(Some(Work::Claim(Box::new(Claim {
        job: leased,
        operation_id: op,
        attempt: frozen.snapshot().clone(),
        until,
    }))))
}
async fn locked_claim(
    tx: &mut Transaction<'_, Postgres>,
    c: &Claim,
) -> Result<(Job, FrozenAttempt, i64), RecoveryError> {
    let (frozen, started) = context(tx, &c.job.attempt_id, true).await?;
    let j = lock_job(tx, &c.job.id)
        .await?
        .ok_or(RecoveryError::LeaseLost)?;
    if j != c.job || frozen.snapshot() != &c.attempt {
        return Err(RecoveryError::LeaseLost);
    }
    let op = operation(tx, &j).await?;
    if op.id != c.operation_id {
        return Err(RecoveryError::LeaseLost);
    }
    source(tx, &j, &frozen, started).await?;
    Ok((j, frozen, started))
}
pub(super) async fn authorize(
    pool: &PgPool,
    clock: &dyn Clock,
    c: &Claim,
) -> Result<(), RecoveryError> {
    let mut tx = pool.begin().await?;
    limits(&mut tx).await?;
    let (j, frozen, started) = locked_claim(&mut tx, c).await?;
    if exact_receipt(&mut tx, &j, &frozen, started, &c.operation_id, j.updated_at)
        .await?
        .is_some()
    {
        return Err(RecoveryError::InvalidStoredData);
    }
    let at = now(clock)?;
    if at < j.updated_at || at >= c.until {
        return Err(RecoveryError::LeaseLost);
    }
    tx.rollback().await?;
    Ok(())
}
pub(super) async fn finish(
    pool: &PgPool,
    clock: &dyn Clock,
    c: &Claim,
    outcome: Outcome,
) -> Result<(), RecoveryError> {
    let mut tx = pool.begin().await?;
    limits(&mut tx).await?;
    let (j, frozen, started) = locked_claim(&mut tx, c).await?;
    let at = now(clock)?;
    if at < j.updated_at || at >= c.until {
        return Err(RecoveryError::LeaseLost);
    }
    let receipt =
        exact_receipt(&mut tx, &j, &frozen, started, &c.operation_id, j.updated_at).await?;
    let name = match outcome {
        Outcome::Observed => "observed",
        Outcome::Transient => "transient",
        Outcome::Invalid => "invalid",
        Outcome::Capped => "capped",
    };
    if (name == "observed") != receipt.is_some() {
        return Err(RecoveryError::InvalidStoredData);
    }
    if name == "capped" && reversals::rows::history(&mut tx, &j.attempt_id).await?.0 < 128 {
        return Err(RecoveryError::InvalidStoredData);
    }
    transition(
        &mut tx,
        clock,
        &j,
        &c.operation_id,
        at,
        (name, receipt.as_ref(), false),
    )
    .await?;
    tx.commit().await?;
    Ok(())
}
pub(super) async fn observation_failure(
    pool: &PgPool,
    c: &Claim,
) -> Result<Outcome, RecoveryError> {
    Ok(if at_capacity(pool, c).await? {
        Outcome::Capped
    } else {
        Outcome::Invalid
    })
}
pub(super) async fn at_capacity(pool: &PgPool, c: &Claim) -> Result<bool, RecoveryError> {
    let mut tx = pool.begin().await?;
    limits(&mut tx).await?;
    let (j, _, _) = locked_claim(&mut tx, c).await?;
    let count = reversals::rows::history(&mut tx, &j.attempt_id).await?.0;
    tx.rollback().await?;
    Ok(count >= 128)
}

#[cfg(test)]
#[path = "../../../tests/support/reversal_item_supervision_jobs.rs"]
mod tests;
