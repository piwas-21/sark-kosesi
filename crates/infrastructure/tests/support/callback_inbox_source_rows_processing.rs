//! Typed private source header controls with paired healthy boundaries.
use super::*;
fn source() -> SourceRow {
    SourceRow {
        intake_id: "a".repeat(32),
        profile_id: "pfl_BOUND".into(),
        provider_id: "tr_BOUND".into(),
        received_at: 1500,
        created_at: 1501,
        event_id: Some("b".repeat(32)),
        source_present: true,
        receipt_present: true,
    }
}
#[test]
fn redacted_exception_still_requires_typed_source_raw_receipt_clock_and_optional_exact_event_id() {
    let good = decode(source()).expect("healthy complete source");
    assert_eq!(good.event_id, Some("b".repeat(32)));
    assert_eq!(good.created_at, 1501);
    for field in [
        "id", "profile", "provider", "received", "source", "receipt", "old", "overflow", "event",
    ] {
        let mut bad = source();
        match field {
            "id" => bad.intake_id = "x".repeat(32),
            "profile" => bad.profile_id = "pfl_".into(),
            "provider" => bad.provider_id = "tr_".into(),
            "received" => bad.received_at = -1,
            "source" => bad.source_present = false,
            "receipt" => bad.receipt_present = false,
            "old" => bad.created_at = 1499,
            "overflow" => bad.created_at = 9_007_199_254_740_992,
            _ => bad.event_id = Some("x".repeat(32)),
        }
        assert_eq!(
            decode(bad).err(),
            Some(PaymentStoreError::InvalidStoredData),
            "{field}"
        );
    }
    let mut missing_event = source();
    missing_event.event_id = None;
    assert!(
        decode(missing_event)
            .expect("raw receipt allows redacted missing-event stop")
            .event_id
            .is_none()
    );
    for at in [1500, 9_007_199_254_740_991] {
        let mut boundary = source();
        boundary.created_at = at;
        assert_eq!(
            decode(boundary)
                .expect("inclusive clock boundary")
                .created_at,
            at
        );
    }
    assert!(decode(source()).is_ok());
}
