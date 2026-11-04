//! Literal persisted-receipt controls without provider or database access.
use super::*;
fn frozen() -> FrozenAttempt {
    crate::mollie::tests::captured(38245).1
}
fn receipt(frozen: &FrozenAttempt) -> ReversalReceipt {
    ReversalReceipt {
        id: "3".repeat(32),
        order_id: frozen.snapshot().binding.order_id.clone(),
        attempt_id: frozen.snapshot().binding.id.clone(),
        effect_key: "synthetic-reversal".into(),
        payment_id: "tr_SYNTHETIC".into(),
        child_id: "re_SYNTHETIC".into(),
        phase: ReversalPhase::RefundPending,
        amount_minor: 1234,
        credential_fingerprint: "d".repeat(64),
        source_created_seconds: 1_710_926_017,
        source_created_nanos: 123_456_789,
        source_reversed_seconds: None,
        source_reversed_nanos: None,
        recorded_at: 1500,
        event_id: "4".repeat(32),
        replayed: true,
    }
}
fn denied(r: &ReversalReceipt, frozen: &FrozenAttempt) {
    assert_eq!(
        validate(r, frozen, 1500),
        Err(PaymentStoreError::InvalidStoredData)
    );
}

#[test]
fn healthy_rotated_key_and_over_original_evidence_do_not_change_the_frozen_attempt() {
    let frozen = frozen();
    let original = frozen.snapshot().clone();
    let mut r = receipt(&frozen);
    assert_ne!(
        r.credential_fingerprint,
        frozen.snapshot().binding.profile.credential_fingerprint
    );
    assert!(validate(&r, &frozen, 1500).is_ok());
    // A larger provider amount remains evidence, with no change to original money or authority.
    r.amount_minor = frozen.snapshot().binding.amount_minor + 1;
    assert!(validate(&r, &frozen, 1500).is_ok());
    assert!(frozen.snapshot() == &original);
    r.amount_minor = 9_007_199_254_740_991;
    assert!(validate(&r, &frozen, 1500).is_ok());
}

#[test]
fn stored_identity_scope_and_money_fields_are_independently_checked() {
    let frozen = frozen();
    type Mutation = (&'static str, fn(&mut ReversalReceipt));
    let changes: [Mutation; 18] = [
        ("observation id", |r| r.id = "x".repeat(32)),
        ("event id", |r| r.event_id = "1".repeat(31)),
        ("order shape", |r| r.order_id = "bad".into()),
        ("order binding", |r| r.order_id = "8".repeat(32)),
        ("attempt shape", |r| r.attempt_id = "bad".into()),
        ("attempt binding", |r| r.attempt_id = "8".repeat(32)),
        ("effect key", |r| r.effect_key = "wrong key".into()),
        ("payment prefix", |r| r.payment_id = "xx_payment".into()),
        ("empty payment id", |r| r.payment_id = "tr_".into()),
        ("long payment id", |r| {
            r.payment_id = format!("tr_{}", "x".repeat(254))
        }),
        ("payment whitespace", |r| {
            r.payment_id = "tr_payment ".into()
        }),
        ("payment control", |r| r.payment_id = "tr_pay\nment".into()),
        ("child prefix", |r| r.child_id = "chb_wrong".into()),
        ("empty child id", |r| r.child_id = "re_".into()),
        ("zero amount", |r| r.amount_minor = 0),
        ("unsafe amount", |r| r.amount_minor = 9_007_199_254_740_992),
        ("fingerprint size", |r| {
            r.credential_fingerprint = "d".repeat(63)
        }),
        ("fingerprint alphabet", |r| {
            r.credential_fingerprint = "G".repeat(64)
        }),
    ];
    for (label, change) in changes {
        let mut r = receipt(&frozen);
        change(&mut r);
        assert_eq!(
            validate(&r, &frozen, 1500),
            Err(PaymentStoreError::InvalidStoredData),
            "{label}"
        );
    }
    assert!(validate(&receipt(&frozen), &frozen, 1500).is_ok());
}

#[test]
fn source_and_recording_time_bounds_are_independently_enforced() {
    let frozen = frozen();
    let changes: [fn(&mut ReversalReceipt); 6] = [
        |r| r.source_created_seconds = -62_167_219_201,
        |r| r.source_created_seconds = 253_402_300_800,
        |r| r.source_created_nanos = -1,
        |r| r.source_created_nanos = 1_000_000_000,
        |r| r.recorded_at = 1499,
        |r| r.recorded_at = MAX_QUOTE_UNIX_SECONDS + 1,
    ];
    for change in changes {
        let mut r = receipt(&frozen);
        change(&mut r);
        denied(&r, &frozen);
    }
    for (seconds, nanos) in [(-62_167_219_200, 0), (253_402_300_799, 999_999_999)] {
        let mut r = receipt(&frozen);
        r.source_created_seconds = seconds;
        r.source_created_nanos = nanos;
        assert!(validate(&r, &frozen, 1500).is_ok());
    }
}

#[test]
fn chargeback_phase_and_reversal_timestamp_are_bound() {
    let frozen = frozen();
    let mut r = receipt(&frozen);
    r.child_id = "chb_SYNTHETIC".into();
    for phase in [
        ReversalPhase::ChargebackReversalUnknown,
        ReversalPhase::ChargebackReceived,
    ] {
        r.phase = phase;
        assert!(validate(&r, &frozen, 1500).is_ok());
    }
    r.phase = ReversalPhase::ChargebackReversed;
    denied(&r, &frozen);
    r.source_reversed_seconds = Some(1_710_926_017);
    denied(&r, &frozen);
    r.source_reversed_nanos = Some(123_456_789);
    assert!(validate(&r, &frozen, 1500).is_ok());
    r.source_reversed_nanos = Some(123_456_788);
    denied(&r, &frozen);
    r.source_reversed_nanos = Some(1_000_000_000);
    denied(&r, &frozen);
    r.source_reversed_nanos = Some(-1);
    denied(&r, &frozen);
    r.source_reversed_nanos = Some(0);
    r.source_reversed_seconds = Some(253_402_300_800);
    denied(&r, &frozen);
    r.source_reversed_seconds = Some(-62_167_219_201);
    denied(&r, &frozen);
    r.source_reversed_seconds = None;
    denied(&r, &frozen);
    r.source_reversed_seconds = Some(1_710_926_018);
    r.phase = ReversalPhase::ChargebackReceived;
    denied(&r, &frozen);
    r.phase = ReversalPhase::RefundPending;
    r.child_id = "re_SYNTHETIC".into();
    denied(&r, &frozen);
}

#[test]
fn phase_labels_are_closed_and_explicit() {
    let cases = [
        ("refund_queued", ReversalPhase::RefundQueued),
        ("refund_pending", ReversalPhase::RefundPending),
        ("refund_processing", ReversalPhase::RefundProcessing),
        ("refund_refunded", ReversalPhase::RefundRefunded),
        ("refund_failed", ReversalPhase::RefundFailed),
        ("refund_canceled", ReversalPhase::RefundCanceled),
        (
            "chargeback_reversal_unknown",
            ReversalPhase::ChargebackReversalUnknown,
        ),
        ("chargeback_received", ReversalPhase::ChargebackReceived),
        ("chargeback_reversed", ReversalPhase::ChargebackReversed),
    ];
    for (label, phase) in cases {
        assert_eq!(parse_phase(label), Some(phase));
        assert_eq!(phase_name(phase), label);
    }
    for invalid in [
        "",
        "paid",
        "refund_cancelled",
        "chargeback_unknown",
        "REFUND_PENDING",
    ] {
        assert_eq!(parse_phase(invalid), None);
    }
}
