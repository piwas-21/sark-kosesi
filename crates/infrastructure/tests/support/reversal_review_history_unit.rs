//! Closed-history corruption controls for the private review projection.
use super::*;

fn valid_job() -> Job {
    Job {
        id: "1".repeat(32),
        attempt_id: "2".repeat(32),
        observation_id: "3".repeat(32),
        page_id: "4".repeat(32),
        ordinal: 1,
        child_id: "re_test".into(),
        enrollment_key: "fixture".into(),
        profile_id: "pfl_test".into(),
        payment_id: "tr_test".into(),
        kind: ReversalKind::Refund,
        credential_fingerprint: "a".repeat(64),
        state: "review".into(),
        review_reason: Some("invalid".into()),
        receipt_id: None,
        generation: 3,
        failures: 2,
        created_at: 1500,
        updated_at: 1623,
        due_at: 1623,
        lease_until: None,
    }
}
fn history() -> Vec<Operation> {
    [
        (1, 1500, 1501, "transient"),
        (2, 1561, 1562, "transient"),
        (3, 1622, 1623, "invalid"),
    ]
    .into_iter()
    .map(|(generation, started_at, finished_at, outcome)| Operation {
        id: format!("{generation:032x}"),
        generation,
        started_at,
        lease_until: started_at + 60,
        outcome: Some(outcome.into()),
        finished_at: Some(finished_at),
        transactions_valid: true,
        receipt_absent: true,
    })
    .collect()
}
fn rejected(job: &Job, reason: Reason, outcome: &str, operations: &[Operation]) {
    assert!(matches!(
        validate_history(job, reason, outcome, operations),
        Err(RecoveryError::InvalidStoredData)
    ));
}
#[test]
fn bounded_history_requires_complete_sequential_transient_attempts_and_exact_failure_count() {
    let job = valid_job();
    let rows = history();
    assert!(validate_history(&job, Reason::Invalid, "invalid", &rows).is_ok());
    let mut bad = valid_job();
    bad.generation = 0;
    rejected(&bad, Reason::Invalid, "invalid", &rows);
    rejected(&job, Reason::Invalid, "invalid", &rows[..2]);
    let mut bad = valid_job();
    bad.failures = 1;
    rejected(&bad, Reason::Invalid, "invalid", &rows);
    let mut bad = valid_job();
    bad.generation = 2;
    bad.failures = 2;
    rejected(&bad, Reason::RetryExhausted, "transient", &rows[..2]);
    let mut missing_time = rows.clone();
    missing_time[0].finished_at = None;
    rejected(&job, Reason::Invalid, "invalid", &missing_time);
    let mut invalid_id = rows.clone();
    invalid_id[0].id = "bad".into();
    rejected(&job, Reason::Invalid, "invalid", &invalid_id);
    let mut wrong_generation = rows.clone();
    wrong_generation[1].generation = 3;
    rejected(&job, Reason::Invalid, "invalid", &wrong_generation);
    let mut before_creation = rows.clone();
    before_creation[0].started_at = 1499;
    rejected(&job, Reason::Invalid, "invalid", &before_creation);
    let mut before_backoff = rows.clone();
    before_backoff[1].started_at = 1560;
    rejected(&job, Reason::Invalid, "invalid", &before_backoff);
    let mut bad_lease = rows.clone();
    bad_lease[0].lease_until += 1;
    rejected(&job, Reason::Invalid, "invalid", &bad_lease);
    let mut reversed = rows.clone();
    reversed[0].finished_at = Some(1499);
    rejected(&job, Reason::Invalid, "invalid", &reversed);
    let mut overflow = rows.clone();
    overflow[0].finished_at = Some(9_007_199_254_740_992);
    rejected(&job, Reason::Invalid, "invalid", &overflow);
    let mut wrong_outcome = rows.clone();
    wrong_outcome[0].outcome = Some("invalid".into());
    rejected(&job, Reason::Invalid, "invalid", &wrong_outcome);
    let mut wrong_xid = rows.clone();
    wrong_xid[0].transactions_valid = false;
    rejected(&job, Reason::Invalid, "invalid", &wrong_xid);
    let mut unexpected_receipt = rows.clone();
    unexpected_receipt[0].receipt_absent = false;
    rejected(&job, Reason::Invalid, "invalid", &unexpected_receipt);
    let mut late_transient = rows.clone();
    late_transient[0].finished_at = Some(1560);
    rejected(&job, Reason::Invalid, "invalid", &late_transient);
    let mut different_checkpoint = rows.clone();
    different_checkpoint[2].finished_at = Some(1624);
    rejected(&job, Reason::Invalid, "invalid", &different_checkpoint);
    let mut uncertain = rows.clone();
    uncertain[2].outcome = Some("uncertain".into());
    rejected(&job, Reason::Uncertain, "uncertain", &uncertain);
    uncertain[2].finished_at = Some(1682);
    let mut later_job = valid_job();
    later_job.updated_at = 1682;
    assert!(validate_history(&later_job, Reason::Uncertain, "uncertain", &uncertain).is_ok());
}
