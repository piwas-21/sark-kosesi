//! Read-only, one-attempt evidence preflight; never provider or recovery authority.
use super::*;
use commerce_domain::payment_attempts::{identities::summarize, searches::SearchSnapshot};
use sqlx::{PgPool, Postgres, Transaction};
use std::collections::BTreeSet;

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

// These states follow the supervisor's page, candidate and poll transitions.
fn checkpoint_position(
    phase: Phase,
    index: usize,
    page: Option<(usize, bool, i64)>,
) -> Result<(), RecoveryError> {
    match page {
        None if index == 0 => Ok(()),
        None => Err(RecoveryError::InvalidStoredData),
        Some((count, has_next, page_number)) => {
            if index > count
                || (phase == Phase::Search && (index != count || !has_next || page_number >= 100))
                || (phase == Phase::Poll && (index != count || has_next))
            {
                Err(RecoveryError::InvalidStoredData)
            } else {
                Ok(())
            }
        }
    }
}

// An old checkpoint can coexist with newer observations or resolutions. Require
// a validated historical supervisor resolution, not today's latest outcome.
async fn poll_resolution(
    tx: &mut Transaction<'_, Postgres>,
    frozen: &FrozenAttempt,
    started_at: i64,
    job: &Job,
    claimed: &BTreeSet<&str>,
) -> Result<(), RecoveryError> {
    let keys: Vec<_> = (1..=job.generation)
        .map(|generation| format!("recovery-{}-{generation}-resolve", job.attempt_id))
        .collect();
    let id: Option<String> = sqlx::query_scalar(
        r#"SELECT left(id,33) FROM payment_identity_resolutions
 WHERE attempt_id=$1 AND effect_key=ANY($2) AND recorded_at<=$3
 AND CASE WHEN jsonb_typeof(summary->'provider_ids')='array'
       AND jsonb_typeof(summary->'paid_provider_ids')='array'
     THEN jsonb_array_length(summary->'provider_ids')=1
       AND jsonb_array_length(summary->'paid_provider_ids')=0 ELSE false END
 ORDER BY revision DESC LIMIT 1"#,
    )
    .bind(&job.attempt_id)
    .bind(&keys)
    .bind(job.updated_at)
    .fetch_optional(&mut **tx)
    .await?;
    let resolution = match id {
        Some(id) => super::super::identities::rows::by_id(tx, frozen, started_at, &id)
            .await
            .map_err(|_| RecoveryError::InvalidStoredData)?,
        None => None,
    }
    .ok_or(RecoveryError::InvalidStoredData)?;
    // The bounded SQL predicate selects a single-identity/no-paid summary;
    // the existing reader then verifies its exact authenticated prefix.
    let resolved = resolution
        .summary
        .provider_ids
        .first()
        .ok_or(RecoveryError::InvalidStoredData)?;
    if claimed.iter().any(|id| resolved != id) {
        return Err(RecoveryError::InvalidStoredData);
    }
    Ok(())
}

/// Return candidate IDs for the consumed portion of the linked chain. Pages
/// before the tip were fully consumed before the supervisor advanced.
fn consumed_candidates(pages: &[SearchSnapshot], index: usize) -> BTreeSet<&str> {
    pages
        .iter()
        .enumerate()
        .flat_map(|(position, page)| {
            let count = if position == 0 {
                index
            } else {
                page.intent.candidates.len()
            };
            page.intent
                .candidates
                .iter()
                .take(count)
                .map(String::as_str)
        })
        .collect()
}

/// Inspect the original frozen attempt, dispatch and exact recovery event in one
/// read-only snapshot. Validate all retained observations, the latest resolution,
/// and the checkpoint-linked search chain and claimed candidate evidence, if any.
/// Poll additionally requires a valid historical supervisor resolution. No
/// unlinked page is evaluated and no output grants financial authority.
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
    let pages = if let Some(job) = &job {
        let index =
            usize::try_from(job.candidate_index).map_err(|_| RecoveryError::InvalidStoredData)?;
        if let Some(page_id) = job.page_id.as_deref() {
            let pages =
                super::super::searches::rows::load_chain(&mut tx, &frozen, started_at, page_id)
                    .await
                    .map_err(|_| RecoveryError::InvalidStoredData)?
                    .ok_or(RecoveryError::InvalidStoredData)?;
            let page = pages.first().ok_or(RecoveryError::InvalidStoredData)?;
            checkpoint_position(
                job.phase,
                index,
                Some((
                    page.intent.candidates.len(),
                    page.intent.next.is_some(),
                    page.page_number,
                )),
            )?;
            Some(pages)
        } else {
            checkpoint_position(job.phase, index, None)?;
            None
        }
    } else {
        None
    };
    let facts = super::super::observations::rows::prefix(&mut tx, &frozen, started_at, i64::MAX)
        .await
        .map_err(|_| RecoveryError::InvalidStoredData)?;
    let summary = summarize(frozen.snapshot(), started_at, &facts)
        .map_err(|_| RecoveryError::InvalidStoredData)?;
    if let (Some(job), Some(pages)) = (&job, &pages) {
        let index =
            usize::try_from(job.candidate_index).map_err(|_| RecoveryError::InvalidStoredData)?;
        let claimed = consumed_candidates(pages, index);
        let observed: BTreeSet<_> = summary.provider_ids.iter().map(String::as_str).collect();
        if claimed.iter().any(|id| !observed.contains(id)) {
            return Err(RecoveryError::InvalidStoredData);
        }
        if job.phase == Phase::Poll {
            poll_resolution(&mut tx, &frozen, started_at, job, &claimed).await?;
        }
    }
    let resolution = super::super::identities::rows::load(&mut tx, &frozen, started_at, None)
        .await
        .map_err(|_| RecoveryError::InvalidStoredData)?;
    let watermark = facts.last().map_or(0, |v| v.revision);
    let result = Custody {
        checkpoint_phase: job.as_ref().map(|j| j.phase),
        checkpoint_review: job.and_then(|j| j.review_reason),
        checkpoint_page: pages.is_some(),
        observations: facts.len(),
        identities: summary.provider_ids.len(),
        paid_observed: !summary.paid_provider_ids.is_empty(),
        resolution_current: resolution.map(|v| v.through_observation == watermark),
    };
    tx.rollback().await?;
    Ok(result)
}

#[cfg(test)]
#[path = "../../../tests/support/payment_custody_position.rs"]
mod tests;
