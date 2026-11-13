//! Detached inspection of an exact stopped scan and its late page, without I/O.
use super::*;

pub struct Inspection {
    /// The stored checkpoint; prior operation history is not reconstructed.
    pub job: Job,
    /// Presence is inspected only for an uncertain stop.
    pub late_page_present: Option<bool>,
}

/// Inspect one scan under the same attempt claim used by page and job writers.
/// The result is historical evidence, not recovery or financial authority.
pub async fn inspect(pool: &PgPool, id: &str) -> Result<Option<Inspection>, RecoveryError> {
    if !identifier(id) {
        return Err(RecoveryError::InvalidRequest);
    }
    let mut tx = pool.begin().await?;
    limits(&mut tx).await?;
    let Some(initial) = rows::load(&mut tx, id).await? else {
        tx.rollback().await?;
        return Ok(None);
    };
    let (frozen, started) = context(&mut tx, &initial.attempt_id, true).await?;
    let job = rows::load(&mut tx, id)
        .await?
        .ok_or(RecoveryError::InvalidStoredData)?;
    if job.attempt_id != initial.attempt_id {
        return Err(RecoveryError::InvalidStoredData);
    }
    validate::source(&mut tx, &job, &frozen, started).await?;
    validate::tip(&mut tx, &job, &frozen, started).await?;
    let late_page_present = if job.review_reason.as_deref() == Some("uncertain") {
        Some(late_page(&mut tx, &job, &frozen, started).await?)
    } else {
        None
    };
    tx.rollback().await?;
    Ok(Some(Inspection {
        job,
        late_page_present,
    }))
}

async fn late_page(
    tx: &mut Transaction<'_, Postgres>,
    job: &Job,
    frozen: &FrozenAttempt,
    started: i64,
) -> Result<bool, RecoveryError> {
    // An uncertain outcome preserves the prior pending checkpoint's failures.
    // Pending admits fewer than three, and this operation is a new generation.
    if job.failures >= 3 || job.failures >= job.generation {
        return Err(RecoveryError::InvalidStoredData);
    }
    let operation: Option<(String, i64)> = sqlx::query_as(
        "SELECT left(id,33),started_at FROM reversal_scan_operations WHERE job_id=$1 AND generation=$2 AND outcome='uncertain' AND predecessor_id IS NOT DISTINCT FROM $3 AND finished_at=$4 AND started_at>=$5 AND lease_until=started_at+60 AND finished_at>=lease_until AND page_id IS NULL AND start_xid IS NOT NULL AND finish_xid IS NOT NULL AND start_xid<>finish_xid",
    )
    .bind(&job.id)
    .bind(job.generation)
    .bind(&job.tip_page_id)
    .bind(job.updated_at)
    .bind(job.created_at)
    .fetch_optional(&mut **tx)
    .await?;
    let (effect, operation_started) = operation.ok_or(RecoveryError::InvalidStoredData)?;
    if !identifier(&effect) {
        return Err(RecoveryError::InvalidStoredData);
    }
    if let Some(tip) = &job.tip_page_id {
        // `validate::tip` has already verified this row and its entire ancestry
        // under the attempt claim. It must also predate this operation's start.
        let recorded_at: Option<i64> = sqlx::query_scalar(
            "SELECT recorded_at FROM payment_reversal_pages WHERE id=$1 AND attempt_id=$2",
        )
        .bind(tip)
        .bind(&job.attempt_id)
        .fetch_optional(&mut **tx)
        .await?;
        if recorded_at.is_none_or(|at| at > operation_started) {
            return Err(RecoveryError::InvalidStoredData);
        }
    }
    let page =
        super::super::reversal_pages::rows::load(tx, frozen, started, Some(&effect), None).await?;
    if let Some(page) = &page
        && (page.previous_page_id != job.tip_page_id
            || page.payment_id != job.payment_id
            || page.kind != job.kind
            || page.credential_fingerprint != job.credential_fingerprint
            || page.recorded_at < operation_started)
    {
        return Err(RecoveryError::InvalidStoredData);
    }
    Ok(page.is_some())
}
