//! Independent literal field/time/status oracles; no financial transition occurs.
#[path = "support/payment_observation_fixture.rs"]
mod fixture;
use commerce_domain::payment_attempts::{observations::*, *};
use fixture::*;
#[test]
fn genuine_late_paid_and_rotated_credential_facts_do_not_depend_on_hold_liveness() {
    let a = attempt();
    let before = a.clone();
    let s = observation();
    assert_eq!(s.recorded_at, 5000);
    assert_eq!(s.intent.payment.amount_minor, 38245);
    assert_eq!(s.intent.credential_fingerprint, "c".repeat(64));
    assert_eq!(s.validate(&a, 1200), Ok(()));
    assert!(a == before);
    assert!(matches!(s.intent.payment.status, PaymentStatus::Paid));
}
#[test]
fn every_closed_provider_status_is_evidence_without_inferring_current_state() {
    for (status, literal) in [
        (PaymentStatus::Open, "open"),
        (PaymentStatus::Pending, "pending"),
        (PaymentStatus::Authorized, "authorized"),
        (PaymentStatus::Paid, "paid"),
        (PaymentStatus::Canceled, "canceled"),
        (PaymentStatus::Expired, "expired"),
        (PaymentStatus::Failed, "failed"),
    ] {
        let mut s = observation();
        s.intent.payment.status = status;
        assert_eq!(s.validate(&attempt(), 1200), Ok(()));
        assert_eq!(
            serde_json::to_value(s.intent.payment.status).expect("status"),
            serde_json::json!(literal)
        );
    }
}
#[test]
fn observation_identity_effect_revision_schema_and_time_have_checked_bounds() {
    let changes: [fn(&mut ObservationSnapshot); 9] = [
        |s| s.schema_version = 2,
        |s| s.id = "bad".into(),
        |s| s.attempt_id = "8".repeat(32),
        |s| s.effect_key = "".into(),
        |s| s.effect_key = "bad effect".into(),
        |s| s.revision = 0,
        |s| s.revision = 9_007_199_254_740_992,
        |s| s.recorded_at = 1199,
        |s| s.recorded_at = 9_007_199_254_740_992,
    ];
    for change in changes {
        let mut s = observation();
        change(&mut s);
        assert_eq!(
            s.validate(&attempt(), 1200),
            Err(PaymentError::InvalidSnapshot)
        );
    }
    let mut max = observation();
    max.revision = 9_007_199_254_740_991;
    max.recorded_at = 9_007_199_254_740_991;
    assert_eq!(max.validate(&attempt(), 1200), Ok(()));
}
#[test]
fn original_attempt_and_valid_committed_dispatch_time_are_required() {
    for started in [1099, 2000, 5000] {
        assert!(observation().validate(&attempt(), started).is_err());
    }
    let mut invalid = attempt();
    invalid.schema_version = 2;
    assert!(observation().validate(&invalid, 1200).is_err());
    assert_eq!(observation().validate(&attempt(), 1100), Ok(()));
}
#[test]
fn actual_fetch_fingerprint_and_every_original_payment_binding_remain_mandatory() {
    for bad in ["".into(), "a".repeat(63), "A".repeat(64), "g".repeat(64)] {
        let mut s = observation();
        s.intent.credential_fingerprint = bad;
        assert_eq!(
            s.validate(&attempt(), 1200),
            Err(PaymentError::InvalidSnapshot)
        );
    }
    let changes: [fn(&mut ProviderPayment); 7] = [
        |p| p.resource = "refund".into(),
        |p| p.id = "wrong".into(),
        |p| p.profile_id = "pfl_OTHER".into(),
        |p| p.amount_minor = 38244,
        |p| p.metadata_schema_version = 2,
        |p| p.order_id = "8".repeat(32),
        |p| p.attempt_id = "8".repeat(32),
    ];
    for change in changes {
        let mut s = observation();
        change(&mut s.intent.payment);
        assert_eq!(
            s.validate(&attempt(), 1200),
            Err(PaymentError::ProviderMismatch)
        );
    }
}
#[test]
fn minimal_closed_snapshot_roundtrips_without_provider_details_or_unknown_fields() {
    let original = observation();
    let value = serde_json::to_value(&original).expect("JSON");
    let decoded: ObservationSnapshot =
        serde_json::from_value(value.clone()).expect("closed record");
    assert!(decoded == original);
    for path in ["", "/intent", "/intent/payment"] {
        let mut v = value.clone();
        v.pointer_mut(path)
            .expect("object")
            .as_object_mut()
            .expect("object")
            .insert("unknown".into(), serde_json::json!(true));
        assert!(serde_json::from_value::<ObservationSnapshot>(v).is_err());
    }
    for (path, v) in [
        ("/intent/payment/mode", serde_json::json!("live")),
        ("/intent/payment/currency", serde_json::json!("USD")),
        ("/intent/payment/status", serde_json::json!("new_status")),
        ("/revision", serde_json::json!(1.5)),
    ] {
        let mut changed = value.clone();
        *changed.pointer_mut(path).expect("field") = v;
        assert!(serde_json::from_value::<ObservationSnapshot>(changed).is_err());
    }
    for excluded in [
        "details",
        "billingAddress",
        "shippingAddress",
        "checkout_url",
        "raw_key",
    ] {
        assert!(
            value
                .pointer(&format!("/intent/payment/{excluded}"))
                .is_none()
        );
    }
}
