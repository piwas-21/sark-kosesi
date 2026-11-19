use super::*;
fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).into()).collect()
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
