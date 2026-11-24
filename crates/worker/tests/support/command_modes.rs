use super::*;
fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).into()).collect()
}

#[test]
fn staff_order_reads_accept_only_exact_commands_and_order_identifiers() {
    let order = "0123456789abcdef0123456789abcdef";
    for (command, expected) in [
        (
            "--review-cancellation",
            Mode::ReviewCancellation(order.into()),
        ),
        (
            "--review-cancellation-inbox",
            Mode::ReviewCancellationInbox(Some(order.into())),
        ),
        (
            "--inspect-cancellation-packing",
            Mode::InspectCancellationPacking(order.into()),
        ),
        (
            "--inspect-handover-evidence",
            Mode::InspectHandoverEvidence(order.into()),
        ),
    ] {
        assert!(parse_mode(&args(&[command, order])).is_ok_and(|mode| mode == expected));
        assert!(parse_mode(&args(&[command, "bad"])).is_err());
        assert!(parse_mode(&args(&[command, order, "extra"])).is_err());
    }
    assert!(matches!(
        parse_mode(&args(&["--review-cancellation-inbox"])),
        Ok(Mode::ReviewCancellationInbox(None))
    ));
    assert!(parse_mode(&args(&["--inspect-handover-evidence"])).is_err());
    assert!(parse_mode(&args(&["--unknown-order-read", order])).is_err());
}

#[test]
fn payment_operator_reads_require_exact_internal_identifiers() {
    let id = "0123456789abcdef0123456789abcdef";
    for (command, expected) in [
        (
            "--review-late-stock-inbox",
            Mode::ReviewLateStockInbox(Some(id.into())),
        ),
        (
            "--inspect-refund-proposal",
            Mode::InspectRefundProposal(id.into()),
        ),
    ] {
        assert!(parse_mode(&args(&[command, id])).is_ok_and(|mode| mode == expected));
        assert!(parse_mode(&args(&[command, &id.to_uppercase()])).is_err());
        assert!(parse_mode(&args(&[command, id, "extra"])).is_err());
    }
    assert!(matches!(
        parse_mode(&args(&["--review-late-stock-inbox"])),
        Ok(Mode::ReviewLateStockInbox(None))
    ));
    assert!(parse_mode(&args(&["--inspect-refund-proposal"])).is_err());
}

#[test]
fn reversal_item_commands_require_exact_valid_arguments() {
    let id = "0123456789abcdef0123456789abcdef";
    assert!(matches!(
        parse_mode(&args(&["--reconcile-reversal-items"])),
        Ok(Mode::ReversalItems)
    ));
    for ordinal in ["1", "10"] {
        assert!(matches!(
            parse_mode(&args(&[
                "--enroll-reversal-item",
                id,
                id,
                id,
                ordinal,
                "effect-123"
            ])),
            Ok(Mode::EnrollReversalItem(_))
        ));
    }
    assert!(matches!(
        parse_mode(&args(&["--inspect-reversal-item", id])),
        Ok(Mode::InspectReversalItem(_))
    ));
    assert!(matches!(
        parse_mode(&args(&["--review-reversal-item", id])),
        Ok(Mode::ReviewReversalItem(_))
    ));
    assert!(matches!(
        parse_mode(&args(&["--review-reversal-inbox"])),
        Ok(Mode::ReviewReversalInbox(None))
    ));
    for time in ["0", "9007199254740991"] {
        assert!(matches!(
            parse_mode(&args(&["--review-reversal-inbox", time, id])),
            Ok(Mode::ReviewReversalInbox(Some(_)))
        ));
    }
    for command in [
        vec!["--reconcile-reversal-items", "extra"],
        vec!["--enroll-reversal-item", id, id, id, "0", "key"],
        vec!["--enroll-reversal-item", id, id, id, "11", "key"],
        vec!["--enroll-reversal-item", id, id, id, "01", "key"],
        vec!["--enroll-reversal-item", id, id, "bad", "1", "key"],
        vec!["--enroll-reversal-item", id, id, id, "1", "bad_key"],
        vec!["--inspect-reversal-item", "bad"],
        vec!["--review-reversal-item", "bad"],
        vec!["--review-reversal-item", id, "token"],
        vec!["--review-reversal-inbox", "1"],
        vec!["--review-reversal-inbox", "-1", id],
        vec!["--review-reversal-inbox", "+1", id],
        vec!["--review-reversal-inbox", "01", id],
        vec!["--review-reversal-inbox", "9007199254740992", id],
        vec!["--review-reversal-inbox", "1", "bad"],
        vec!["--review-reversal-inbox", "1", id, "token"],
    ] {
        assert!(parse_mode(&args(&command)).is_err());
    }
}
#[test]
fn reversal_commands_require_exact_valid_arguments() {
    let id = "0123456789abcdef0123456789abcdef";
    assert!(matches!(parse_mode(&args(&[])), Ok(Mode::Probe)));
    assert!(matches!(
        parse_mode(&args(&["--reconcile-reversal-pages"])),
        Ok(Mode::ReversalPages)
    ));
    for kind in ["refund", "chargeback"] {
        assert!(matches!(
            parse_mode(&args(&[
                "--enroll-reversal-scan",
                id,
                id,
                kind,
                "effect-123"
            ])),
            Ok(Mode::EnrollReversal(_))
        ));
    }
    assert!(matches!(
        parse_mode(&args(&["--inspect-reversal-scan", id])),
        Ok(Mode::InspectReversal(_))
    ));
    for command in [
        vec!["--reconcile-reversal-pages", "extra"],
        vec!["--enroll-reversal-scan", id, id, "refund"],
        vec!["--enroll-reversal-scan", id, id, "refund", "key", "extra"],
        vec!["--enroll-reversal-scan", "bad", id, "refund", "key"],
        vec!["--enroll-reversal-scan", id, "bad", "refund", "key"],
        vec!["--enroll-reversal-scan", id, id, "Refund", "key"],
        vec!["--enroll-reversal-scan", id, id, "refund", "secret_value"],
        vec!["--inspect-reversal-scan", "bad"],
    ] {
        assert!(parse_mode(&args(&command)).is_err());
    }
}

#[test]
fn packing_release_requires_exact_confirmed_arguments() {
    let id = "0123456789abcdef0123456789abcdef";
    assert!(matches!(
        parse_mode(&args(&[
            "--release-packing",
            id,
            id,
            "effect-123",
            "--confirm"
        ])),
        Ok(Mode::ReleasePacking { .. })
    ));
    for command in [
        vec!["--release-packing", id, id, "effect-123"],
        vec!["--release-packing", id, id, "effect-123", "confirm"],
        vec!["--release-packing", "bad", id, "effect-123", "--confirm"],
        vec!["--release-packing", id, "bad", "effect-123", "--confirm"],
        vec!["--release-packing", id, id, "bad_key", "--confirm"],
        vec![
            "--release-packing",
            id,
            id,
            "effect-123",
            "--confirm",
            "extra",
        ],
    ] {
        assert!(parse_mode(&args(&command)).is_err());
    }
}

#[test]
fn packing_record_requires_exact_confirmed_arguments() {
    let id = "0123456789abcdef0123456789abcdef";
    assert!(matches!(
        parse_mode(&args(&[
            "--record-packing",
            id,
            "effect-123",
            "fixture.json",
            "--confirm"
        ])),
        Ok(Mode::RecordPacking { .. })
    ));
    for command in [
        vec!["--record-packing", id, "effect-123", "fixture.json"],
        vec![
            "--record-packing",
            id,
            "effect-123",
            "fixture.json",
            "confirm",
        ],
        vec![
            "--record-packing",
            "bad",
            "effect-123",
            "fixture.json",
            "--confirm",
        ],
        vec![
            "--record-packing",
            id,
            "bad_key",
            "fixture.json",
            "--confirm",
        ],
        vec!["--record-packing", id, "effect-123", "", "--confirm"],
        vec![
            "--record-packing",
            id,
            "effect-123",
            "fixture.json",
            "--confirm",
            "extra",
        ],
    ] {
        assert!(parse_mode(&args(&command)).is_err());
    }
}

#[test]
fn operator_recovery_commands_require_exact_confirmation_and_a_file() {
    let id = "0123456789abcdef0123456789abcdef";
    for (command, action) in [
        ("--retry-late-stock", staff_recovery::Action::RetryLateStock),
        (
            "--resume-payment-recovery",
            staff_recovery::Action::ResumePayment,
        ),
    ] {
        let expected = Mode::StaffRecovery {
            action,
            target: id.into(),
            path: "private-intent.json".into(),
        };
        assert!(
            parse_mode(&args(&[command, id, "private-intent.json", "--confirm"]))
                .is_ok_and(|mode| mode == expected)
        );
        for invalid in [
            vec![command, id, "private-intent.json"],
            vec![command, id, "private-intent.json", "true"],
            vec![command, id, "", "--confirm"],
            vec![command, "bad", "private-intent.json", "--confirm"],
            vec![command, id, "private-intent.json", "--confirm", "extra"],
        ] {
            assert!(parse_mode(&args(&invalid)).is_err());
        }
    }
}
