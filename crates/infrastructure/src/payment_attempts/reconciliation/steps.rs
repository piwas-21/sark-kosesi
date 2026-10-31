use super::super::{identities, observations, searches};
use super::*;
use crate::mollie::SearchCursor;
fn custody(error: PaymentStoreError) -> MollieError {
    MollieError::Custody(error)
}
fn invalid() -> MollieError {
    custody(PaymentStoreError::InvalidStoredData)
}
pub(crate) fn review(job: &mut Job, reason: ReviewReason) {
    job.phase = Phase::NeedsReview;
    job.review_reason = Some(reason);
}
async fn attempt(pool: &PgPool, id: &str) -> Result<FrozenAttempt, MollieError> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|_| custody(PaymentStoreError::Database))?;
    limits(&mut tx).await.map_err(custody)?;
    let (frozen, _) = context(&mut tx, id, false).await.map_err(custody)?;
    tx.rollback()
        .await
        .map_err(|_| custody(PaymentStoreError::Database))?;
    Ok(frozen)
}
/// Each claim can make at most one provider GET; only the GET-only client enters.
pub(crate) async fn advance(
    pool: &PgPool,
    clock: &dyn Clock,
    ids: &dyn Identifiers,
    client: &VerifiedReconciliationClient,
    claim: &Claim,
) -> Result<Job, MollieError> {
    let frozen = attempt(pool, &claim.job.attempt_id).await?;
    let mut next = claim.job.clone();
    next.failures = 0;
    match next.phase {
        Phase::Search => {
            let effect = format!("recovery-{}-page-{}", next.attempt_id, next.operation);
            let existing = searches::read_recorded(pool, &next.attempt_id, &effect)
                .await
                .map_err(custody)?;
            let page = if let Some(page) = existing {
                page
            } else {
                let previous = match &next.page_id {
                    Some(id) => Some(
                        searches::read_page(pool, &next.attempt_id, id)
                            .await
                            .map_err(custody)?,
                    ),
                    None => None,
                };
                if previous.as_ref().is_some_and(|p| p.page_number >= 100) {
                    review(&mut next, ReviewReason::ScanLimit);
                    return Ok(next);
                }
                let cursor = previous
                    .as_ref()
                    .map(|p| p.intent.next.as_deref().ok_or_else(invalid))
                    .transpose()?;
                let cursor = cursor.map(SearchCursor::capture).transpose()?;
                let verified = client
                    .search_page(frozen.snapshot(), cursor.as_ref())
                    .await?;
                searches::record_page(
                    pool,
                    clock,
                    ids,
                    &effect,
                    next.page_id.as_deref(),
                    &verified,
                )
                .await
                .map_err(custody)?
                .snapshot
            };
            if page.intent.previous_page_id != next.page_id {
                return Err(invalid());
            }
            next.page_id = Some(page.id);
            next.candidate_index = 0;
            next.phase = Phase::Candidates;
        }
        Phase::Candidates => {
            let page = searches::read_page(
                pool,
                &next.attempt_id,
                next.page_id.as_deref().ok_or_else(invalid)?,
            )
            .await
            .map_err(custody)?;
            let index = usize::try_from(next.candidate_index).map_err(|_| invalid())?;
            if let Some(id) = page.intent.candidates.get(index) {
                fetch(pool, clock, ids, client, frozen.snapshot(), &next, id).await?;
                next.candidate_index += 1;
            } else {
                if index != page.intent.candidates.len() {
                    return Err(invalid());
                }
                let summary = resolve(pool, clock, ids, &next).await?;
                if summary.provider_ids.len() > 1 {
                    review(&mut next, ReviewReason::Ambiguous);
                } else if page.intent.next.is_some() && page.page_number < 100 {
                    next.phase = Phase::Search;
                } else if page.intent.next.is_some() {
                    review(&mut next, ReviewReason::ScanLimit);
                } else if !summary.paid_provider_ids.is_empty() {
                    review(&mut next, ReviewReason::PaidObserved);
                } else if summary.provider_ids.is_empty() {
                    review(&mut next, ReviewReason::Unresolved);
                } else {
                    next.phase = Phase::Poll;
                }
            }
        }
        Phase::Poll => {
            let summary = resolve(pool, clock, ids, &next).await?;
            if summary.provider_ids.is_empty() {
                review(&mut next, ReviewReason::Unresolved);
            } else if summary.provider_ids.len() != 1 {
                review(&mut next, ReviewReason::Ambiguous);
            } else if !summary.paid_provider_ids.is_empty() {
                review(&mut next, ReviewReason::PaidObserved);
            } else if next.polls >= 10 {
                review(&mut next, ReviewReason::PollLimit);
            } else {
                let id = summary.provider_ids.first().ok_or_else(invalid)?;
                fetch(pool, clock, ids, client, frozen.snapshot(), &next, id).await?;
                next.polls += 1;
                let summary = identities::resolve(
                    pool,
                    clock,
                    ids,
                    &next.attempt_id,
                    &format!(
                        "recovery-{}-{}-after-poll",
                        next.attempt_id, next.generation
                    ),
                )
                .await
                .map_err(custody)?
                .resolution
                .summary;
                if summary.provider_ids.len() > 1 {
                    review(&mut next, ReviewReason::Ambiguous);
                } else if !summary.paid_provider_ids.is_empty() {
                    review(&mut next, ReviewReason::PaidObserved);
                } else if next.polls >= 10 {
                    review(&mut next, ReviewReason::PollLimit);
                }
            }
        }
        Phase::NeedsReview => return Err(invalid()),
    }
    Ok(next)
}
async fn resolve(
    pool: &PgPool,
    clock: &dyn Clock,
    ids: &dyn Identifiers,
    job: &Job,
) -> Result<commerce_domain::payment_attempts::identities::IdentitySummary, MollieError> {
    let effect = format!("recovery-{}-{}-resolve", job.attempt_id, job.generation);
    Ok(
        identities::resolve(pool, clock, ids, &job.attempt_id, &effect)
            .await
            .map_err(custody)?
            .resolution
            .summary,
    )
}
async fn fetch(
    pool: &PgPool,
    clock: &dyn Clock,
    ids: &dyn Identifiers,
    client: &VerifiedReconciliationClient,
    attempt: &AttemptSnapshot,
    job: &Job,
    id: &str,
) -> Result<(), MollieError> {
    // A fresh lease uses a fresh fetch operation. A delayed stale worker may retain
    // its authenticated facts but cannot advance progress under an expired fence.
    let effect = format!("recovery-{}-{}-fetch", job.attempt_id, job.generation);
    if observations::read_recorded(pool, &job.attempt_id, &effect)
        .await
        .map_err(custody)?
        .is_none()
    {
        let verified = client.fetch(attempt, id).await?;
        observations::record_verified(pool, clock, ids, &job.attempt_id, &effect, &verified)
            .await
            .map_err(custody)?;
    }
    Ok(())
}
