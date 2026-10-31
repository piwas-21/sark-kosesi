//! Hostile persisted audit controls and bounded transition boundaries.
use super::*;
fn before() -> Job {
    Job {
        attempt_id: "a".repeat(32),
        source_event_id: "b".repeat(32),
        created_at: 1000,
        updated_at: 1500,
        due_at: 1500,
        generation: 12,
        resume_revision: 0,
        lease_until: None,
        phase: Phase::NeedsReview,
        page_id: None,
        candidate_index: 0,
        operation: 10,
        polls: 0,
        failures: 5,
        review_reason: Some(ReviewReason::RetryExhausted),
    }
}
fn stored() -> Stored {
    Stored {
        attempt_id: "a".repeat(32),
        revision: 1,
        actor: "finance".into(),
        reason: "incident".into(),
        profile_id: "pfl_TEST".into(),
        before_state: Some(serde_json::to_value(before()).expect("before")),
        after_state: Some(
            serde_json::to_value(resumed(&before(), 1500).expect("after")).expect("value"),
        ),
    }
}
#[test]
fn persisted_resume_decoder_rejects_missing_open_or_incoherent_snapshots() {
    assert!(stored().decode().is_ok());
    for case in [
        "missing-before",
        "missing-after",
        "null-before",
        "null-after",
        "open-before",
        "open-after",
        "bad-job-before",
        "bad-job-after",
        "not-retry",
        "revision",
        "revision-mismatch",
        "identity",
        "changed-progress",
    ] {
        let mut row = stored();
        match case {
            "missing-before" => row.before_state = None,
            "missing-after" => row.after_state = None,
            "null-before" => row.before_state = Some(serde_json::Value::Null),
            "null-after" => row.after_state = Some(serde_json::Value::Null),
            "open-before" => row.before_state.as_mut().expect("object")["extra"] = true.into(),
            "open-after" => row.after_state.as_mut().expect("object")["extra"] = true.into(),
            "bad-job-before" => {
                row.before_state.as_mut().expect("object")["generation"] = (-1).into()
            }
            "bad-job-after" => {
                row.after_state.as_mut().expect("object")["generation"] = (-1).into()
            }
            "not-retry" => {
                row.before_state.as_mut().expect("object")["review_reason"] = "paid_observed".into()
            }
            "revision" => row.revision = 4,
            "revision-mismatch" => row.revision = 2,
            "identity" => row.attempt_id = "c".repeat(32),
            "changed-progress" => row.after_state.as_mut().expect("object")["operation"] = 9.into(),
            _ => unreachable!(),
        }
        assert_eq!(
            row.decode().err(),
            Some(OperatorError::InvalidStoredData),
            "{case}"
        );
    }
}
#[test]
fn transient_resume_preserves_hard_limits_and_cannot_rewind_time() {
    assert!(resumed(&before(), 1500).is_ok());
    for field in [
        "phase",
        "reason",
        "failures",
        "polls",
        "generation",
        "resume-revision",
        "rewind",
        "overflow",
    ] {
        let mut job = before();
        let mut at = 1500;
        match field {
            "phase" => job.phase = Phase::Search,
            "reason" => job.review_reason = Some(ReviewReason::PaidObserved),
            "failures" => job.failures = 4,
            "polls" => job.polls = 10,
            "generation" => job.generation = 1500,
            "resume-revision" => job.resume_revision = 3,
            "rewind" => at = 1499,
            "overflow" => at = 9_007_199_254_740_992,
            _ => unreachable!(),
        }
        assert_eq!(resumed(&job, at).err().map(|_| true), Some(true), "{field}");
    }
    let mut job = before();
    job.page_id = Some("c".repeat(32));
    assert!(resumed(&job, 1500).expect("resume retained page").phase == Phase::Candidates);
}
#[test]
fn boundary_errors_are_static_and_never_contain_storage_or_session_detail() {
    for (source, target) in [
        (StaffError::Denied, OperatorError::Denied),
        (
            StaffError::ClockUnavailable,
            OperatorError::ClockUnavailable,
        ),
        (StaffError::InvalidRequest, OperatorError::Storage),
    ] {
        assert_eq!(OperatorError::from(source), target);
    }
    assert_eq!(
        OperatorError::from(sqlx::Error::RowNotFound),
        OperatorError::Storage
    );
    for (source, target) in [
        (
            RecoveryError::ClockUnavailable,
            OperatorError::ClockUnavailable,
        ),
        (RecoveryError::Storage, OperatorError::Storage),
        (
            RecoveryError::InvalidRequest,
            OperatorError::InvalidStoredData,
        ),
    ] {
        assert_eq!(OperatorError::from(source), target);
    }
    assert!(!text(""));
    assert!(!text(&"a".repeat(501)));
    assert!(!text("test\n"));
    assert!(text("incident checked"));
}
