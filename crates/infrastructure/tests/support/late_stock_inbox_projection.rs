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
