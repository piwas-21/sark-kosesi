//! Read-only, one-attempt evidence preflight; never provider or recovery authority.
use super::*;
use commerce_domain::payment_attempts::identities::summarize;
use sqlx::PgPool;

/// Redacted information only; counts do not establish absence at the provider.
#[derive(Debug, PartialEq, Eq)]
pub struct Custody {
    pub checkpoint_phase: Option<Phase>,
    pub checkpoint_review: Option<ReviewReason>,
    pub checkpoint_page: bool,
    pub observations: usize,
    pub identities: usize,
    pub paid_observed: bool,
    /// None means no resolution; Some(false) means later observations exist.
    pub resolution_current: Option<bool>,
}

/// Inspect the original frozen attempt, dispatch and exact recovery event in one
/// read-only snapshot. Validate all retained observations, the latest resolution,
/// and the checkpoint-linked search chain, if any. No unlinked page is evaluated.
pub async fn inspect_custody(pool: &PgPool, id: &str) -> Result<Custody, RecoveryError> {
    if !identifier(id) {
        return Err(RecoveryError::InvalidRequest);
    }
    let mut tx = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
        .execute(&mut *tx)
        .await?;
    sqlx::query("SET LOCAL statement_timeout='2s'")
        .execute(&mut *tx)
        .await?;
    sqlx::query("SET LOCAL lock_timeout='1s'")
        .execute(&mut *tx)
        .await?;
    let (frozen, started_at) = context(&mut tx, id, false)
        .await
        .map_err(|_| RecoveryError::InvalidStoredData)?;
    let event_count: i64 = sqlx::query_scalar(r#"SELECT count(*) FROM payment_dispatch_starts s
 JOIN commerce_outbox e ON e.aggregate_id=$2 AND e.kind='payment.reconciliation_required.v1'
 WHERE s.attempt_id=$1 AND s.started_at=$3 AND e.created_at=s.started_at
 AND e.payload=jsonb_build_object('schema_version',1,'order_id',$2,'attempt_id',$1,'dispatch_id',s.id)"#)
        .bind(id).bind(&frozen.snapshot().binding.order_id).bind(started_at)
        .fetch_one(&mut *tx).await?;
    if event_count != 1 {
        return Err(RecoveryError::InvalidStoredData);
    }
    let job = jobs::load(&mut tx, id).await?;
    let checkpoint_page = if let Some((page_id, candidate_index)) = job
        .as_ref()
        .and_then(|j| j.page_id.as_deref().map(|id| (id, j.candidate_index)))
    {
        let page =
            super::super::searches::rows::load(&mut tx, &frozen, started_at, None, Some(page_id))
                .await
                .map_err(|_| RecoveryError::InvalidStoredData)?
                .ok_or(RecoveryError::InvalidStoredData)?;
        let index =
            usize::try_from(candidate_index).map_err(|_| RecoveryError::InvalidStoredData)?;
        if index > page.intent.candidates.len() {
            return Err(RecoveryError::InvalidStoredData);
        }
        true
    } else {
        false
    };
    let facts = super::super::observations::rows::prefix(&mut tx, &frozen, started_at, i64::MAX)
        .await
        .map_err(|_| RecoveryError::InvalidStoredData)?;
    let summary = summarize(frozen.snapshot(), started_at, &facts)
        .map_err(|_| RecoveryError::InvalidStoredData)?;
    let resolution = super::super::identities::rows::load(&mut tx, &frozen, started_at, None)
        .await
        .map_err(|_| RecoveryError::InvalidStoredData)?;
    let watermark = facts.last().map_or(0, |v| v.revision);
    let result = Custody {
        checkpoint_phase: job.as_ref().map(|j| j.phase),
        checkpoint_review: job.and_then(|j| j.review_reason),
        checkpoint_page,
        observations: facts.len(),
        identities: summary.provider_ids.len(),
        paid_observed: !summary.paid_provider_ids.is_empty(),
        resolution_current: resolution.map(|v| v.through_observation == watermark),
    };
    tx.rollback().await?;
    Ok(result)
}
