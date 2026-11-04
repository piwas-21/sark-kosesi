//! Private, bounded staff inspection of historical late-stock review stops.
use super::{LateStockError, Outcome, ReviewCase, ReviewReason, identifier, rows};
use crate::staff_access::{SessionToken, StaffError, lock_staff};
use commerce_application::{request_context::Clock, staff_access::Capability};
use sqlx::PgPool;

/// Limits full historical proofs, each of which may include a 2 MiB footprint.
pub const MAX_CANDIDATES: u8 = 4;

#[derive(Debug)]
pub enum InboxError {
    InvalidRequest,
    Staff(StaffError),
    Custody(LateStockError),
}

/// Deliberately excludes customer details, footprints, provider and payment data.
#[derive(Debug, PartialEq, Eq)]
pub struct Summary {
    pub case_id: String,
    pub case_digest: String,
    pub status: CaseStatus,
    pub next_generation: Option<i32>,
    pub order_id: String,
    pub assessed_at: i64,
    pub reason: ReviewReason,
}

#[derive(Debug, PartialEq, Eq)]
pub enum CaseStatus {
    Open,
    Allocated,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Page {
    pub cases: Vec<Summary>,
    /// Last scanned candidate, even when filtering produces no matches. A full
    /// page may be followed by an empty page; None means this scan reached its end.
    pub next_cursor: Option<String>,
}

#[derive(sqlx::FromRow)]
struct Candidate {
    id: String,
    order_id: String,
    attempt_id: String,
}

fn validate_candidate(candidate: &Candidate) -> Result<(), LateStockError> {
    if !identifier(&candidate.id)
        || !identifier(&candidate.order_id)
        || !identifier(&candidate.attempt_id)
    {
        return Err(LateStockError::InvalidStoredData);
    }
    Ok(())
}

// The caller has already validated the complete case, outcome and decision custody.
fn project(
    candidate: &Candidate,
    outcome: Option<Outcome>,
    case: ReviewCase,
    case_digest: String,
    decision_count: usize,
) -> Result<Summary, LateStockError> {
    if case.id != candidate.id
        || case.order_id != candidate.order_id
        || case.attempt_id != candidate.attempt_id
    {
        return Err(LateStockError::InvalidStoredData);
    }
    let status = match outcome {
        Some(Outcome::NeedsReview(ref current)) if current == &case => CaseStatus::Open,
        Some(Outcome::Late(ref allocation))
            if allocation.order_id == candidate.order_id
                && allocation.attempt_id == candidate.attempt_id =>
        {
            CaseStatus::Allocated
        }
        _ => return Err(LateStockError::InvalidStoredData),
    };
    let next_generation = if status == CaseStatus::Open
        && case.reason == ReviewReason::ComponentScarcity
        && decision_count < 3
    {
        i32::try_from(decision_count + 1).ok()
    } else {
        None
    };
    Ok(Summary {
        case_id: case.id,
        case_digest,
        status,
        next_generation,
        order_id: case.order_id,
        assessed_at: case.assessed_at,
        reason: case.reason,
    })
}

/// A live keyset scan, not a snapshot: refresh from the beginning to include
/// cases inserted at or before a previously returned cursor. Filtering happens
/// after validation, and the page fails closed if any scanned custody is invalid.
pub async fn list(
    pool: &PgPool,
    token: &SessionToken,
    clock: &dyn Clock,
    after: Option<&str>,
    reason: Option<ReviewReason>,
    limit: u8,
) -> Result<Page, InboxError> {
    if limit == 0 || limit > MAX_CANDIDATES || after.is_some_and(|value| !identifier(value)) {
        return Err(InboxError::InvalidRequest);
    }
    let mut access = lock_staff(pool, token, clock, Capability::StockReview)
        .await
        .map_err(InboxError::Staff)?;
    // Separate queries preserve an index range seek even under generic plans.
    let candidates: Vec<Candidate> = if let Some(after) = after {
        sqlx::query_as("SELECT left(id,33) AS id,left(order_id,33) AS order_id,left(attempt_id,33) AS attempt_id FROM late_stock_review_cases WHERE id COLLATE \"C\" > $1 COLLATE \"C\" ORDER BY id COLLATE \"C\" LIMIT $2")
            .bind(after).bind(i64::from(limit)).fetch_all(&mut *access.tx).await
    } else {
        sqlx::query_as("SELECT left(id,33) AS id,left(order_id,33) AS order_id,left(attempt_id,33) AS attempt_id FROM late_stock_review_cases ORDER BY id COLLATE \"C\" LIMIT $1")
            .bind(i64::from(limit)).fetch_all(&mut *access.tx).await
    }.map_err(|error| InboxError::Custody(error.into()))?;
    let mut cases = Vec::new();
    for candidate in &candidates {
        validate_candidate(candidate).map_err(InboxError::Custody)?;
        // Serialize the whole proof with decisions and allocations for this attempt.
        crate::order_stock::source::claim_attempt(&mut access.tx, &candidate.attempt_id)
            .await
            .map_err(|error| InboxError::Custody(error.into()))?;
        let outcome = rows::read(&mut access.tx, &candidate.order_id)
            .await
            .map_err(InboxError::Custody)?;
        let case = rows::review_case(&mut access.tx, &candidate.order_id)
            .await
            .map_err(InboxError::Custody)?
            .ok_or(InboxError::Custody(LateStockError::InvalidStoredData))?;
        let decisions = super::disposition::validate_history(&mut access.tx, &case.id)
            .await
            .map_err(InboxError::Custody)?;
        let case_digest: String = sqlx::query_scalar(
            "SELECT late_stock_review_digest(s) FROM late_stock_review_cases s WHERE s.id=$1",
        )
        .bind(&case.id)
        .fetch_one(&mut *access.tx)
        .await
        .map_err(|error| InboxError::Custody(error.into()))?;
        let summary = project(candidate, outcome, case, case_digest, decisions.len())
            .map_err(InboxError::Custody)?;
        if reason.is_none_or(|wanted| wanted == summary.reason) {
            cases.push(summary);
        }
    }
    let next_cursor = if candidates.len() == usize::from(limit) {
        candidates.last().map(|candidate| candidate.id.clone())
    } else {
        None
    };
    // Recheck after potentially expensive proofs, before releasing locked authority.
    access.check_time(clock).map_err(InboxError::Staff)?;
    access
        .tx
        .commit()
        .await
        .map_err(|error| InboxError::Custody(error.into()))?;
    Ok(Page { cases, next_cursor })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::late_stock::{LateAllocation, LateFootprint, ReviewFootprint};
    use commerce_domain::order_stock::{Footprint, Settlement};

    const CASE_ID: &str = "11111111111111111111111111111111";
    const ORDER_ID: &str = "22222222222222222222222222222222";
    const ATTEMPT_ID: &str = "33333333333333333333333333333333";
    const OTHER_ID: &str = "44444444444444444444444444444444";
    const DIGEST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn candidate() -> Candidate {
        Candidate {
            id: CASE_ID.into(),
            order_id: ORDER_ID.into(),
            attempt_id: ATTEMPT_ID.into(),
        }
    }

    // rows::read has already validated complete historical proof before projection.
    fn case() -> ReviewCase {
        ReviewCase {
            id: CASE_ID.into(),
            order_id: ORDER_ID.into(),
            attempt_id: ATTEMPT_ID.into(),
            recognition_id: OTHER_ID.into(),
            group_id: OTHER_ID.into(),
            actor: "review".into(),
            reason: ReviewReason::ComponentScarcity,
            assessed_at: 123,
            footprint: ReviewFootprint {
                schema_version: 1,
                original: vec![],
                components: vec![],
            },
        }
    }

    fn allocation() -> LateAllocation {
        LateAllocation {
            id: OTHER_ID.into(),
            order_id: ORDER_ID.into(),
            attempt_id: ATTEMPT_ID.into(),
            recognition_id: OTHER_ID.into(),
            original_group_id: OTHER_ID.into(),
            fresh_group_id: CASE_ID.into(),
            actor: "allocation".into(),
            allocated_at: 123,
            expires_at: 183,
            footprint: LateFootprint {
                schema_version: 1,
                original: vec![],
                fresh: vec![],
                components: vec![],
            },
        }
    }

    #[test]
    fn candidate_identifiers_fail_closed_independently() {
        let mut item = candidate();
        assert!(validate_candidate(&item).is_ok());
        item.id = "invalid".into();
        assert!(matches!(
            validate_candidate(&item),
            Err(LateStockError::InvalidStoredData)
        ));
        item = candidate();
        item.order_id = "invalid".into();
        assert!(matches!(
            validate_candidate(&item),
            Err(LateStockError::InvalidStoredData)
        ));
        item = candidate();
        item.attempt_id = "invalid".into();
        assert!(matches!(
            validate_candidate(&item),
            Err(LateStockError::InvalidStoredData)
        ));
    }

    #[test]
    fn projection_requires_exact_case_and_a_review_or_late_outcome() {
        let item = candidate();
        let reviewed = case();
        let project_case = |outcome, source| project(&item, outcome, source, DIGEST.into(), 0);
        assert!(matches!(
            project_case(None, reviewed.clone()),
            Err(LateStockError::InvalidStoredData)
        ));
        let settlement = Settlement {
            id: CASE_ID.into(),
            order_id: ORDER_ID.into(),
            attempt_id: ATTEMPT_ID.into(),
            recognition_id: OTHER_ID.into(),
            group_id: OTHER_ID.into(),
            actor: "settlement".into(),
            settled_at: 123,
            footprint: Footprint {
                schema_version: 1,
                members: vec![],
                components: vec![],
            },
        };
        assert!(matches!(
            project_case(Some(Outcome::Original(settlement)), reviewed.clone()),
            Err(LateStockError::InvalidStoredData)
        ));
        for field in 0..3 {
            let mut wrong = reviewed.clone();
            match field {
                0 => wrong.id = OTHER_ID.into(),
                1 => wrong.order_id = OTHER_ID.into(),
                _ => wrong.attempt_id = OTHER_ID.into(),
            }
            assert!(matches!(
                project_case(Some(Outcome::NeedsReview(wrong.clone())), wrong),
                Err(LateStockError::InvalidStoredData)
            ));
        }
        let mut other = reviewed.clone();
        other.assessed_at += 1;
        assert!(matches!(
            project_case(Some(Outcome::NeedsReview(other)), reviewed.clone()),
            Err(LateStockError::InvalidStoredData)
        ));
        let mut wrong_allocation = allocation();
        wrong_allocation.order_id = OTHER_ID.into();
        assert!(matches!(
            project_case(Some(Outcome::Late(wrong_allocation)), reviewed.clone()),
            Err(LateStockError::InvalidStoredData)
        ));
        let mut wrong_allocation = allocation();
        wrong_allocation.attempt_id = OTHER_ID.into();
        assert!(matches!(
            project_case(Some(Outcome::Late(wrong_allocation)), reviewed.clone()),
            Err(LateStockError::InvalidStoredData)
        ));
        assert_eq!(
            project_case(Some(Outcome::NeedsReview(reviewed.clone())), reviewed).ok(),
            Some(Summary {
                case_id: CASE_ID.into(),
                case_digest: DIGEST.into(),
                status: CaseStatus::Open,
                next_generation: Some(1),
                order_id: ORDER_ID.into(),
                assessed_at: 123,
                reason: ReviewReason::ComponentScarcity,
            })
        );
    }

    #[test]
    fn retry_generation_requires_open_scarcity_and_remaining_capacity() {
        let item = candidate();
        let reviewed = case();
        for (count, expected) in [(0, Some(1)), (1, Some(2)), (2, Some(3)), (3, None)] {
            let result = project(
                &item,
                Some(Outcome::NeedsReview(reviewed.clone())),
                reviewed.clone(),
                DIGEST.into(),
                count,
            )
            .ok();
            assert_eq!(result.and_then(|summary| summary.next_generation), expected);
        }
        let mut release = reviewed.clone();
        release.reason = ReviewReason::ReleaseIntentUnresolved;
        let result = project(
            &item,
            Some(Outcome::NeedsReview(release.clone())),
            release,
            DIGEST.into(),
            0,
        )
        .ok();
        assert_eq!(result.and_then(|summary| summary.next_generation), None);
        let result = project(
            &item,
            Some(Outcome::Late(allocation())),
            reviewed,
            DIGEST.into(),
            1,
        )
        .ok();
        assert_eq!(
            result.map(|summary| (summary.status, summary.next_generation)),
            Some((CaseStatus::Allocated, None))
        );
    }
}
