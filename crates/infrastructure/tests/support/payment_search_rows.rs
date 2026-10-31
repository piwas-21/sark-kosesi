use super::*;
#[path = "../../../domain/tests/support/payment_search_fixture.rs"]
mod fixture;
fn frozen() -> FrozenAttempt {
    let mut a = fixture::attempt();
    let c = crate::payment_test_fixture::config();
    a.binding.configuration_sha256 = c.identity().sha256;
    a.binding.profile = c.identity().profile;
    FrozenAttempt::capture(a.binding, &c).expect("synthetic original")
}
fn row() -> Row {
    let s = fixture::first();
    Row {
        id: s.id.clone(),
        attempt_id: s.attempt_id.clone(),
        effect_key: s.effect_key.clone(),
        revision: s.revision,
        page_number: s.page_number,
        recorded_at: s.recorded_at,
        intent: Some(json(&s.intent)),
        snapshot: Some(json(&s)),
        event_present: true,
    }
}
fn json<T: serde::Serialize>(v: &T) -> Value {
    serde_json::to_value(v).expect("fixture")
}
#[test]
fn exact_closed_page_retains_multiple_hints_and_empty_end_pages() {
    assert!(row().decode(&frozen(), 1200).expect("healthy") == fixture::first());
    let s = fixture::second();
    let mut r = row();
    r.id = s.id.clone();
    r.effect_key = s.effect_key.clone();
    r.revision = 2;
    r.page_number = 2;
    r.recorded_at = 5100;
    r.intent = Some(json(&s.intent));
    r.snapshot = Some(json(&s));
    assert!(r.decode(&frozen(), 1200).expect("healthy end") == s);
}
#[test]
fn every_stored_header_and_event_must_match_the_closed_snapshot() {
    let changes: [fn(&mut Row); 10] = [
        |r| r.id = "8".repeat(32),
        |r| r.attempt_id = "8".repeat(32),
        |r| r.effect_key = "different".into(),
        |r| r.revision = 2,
        |r| r.page_number = 2,
        |r| r.recorded_at = 5001,
        |r| r.intent = None,
        |r| r.intent.as_mut().expect("intent")["next"] = json(&"tr_FALSE"),
        |r| r.snapshot = None,
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
fn root_and_intent_positional_arrays_unknown_fields_or_invalid_original_dispatch_fail() {
    for (path, v) in [
        (
            "",
            serde_json::json!([
                1,
                "9".repeat(32),
                "1".repeat(32),
                "list-a",
                1,
                1,
                5000,
                fixture::first().intent
            ]),
        ),
        (
            "/intent",
            serde_json::json!([null, null, "tr_A", ["tr_ONE", "tr_TWO"], "c".repeat(64)]),
        ),
        ("/page_number", json(&0)),
    ] {
        let mut r = row();
        *r.snapshot
            .as_mut()
            .expect("fixture")
            .pointer_mut(path)
            .expect("path") = v;
        assert_eq!(
            r.decode(&frozen(), 1200).err(),
            Some(PaymentStoreError::InvalidStoredData)
        );
    }
    let mut r = row();
    r.snapshot.as_mut().expect("fixture")["extra"] = json(&true);
    assert_eq!(
        r.decode(&frozen(), 1200).err(),
        Some(PaymentStoreError::InvalidStoredData)
    );
    assert_eq!(
        row().decode(&frozen(), 2000).err(),
        Some(PaymentStoreError::InvalidStoredData)
    );
}
