use super::*;
#[test]
fn private_projection_maps_kurdish_public_locale_and_denies_unknown_stored_locale() {
    let mut snapshot: commerce_domain::checkout_quotes::QuoteSnapshot=serde_json::from_value(serde_json::json!({
        "schema_version":1,"currency":"EUR",
        "binding":{"quote_id":"11111111111111111111111111111111","basket_id":"22222222222222222222222222222222","basket_revision":1,"locale":"kmr","country":"NL","postcode":"1234 AB","policy_id":"NL-POLICY","policy_revision":1,"policy_review_revision":1,"created_at":1100,"expires_at":1400},
        "lines":[{"sku":"SET-FILLED","publication_revision":1,"bom_revision":1,"quantity":3,"unit_gross_minor":100,"vat_basis_points":2100,"amounts":{"gross_minor":300,"net_minor":248,"vat_minor":52}}],
        "shipping":{"gross_minor":900,"net_minor":744,"vat_minor":156},"shipping_vat_basis_points":2100,
        "total":{"gross_minor":1200,"net_minor":992,"vat_minor":208}
    })).expect("fixture");
    assert!(snapshot.totals().is_ok());
    let view = QuoteView::try_from(snapshot.clone()).expect("private projection");
    assert_eq!(view.locale, "ku");
    assert_eq!(view.total.gross_minor, 1200);
    snapshot.binding.locale = "ku".into();
    assert!(matches!(
        QuoteView::try_from(snapshot),
        Err(QuoteStoreError::InvalidStoredData)
    ));
}
