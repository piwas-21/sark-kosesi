//! Independent stored-job decoding controls, with synthetic identifiers only.
use super::*;
fn pending() -> Row {
    Row {
        id: "a".repeat(32),
        attempt_id: "b".repeat(32),
        observation_id: "c".repeat(32),
        enrollment_key: "enroll-1".into(),
        profile_id: "pfl_sample".into(),
        payment_id: "tr_sample".into(),
        reversal_kind: "refund".into(),
        credential_fingerprint: "d".repeat(64),
        state: "pending".into(),
        review_reason: None,
        tip_page_id: None,
        generation: 0,
        failures: 0,
        created_at: 100,
        updated_at: 100,
        due_at: 100,
        lease_until: None,
    }
}
#[test]
fn accepts_each_permitted_state_kind_and_review_reason() {
    assert!(decode(pending()).is_ok());
    let mut chargeback = pending();
    chargeback.reversal_kind = "chargeback".into();
    assert!(decode(chargeback).is_ok());
    let mut leased = pending();
    leased.state = "leased".into();
    leased.generation = 1;
    leased.lease_until = Some(160);
    assert!(decode(leased).is_ok());
    for reason in [
        "ended",
        "scan_limit",
        "uncertain",
        "invalid",
        "retry_exhausted",
    ] {
        let mut review = pending();
        review.state = "review".into();
        review.review_reason = Some(reason.into());
        review.generation = 40;
        review.failures = 3;
        review.tip_page_id = Some("e".repeat(32));
        assert!(decode(review).is_ok(), "{reason}");
    }
}
#[test]
fn rejects_each_damaged_identity_clock_and_checkpoint_shape() {
    type Change = (&'static str, fn(&mut Row));
    let changes: &[Change] = &[
        ("job id", |r| r.id = "invalid".into()),
        ("attempt", |r| r.attempt_id = "invalid".into()),
        ("observation", |r| r.observation_id = "invalid".into()),
        ("key", |r| r.enrollment_key = "bad key".into()),
        ("profile prefix", |r| r.profile_id = "bad".into()),
        ("empty profile", |r| r.profile_id = "pfl_".into()),
        ("long profile", |r| {
            r.profile_id = format!("pfl_{}", "x".repeat(253))
        }),
        ("payment prefix", |r| r.payment_id = "bad".into()),
        ("empty payment", |r| r.payment_id = "tr_".into()),
        ("long payment", |r| {
            r.payment_id = format!("tr_{}", "x".repeat(254))
        }),
        ("profile whitespace", |r| r.profile_id = "pfl_name ".into()),
        ("payment controls", |r| r.payment_id = "tr_a\nb".into()),
        ("kind", |r| r.reversal_kind = "Refund".into()),
        ("fingerprint", |r| r.credential_fingerprint = "z".repeat(64)),
        ("negative generation", |r| r.generation = -1),
        ("generation limit", |r| r.generation = 41),
        ("negative failures", |r| r.failures = -1),
        ("failures limit", |r| r.failures = 4),
        ("created", |r| r.created_at = -1),
        ("updated", |r| r.updated_at = 99),
        ("due", |r| r.due_at = 99),
        ("tip", |r| r.tip_page_id = Some("bad".into())),
        ("state", |r| r.state = "other".into()),
        ("missing lease", |r| r.state = "leased".into()),
        ("unexpected lease", |r| r.lease_until = Some(160)),
        ("missing review reason", |r| r.state = "review".into()),
        ("unexpected review reason", |r| {
            r.review_reason = Some("ended".into())
        }),
        ("unknown reason", |r| {
            r.state = "review".into();
            r.review_reason = Some("unknown".into())
        }),
        ("short lease", |r| {
            r.state = "leased".into();
            r.generation = 1;
            r.lease_until = Some(159)
        }),
        ("long lease", |r| {
            r.state = "leased".into();
            r.generation = 1;
            r.lease_until = Some(161)
        }),
        ("zero generation lease", |r| {
            r.state = "leased".into();
            r.lease_until = Some(160)
        }),
        ("pending due before updated", |r| {
            r.updated_at = 101;
            r.due_at = 100
        }),
        ("review due differs", |r| {
            r.state = "review".into();
            r.review_reason = Some("ended".into());
            r.due_at = 101
        }),
        ("exhausted pending", |r| r.generation = 40),
        ("three failures pending", |r| r.failures = 3),
    ];
    for (label, change) in changes {
        let mut r = pending();
        change(&mut r);
        assert!(
            matches!(decode(r), Err(RecoveryError::InvalidStoredData)),
            "{label}"
        );
    }
}
