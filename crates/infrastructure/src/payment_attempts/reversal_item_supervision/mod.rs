//! Private bounded exact reversal-item supervision; evidence carries no financial authority.
use super::evidence::{context, fresh_id, input, limits, now};
use super::*;
use crate::mollie::{ReversalKind, VerifiedReconciliationClient};
use commerce_application::request_context::{Clock, Identifiers};
use sqlx::PgPool;
pub mod inbox;
mod jobs;
pub mod review;
mod rows;
pub mod runner;
mod transitions;
mod validate;
pub use super::reversal_supervision::RecoveryError;
pub use runner::run_batch;

#[derive(Clone, PartialEq, Eq)]
pub struct Job {
    pub id: String,
    pub attempt_id: String,
    pub observation_id: String,
    pub page_id: String,
    pub ordinal: i32,
    pub child_id: String,
    pub enrollment_key: String,
    pub profile_id: String,
    pub payment_id: String,
    pub kind: ReversalKind,
    pub credential_fingerprint: String,
    pub state: String,
    pub review_reason: Option<String>,
    pub receipt_id: Option<String>,
    pub generation: i32,
    pub failures: i32,
    pub created_at: i64,
    pub updated_at: i64,
    pub due_at: i64,
    pub lease_until: Option<i64>,
}
fn kind_name(kind: ReversalKind) -> &'static str {
    match kind {
        ReversalKind::Refund => "refund",
        ReversalKind::Chargeback => "chargeback",
    }
}
fn fingerprint(v: &str) -> bool {
    v.len() == 64
        && v.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub struct EnrollIntent<'a> {
    pub attempt_id: &'a str,
    pub observation_id: &'a str,
    pub page_id: &'a str,
    pub ordinal: i32,
    pub enrollment_key: &'a str,
}
/// Enroll an exact candidate from a validated historical page.
pub async fn enroll(
    pool: &PgPool,
    clock: &dyn Clock,
    ids: &dyn Identifiers,
    client: &VerifiedReconciliationClient,
    intent: EnrollIntent<'_>,
) -> Result<Job, RecoveryError> {
    input(intent.attempt_id, intent.enrollment_key)?;
    if !identifier(intent.observation_id)
        || !identifier(intent.page_id)
        || !(1..=10).contains(&intent.ordinal)
    {
        return Err(RecoveryError::InvalidRequest);
    }
    let mut tx = pool.begin().await?;
    limits(&mut tx).await?;
    let (frozen, started) = context(&mut tx, intent.attempt_id, true).await?;
    let page =
        super::reversal_pages::rows::load(&mut tx, &frozen, started, None, Some(intent.page_id))
            .await?
            .ok_or(RecoveryError::InvalidRequest)?;
    let child = page
        .candidates
        .get((intent.ordinal - 1) as usize)
        .ok_or(RecoveryError::InvalidRequest)?;
    if frozen.snapshot().binding.profile.id != client.profile_id()
        || page.credential_fingerprint != client.credential_fingerprint()
    {
        return Err(RecoveryError::InvalidRequest);
    }
    if let Some(existing) = rows::by_key(&mut tx, intent.attempt_id, intent.enrollment_key).await? {
        if existing.observation_id != intent.observation_id
            || existing.page_id != intent.page_id
            || existing.ordinal != intent.ordinal
        {
            return Err(RecoveryError::Conflict);
        }
        validate::source(&mut tx, &existing, &frozen, started).await?;
        validate::receipt(&mut tx, &existing, &frozen, started).await?;
        tx.rollback().await?;
        return Ok(existing);
    }
    let candidate_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM reversal_item_jobs WHERE page_id=$1 AND ordinal=$2)",
    )
    .bind(intent.page_id)
    .bind(intent.ordinal)
    .fetch_one(&mut *tx)
    .await?;
    if candidate_exists {
        return Err(RecoveryError::Conflict);
    }
    let job_count: i64 = sqlx::query_scalar("SELECT count(*) FROM (SELECT 1 FROM reversal_item_jobs WHERE attempt_id=$1 LIMIT 128) bounded")
        .bind(intent.attempt_id).fetch_one(&mut *tx).await?;
    if job_count >= 128 {
        return Err(RecoveryError::Conflict);
    }
    let at = now(clock)?;
    let observation =
        super::observations::rows::by_id(&mut tx, &frozen, started, intent.observation_id).await?;
    if at < page.recorded_at || at < observation.recorded_at {
        return Err(RecoveryError::ClockUnavailable);
    }
    let expected = Job {
        id: fresh_id(ids)?,
        attempt_id: intent.attempt_id.into(),
        observation_id: intent.observation_id.into(),
        page_id: intent.page_id.into(),
        ordinal: intent.ordinal,
        child_id: child.clone(),
        enrollment_key: intent.enrollment_key.into(),
        profile_id: client.profile_id().into(),
        payment_id: page.payment_id.clone(),
        kind: page.kind,
        credential_fingerprint: page.credential_fingerprint.clone(),
        state: "pending".into(),
        review_reason: None,
        receipt_id: None,
        generation: 0,
        failures: 0,
        created_at: at,
        updated_at: at,
        due_at: at,
        lease_until: None,
    };
    validate::source(&mut tx, &expected, &frozen, started).await?;
    let count = sqlx::query("INSERT INTO reversal_item_jobs(id,attempt_id,observation_id,page_id,ordinal,child_id,enrollment_key,profile_id,payment_id,reversal_kind,credential_fingerprint,state,generation,failures,created_at,updated_at,due_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,'pending',0,0,$12,$12,$12)")
        .bind(&expected.id).bind(&expected.attempt_id).bind(&expected.observation_id).bind(&expected.page_id).bind(expected.ordinal)
        .bind(&expected.child_id).bind(&expected.enrollment_key).bind(&expected.profile_id).bind(&expected.payment_id)
        .bind(kind_name(expected.kind)).bind(&expected.credential_fingerprint).bind(at).execute(&mut *tx).await?.rows_affected();
    if count != 1 || rows::load(&mut tx, &expected.id).await? != Some(expected.clone()) {
        return Err(RecoveryError::InvalidStoredData);
    }
    if now(clock)? < at {
        return Err(RecoveryError::ClockUnavailable);
    }
    tx.commit().await?;
    Ok(expected)
}
/// Read and revalidate the source and any terminal receipt.
pub async fn read_job(pool: &PgPool, id: &str) -> Result<Option<Job>, RecoveryError> {
    if !identifier(id) {
        return Err(RecoveryError::InvalidRequest);
    }
    let mut tx = pool.begin().await?;
    limits(&mut tx).await?;
    let result = inspected(&mut tx, id).await?.map(|(job, _, _)| job);
    tx.rollback().await?;
    Ok(result)
}

/// Inspect the exact late fact for a parked uncertain operation without changing its checkpoint.
pub async fn read_late_receipt(
    pool: &PgPool,
    id: &str,
) -> Result<Option<super::reversals::ReversalReceipt>, RecoveryError> {
    if !identifier(id) {
        return Err(RecoveryError::InvalidRequest);
    }
    let mut tx = pool.begin().await?;
    limits(&mut tx).await?;
    let result = match inspected(&mut tx, id).await? {
        Some((job, frozen, started)) if job.review_reason.as_deref() == Some("uncertain") => {
            validate::late_receipt(&mut tx, &job, &frozen, started).await?
        }
        _ => None,
    };
    tx.rollback().await?;
    Ok(result)
}

async fn inspected(
    tx: &mut Transaction<'_, Postgres>,
    id: &str,
) -> Result<Option<(Job, FrozenAttempt, i64)>, RecoveryError> {
    let Some(initial) = rows::load(tx, id).await? else {
        return Ok(None);
    };
    let (frozen, started) = context(tx, &initial.attempt_id, true).await?;
    let current = rows::load(tx, id)
        .await?
        .ok_or(RecoveryError::InvalidStoredData)?;
    if current.attempt_id != initial.attempt_id {
        return Err(RecoveryError::InvalidStoredData);
    }
    validate::source(tx, &current, &frozen, started).await?;
    validate::receipt(tx, &current, &frozen, started).await?;
    Ok(Some((current, frozen, started)))
}
