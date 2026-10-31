//! Literal damaged-import controls for the bounded stored-fact decoder.
use super::*;
#[path = "../../../domain/tests/support/payment_observation_fixture.rs"]
mod fixture;
fn frozen() -> FrozenAttempt {
    let mut a = fixture::attempt();
    let c = crate::payment_test_fixture::config();
    let current = c.identity();
    a.binding.profile = current.profile;
    a.binding.configuration_sha256 = current.sha256;
    FrozenAttempt::capture(a.binding, &c).expect("synthetic original request")
}
fn row() -> Row {
    let s = fixture::observation();
    Row {
        id: s.id.clone(),
        attempt_id: s.attempt_id.clone(),
        effect_key: s.effect_key.clone(),
        revision: s.revision,
        recorded_at: s.recorded_at,
        provider_id: s.intent.payment.id.clone(),
        credential_fingerprint: s.intent.credential_fingerprint.clone(),
        status: "paid".into(),
        snapshot: Some(serde_json::to_value(s).expect("closed fixture")),
        event_present: true,
    }
}
#[test]
fn exact_late_fetched_fact_is_read_without_inventing_current_state() {
    let s = row()
        .decode(&frozen(), 1200)
        .expect("healthy closed projection");
    assert_eq!(s.recorded_at, 5000);
    assert_eq!(s.intent.payment.amount_minor, 38245);
    assert!(matches!(
        s.intent.payment.status,
        commerce_domain::payment_attempts::PaymentStatus::Paid
    ));
}
#[test]
fn every_stored_header_is_bound_to_the_validated_original_fact() {
    let changes: [fn(&mut Row); 9] = [
        |r| r.id = "8".repeat(32),
        |r| r.attempt_id = "8".repeat(32),
        |r| r.effect_key = "changed".into(),
        |r| r.revision = 2,
        |r| r.recorded_at = 5001,
        |r| r.provider_id = "tr_CHANGED".into(),
        |r| r.credential_fingerprint = "d".repeat(64),
        |r| r.status = "pending".into(),
        |r| r.event_present = false,
    ];
    for change in changes {
        let mut r = row();
        change(&mut r);
        assert_eq!(
            r.decode(&frozen(), 1200).err(),
            Some(PaymentStoreError::InvalidStoredData)
        );
    }
}
#[test]
fn missing_unknown_or_populated_positional_objects_never_decode_as_stored_facts() {
    let original = fixture::observation();
    let root_array = serde_json::json!([
        1,
        original.id,
        original.attempt_id,
        original.effect_key,
        1,
        5000,
        original.intent
    ]);
    let intent_array = serde_json::json!([fixture::payment(), "c".repeat(64)]);
    let payment_array = serde_json::json!([
        "payment",
        "tr_SYNTHETIC_NotARealPayment",
        "pfl_SYNTHETIC_NotARealProfile",
        "test",
        "EUR",
        38245,
        1,
        "2".repeat(32),
        "1".repeat(32),
        "paid"
    ]);
    let mut bad = row();
    bad.snapshot = None;
    assert_eq!(
        bad.decode(&frozen(), 1200).err(),
        Some(PaymentStoreError::InvalidStoredData)
    );
    for (pointer, value) in [
        ("", root_array),
        ("/intent", intent_array),
        ("/intent/payment", payment_array),
        ("/intent/payment/status", serde_json::json!("unknown")),
    ] {
        let mut r = row();
        *r.snapshot
            .as_mut()
            .expect("fixture")
            .pointer_mut(pointer)
            .expect("path") = value;
        assert_eq!(
            r.decode(&frozen(), 1200).err(),
            Some(PaymentStoreError::InvalidStoredData)
        );
    }
    let mut r = row();
    r.snapshot
        .as_mut()
        .expect("fixture")
        .as_object_mut()
        .expect("object")
        .insert("extra".into(), serde_json::json!(true));
    assert_eq!(
        r.decode(&frozen(), 1200).err(),
        Some(PaymentStoreError::InvalidStoredData)
    );
}
#[test]
fn coherent_snapshot_still_cannot_replace_the_original_amount_or_dispatch_proof() {
    let mut r = row();
    r.snapshot.as_mut().expect("fixture")["intent"]["payment"]["amount_minor"] =
        serde_json::json!(38244);
    assert_eq!(
        r.decode(&frozen(), 1200).err(),
        Some(PaymentStoreError::InvalidStoredData)
    );
    assert_eq!(
        row().decode(&frozen(), 2000).err(),
        Some(PaymentStoreError::InvalidStoredData)
    );
    assert!(row().decode(&frozen(), 1200).is_ok());
}
