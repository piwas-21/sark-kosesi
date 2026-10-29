use super::*;
fn row() -> StoredEvent {
    StoredEvent {
        payload: r#"{"kind":"family","code":"FAMILY-A"}"#.into(),
        bom_revision: None,
        actor: "cataloguer".into(),
        source_ref: "fixture".into(),
        reason: "fixture".into(),
    }
}
#[test]
fn event_records_are_closed_bounded_and_cannot_claim_an_unbound_revision() {
    assert_eq!(decode(row()).expect("valid event").bom_revision, None);
    for case in 0..10 {
        let mut r = row();
        match case {
            0 => r.payload = "x".repeat(131073),
            1 => r.actor = "/".into(),
            2 => r.source_ref = " ".into(),
            3 => r.reason = " ".into(),
            4 => r.payload = "{}".into(),
            5 => r.payload = r#"{"kind":"family","code":"DEMO-FAMILY"}"#.into(),
            6 => r.bom_revision = Some(1),
            7 => r.payload = r#"{"kind":"family","code":"FAMILY-A","published":true}"#.into(),
            8 => r.source_ref = "null\0source".into(),
            9 => r.reason = "x".repeat(501),
            _ => unreachable!(),
        }
        assert!(
            matches!(decode(r), Err(DefinitionError::InvalidStoredData)),
            "event boundary {case}"
        );
    }
}
