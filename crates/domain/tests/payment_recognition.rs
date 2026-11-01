//! Literal gross-payment states; no net balance or fulfilment authority.
#[path = "support/payment_observation_fixture.rs"]
mod fixture;
use commerce_domain::{
    Currency,
    payment_attempts::{PaymentError, identities::IdentitySummary, recognition::*},
};
fn summary(ids: &[&str], paid: &[&str]) -> IdentitySummary {
    IdentitySummary {
        provider_ids: ids.iter().map(|s| (*s).into()).collect(),
        paid_provider_ids: paid.iter().map(|s| (*s).into()).collect(),
    }
}
#[test]
fn literal_gross_states_preserve_paid_and_prioritize_identity_ambiguity() {
    for (ids, paid, state, code) in [
        (vec![], vec![], State::Unconfirmed, "unconfirmed"),
        (vec!["tr_A"], vec![], State::Unconfirmed, "unconfirmed"),
        (vec!["tr_A"], vec!["tr_A"], State::Received, "received"),
        (
            vec!["tr_A", "tr_B"],
            vec![],
            State::NeedsReview,
            "needs_review",
        ),
        (
            vec!["tr_A", "tr_B"],
            vec!["tr_A"],
            State::NeedsReview,
            "needs_review",
        ),
        (
            vec!["tr_A", "tr_B"],
            vec!["tr_A", "tr_B"],
            State::NeedsReview,
            "needs_review",
        ),
    ] {
        assert_eq!(derive(&summary(&ids, &paid)), Ok(state));
        assert_eq!(state.code(), code);
    }
}
#[test]
fn malformed_noncanonical_and_unbounded_identity_summaries_fail_closed() {
    for s in [
        summary(&["bad"], &[]),
        summary(&["tr_B", "tr_A"], &[]),
        summary(&["tr_A", "tr_A"], &[]),
        summary(&[], &["tr_A"]),
        summary(&["tr_A", "tr_B"], &["tr_B", "tr_A"]),
        summary(&["tr_A"], &["tr_A", "tr_A"]),
    ] {
        assert_eq!(derive(&s), Err(PaymentError::InvalidSnapshot));
    }
    let bounded = IdentitySummary {
        provider_ids: (0..1000).map(|n| format!("tr_{n:04}")).collect(),
        paid_provider_ids: vec![],
    };
    assert_eq!(derive(&bounded), Ok(State::NeedsReview));
    let mut overflow = bounded;
    overflow.provider_ids.push("tr_1000".into());
    assert_eq!(derive(&overflow), Err(PaymentError::InvalidSnapshot));
}
#[test]
fn recognition_requires_original_money_identity_prefix_state_and_monotonic_bounded_time() {
    let a = fixture::attempt();
    let s = summary(&["tr_A"], &["tr_A"]);
    let r = Recognition {
        id: "7".repeat(32),
        order_id: "2".repeat(32),
        attempt_id: "1".repeat(32),
        resolution_id: "8".repeat(32),
        through_observation: 1,
        recorded_at: 5000,
        state: State::Received,
        currency: Currency::EUR,
        amount_minor: 38245,
    };
    assert_eq!(r.validate(&a, &s, 1, 5000), Ok(()));
    let changes: [fn(&mut Recognition); 11] = [
        |r| r.id = "bad".into(),
        |r| r.resolution_id = "bad".into(),
        |r| r.order_id = "3".repeat(32),
        |r| r.attempt_id = "4".repeat(32),
        |r| r.through_observation = -1,
        |r| r.through_observation = 1001,
        |r| r.through_observation = 2,
        |r| r.recorded_at = 4999,
        |r| r.recorded_at = 9_007_199_254_740_992,
        |r| r.state = State::Unconfirmed,
        |r| r.amount_minor = 38244,
    ];
    for change in changes {
        let mut bad = r.clone();
        change(&mut bad);
        assert_eq!(
            bad.validate(&a, &s, 1, 5000),
            Err(PaymentError::InvalidSnapshot)
        );
    }
    assert_eq!(
        r.validate(&a, &s, 1, 1099),
        Err(PaymentError::InvalidSnapshot)
    );
    let mut bad_attempt = a.clone();
    bad_attempt.binding.amount_minor = -1;
    assert_eq!(
        r.validate(&bad_attempt, &s, 1, 5000),
        Err(PaymentError::InvalidSnapshot)
    );
    for watermark in [0, 1000] {
        let mut end = r.clone();
        end.through_observation = watermark;
        assert_eq!(end.validate(&a, &s, watermark, 5000), Ok(()));
    }
}

#[test]
fn late_paid_and_reordered_open_prefixes_keep_the_paid_identity() {
    use commerce_domain::payment_attempts::{PaymentStatus, identities::summarize};
    let a = fixture::attempt();
    let mut paid = fixture::observation();
    paid.intent.payment = fixture::payment();
    let mut open = paid.clone();
    open.revision = 2;
    open.intent.payment.status = PaymentStatus::Open;
    let prefix = summarize(&a, 1200, &[paid, open]).expect("authenticated full prefix");
    assert_eq!(prefix.provider_ids, vec!["tr_SYNTHETIC_NotARealPayment"]);
    assert_eq!(
        prefix.paid_provider_ids,
        vec!["tr_SYNTHETIC_NotARealPayment"]
    );
    assert_eq!(derive(&prefix), Ok(State::Received));
}
