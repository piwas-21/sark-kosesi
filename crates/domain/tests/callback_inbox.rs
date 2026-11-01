//! Literal private inbox budgets and hostile header controls, not provider proof.
use commerce_domain::payment_attempts::{PaymentError, callback_inbox::*};
use serde_json::json;

fn intake() -> IntakeSnapshot {
    IntakeSnapshot {
        schema_version: 1,
        id: "1".repeat(32),
        profile_id: "pfl_PRIVATE_TEST".into(),
        provider_id: "tr_PRIVATE_TEST".into(),
        received_at: 1500,
    }
}
fn job() -> CallbackJob {
    CallbackJob {
        intake_id: "1".repeat(32),
        source_event_id: "2".repeat(32),
        created_at: 1500,
        updated_at: 1500,
        due_at: 1500,
        generation: 1,
        lease_until: Some(1560),
        state: JobState::Leased,
        review_reason: None,
        settled: false,
    }
}
#[test]
fn intake_headers_are_bounded_closed_and_independent_of_any_order() {
    let controls: [fn(&mut IntakeSnapshot); 13] = [
        |i| i.schema_version = 2,
        |i| i.id = "g".repeat(32),
        |i| i.id = "1".repeat(33),
        |i| i.profile_id = "pfl_".into(),
        |i| i.profile_id = "other".into(),
        |i| i.profile_id = "pfl_bad\n".into(),
        |i| i.provider_id = "tr_".into(),
        |i| i.provider_id = "other".into(),
        |i| i.provider_id = "tr_bad\n".into(),
        |i| i.provider_id = "tr_space ".into(),
        |i| i.provider_id = format!("tr_{}", "x".repeat(254)),
        |i| i.received_at = -1,
        |i| i.received_at = 9_007_199_254_740_992,
    ];
    assert!(intake().validate().is_ok());
    for control in controls {
        let mut i = intake();
        control(&mut i);
        assert_eq!(i.validate(), Err(PaymentError::InvalidSnapshot));
    }
    for at in [0, 9_007_199_254_740_991] {
        let mut i = intake();
        i.received_at = at;
        assert!(i.validate().is_ok());
    }
    let mut value = serde_json::to_value(intake()).expect("literal fixture");
    value["status"] = "paid".into();
    assert!(serde_json::from_value::<IntakeSnapshot>(value).is_err());
}
#[test]
fn job_headers_reject_every_invalid_lease_clock_budget_and_terminal_shape() {
    let controls: [fn(&mut CallbackJob); 26] = [
        |j| j.intake_id = "invalid".into(),
        |j| j.source_event_id = "invalid".into(),
        |j| j.created_at = -1,
        |j| j.created_at = 9_007_199_254_740_992,
        |j| j.updated_at = 1499,
        |j| j.updated_at = 9_007_199_254_740_992,
        |j| j.due_at = 1499,
        |j| j.due_at = 9_007_199_254_740_992,
        |j| j.generation = -1,
        |j| j.generation = 6,
        |j| j.lease_until = None,
        |j| j.generation = 0,
        |j| j.lease_until = Some(1500),
        |j| j.lease_until = Some(9_007_199_254_740_992),
        |j| j.lease_until = Some(1559),
        |j| j.review_reason = Some(ReviewReason::Unmatched),
        |j| {
            j.state = JobState::NeedsReview;
            j.lease_until = None;
        },
        |j| {
            j.state = JobState::Pending;
            j.lease_until = None;
            j.updated_at = 1501;
        },
        |j| {
            j.state = JobState::Pending;
            j.lease_until = None;
            j.generation = 5;
        },
        |j| {
            j.state = JobState::Done;
            j.lease_until = None;
            j.generation = 0;
        },
        |j| {
            j.state = JobState::Pending;
            j.lease_until = None;
            j.generation = 0;
            j.updated_at = 1501;
            j.due_at = 1501;
        },
        |j| {
            j.state = JobState::Pending;
            j.lease_until = None;
            j.generation = 0;
            j.due_at = 1501;
        },
        |j| j.settled = true,
        |j| {
            j.state = JobState::Done;
            j.lease_until = None;
            j.due_at = 1501;
        },
        |j| {
            j.state = JobState::NeedsReview;
            j.lease_until = None;
            j.review_reason = Some(ReviewReason::RetryExhausted);
        },
        |j| {
            j.state = JobState::NeedsReview;
            j.lease_until = None;
            j.review_reason = Some(ReviewReason::GenerationLimit);
        },
    ];
    assert!(job().validate().is_ok());
    for control in controls {
        let mut j = job();
        control(&mut j);
        assert_eq!(j.validate(), Err(PaymentError::InvalidSnapshot));
    }
    for generation in 1..=5 {
        let mut j = job();
        j.generation = generation;
        assert!(j.validate().is_ok());
    }
    for reason in [
        ReviewReason::RetryExhausted,
        ReviewReason::CustodyInvalid,
        ReviewReason::Unmatched,
        ReviewReason::GenerationLimit,
        ReviewReason::PaidObserved,
        ReviewReason::Ambiguous,
    ] {
        let mut j = job();
        j.state = JobState::NeedsReview;
        j.lease_until = None;
        j.review_reason = Some(reason);
        j.generation = 5;
        assert!(j.validate().is_ok());
    }
    for settled in [false, true] {
        let mut j = job();
        j.state = JobState::Done;
        j.lease_until = None;
        j.settled = settled;
        assert!(j.validate().is_ok());
    }
    let mut initial = job();
    initial.state = JobState::Pending;
    initial.lease_until = None;
    initial.generation = 0;
    assert!(initial.validate().is_ok());
    let mut unknown = serde_json::to_value(&initial).expect("fixture");
    unknown["unknown"] = true.into();
    assert!(serde_json::from_value::<CallbackJob>(unknown).is_err());
    for state in ["unknown", "search"] {
        assert!(serde_json::from_value::<JobState>(json!(state)).is_err());
    }
    assert!(serde_json::from_value::<ReviewReason>(json!("silently_restart")).is_err());
}
#[test]
fn private_limits_are_literal_bounded_and_a_fifth_generation_stops() {
    assert_eq!(LEASE_SECONDS, 60);
    assert_eq!(MAX_BATCH, 25);
    assert_eq!(MAX_GENERATIONS, 5);
    assert_eq!(GLOBAL_CAPACITY, 200);
    assert_eq!(PROFILE_CAPACITY, 50);
    assert_eq!(GLOBAL_PER_MINUTE, 120);
    assert_eq!(PROFILE_PER_MINUTE, 30);
    for (generation, delay) in [(1, 60), (2, 120), (3, 240), (4, 480)] {
        assert_eq!(retry_delay(generation), Ok(delay));
    }
    for generation in [i64::MIN, -1, 0, 5, 6, i64::MAX] {
        assert_eq!(retry_delay(generation), Err(PaymentError::InvalidRequest));
    }
}
