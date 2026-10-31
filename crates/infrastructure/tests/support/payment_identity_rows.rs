//! Closed minimal-header controls, paired with healthy decoding and SQL integration.
use super::*;
fn row() -> Row {
    Row {
        id: "1".repeat(32),
        attempt_id: "2".repeat(32),
        effect_key: "resolve".into(),
        revision: 1,
        through_observation: 0,
        recorded_at: 1500,
        summary: Some(serde_json::json!({"provider_ids":[],"paid_provider_ids":[]})),
        event_present: true,
    }
}
#[test]
fn headers_require_original_attempt_event_bounded_identity_sequence_and_time() {
    assert!(row().decode_header(&"2".repeat(32), 1500).is_ok());
    let changes: [fn(&mut Row); 9] = [
        |r| r.event_present = false,
        |r| r.id = "bad".into(),
        |r| r.attempt_id = "3".repeat(32),
        |r| r.effect_key = "bad effect".into(),
        |r| r.revision = 0,
        |r| r.through_observation = 1001,
        |r| r.recorded_at = 1499,
        |r| r.summary = None,
        |r| r.summary = Some(serde_json::json!([])),
    ];
    for change in changes {
        let mut r = row();
        change(&mut r);
        assert_eq!(
            r.decode_header(&"2".repeat(32), 1500).err(),
            Some(PaymentStoreError::InvalidStoredData)
        );
    }
}
