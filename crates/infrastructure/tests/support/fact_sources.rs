use super::*;
fn row() -> Sources {
    Sources {
        identity_key: "identity".into(),
        bom_key: "bom".into(),
        specification_actor: "author".into(),
        specification_reason: "measured".into(),
        minor_units: 12345,
        currency: "EUR".into(),
        price_actor: "pricer".into(),
        price_evidence_ref: "price evidence".into(),
        price_reason: "reason".into(),
    }
}
#[test]
fn stored_sources_reject_invalid_money_currency_authority_and_evidence() {
    assert_eq!(row().validated_price().expect("valid").minor_units(), 12345);
    for field in 0..9 {
        let mut value = row();
        match field {
            0 => value.specification_actor = "bad/id".into(),
            1 => value.specification_reason = " ".into(),
            2 => value.price_actor = " ".into(),
            3 => value.price_evidence_ref = " ".into(),
            4 => value.price_evidence_ref = "hidden\0ref".into(),
            5 => value.price_reason = " ".into(),
            6 => value.currency = "USD".into(),
            7 => value.minor_units = -1,
            8 => value.price_evidence_ref = "x".repeat(513),
            _ => unreachable!(),
        }
        assert!(
            matches!(value.validated_price(), Err(FactError::InvalidStoredData)),
            "field {field}"
        );
    }
}
