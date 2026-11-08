use commerce_domain::{
    Currency, Money,
    checkout_quotes::{
        CapturedAmount, PromotedQuoteSnapshot, QuoteBinding, QuoteSnapshot, SnapshotError,
    },
    promotion_policies::{PromotionPolicy, RedemptionLimits},
    promotions::{DiscountKind, EligibleProduct, Promotion},
    quotes::{PricedItem, calculate},
};
fn original() -> QuoteSnapshot {
    let items = [
        PricedItem {
            sku: "A".into(),
            publication_revision: 1,
            quantity: 1,
            unit_gross: Money::eur(121).expect("EUR"),
            vat_basis_points: 2100,
        },
        PricedItem {
            sku: "B".into(),
            publication_revision: 1,
            quantity: 1,
            unit_gross: Money::eur(109).expect("EUR"),
            vat_basis_points: 900,
        },
    ];
    let totals = calculate(&items, Money::eur(1210).expect("EUR"), 2100).expect("quote");
    QuoteSnapshot::capture(
        QuoteBinding {
            quote_id: "1".repeat(32),
            basket_id: "a".repeat(32),
            basket_revision: 2,
            locale: "nl".into(),
            created_at: 1100,
            expires_at: 1400,
            country: "NL".into(),
            postcode: "1234AB".into(),
            policy_id: "NL-POSTCODE".into(),
            policy_revision: 2,
            policy_review_revision: 3,
        },
        &totals,
        &[("A".into(), 4), ("B".into(), 5)],
    )
    .expect("owned quote")
}
fn policy() -> PromotionPolicy {
    PromotionPolicy {
        currency: Currency::EUR,
        promotion: Promotion {
            id: "SYNTHETIC-OFFER".into(),
            revision: 7,
            valid_from: 1000,
            expires_at: 1400,
            countries: vec!["NL".into()],
            products: vec![
                EligibleProduct {
                    sku: "A".into(),
                    publication_revision: 1,
                },
                EligibleProduct {
                    sku: "B".into(),
                    publication_revision: 1,
                },
            ],
            minimum_eligible_gross_minor: 0,
            maximum_discount_minor: 100,
            discount: DiscountKind::Fixed { minor_units: 23 },
            evidence_ref: "synthetic".into(),
        },
        limits: RedemptionLimits {
            total_discount_minor: 1000,
            total_redemptions: 10,
            per_customer_redemptions: None,
        },
    }
}
fn snapshot() -> PromotedQuoteSnapshot {
    PromotedQuoteSnapshot::capture(original(), &policy(), 4).expect("captured promotion")
}
#[test]
fn exact_mixed_vat_allocations_and_owned_source_survive_closed_roundtrip() {
    let result = snapshot();
    assert_eq!(
        (
            result.schema_version,
            result.allocation_version,
            result.discount_minor
        ),
        (2, 1, 23)
    );
    assert_eq!(
        (
            result.promotion.id.as_str(),
            result.promotion.revision,
            result.promotion.review_revision
        ),
        ("SYNTHETIC-OFFER", 7, 4)
    );
    assert_eq!(result.original.binding.quote_id, "1".repeat(32));
    assert_eq!(result.original.lines[0].bom_revision, 4);
    assert_eq!(
        result
            .lines
            .iter()
            .map(|line| (
                line.sku.as_str(),
                line.discount_minor,
                line.amounts.gross_minor,
                line.amounts.net_minor,
                line.amounts.vat_minor
            ))
            .collect::<Vec<_>>(),
        vec![("A", 12, 109, 90, 19), ("B", 11, 98, 90, 8)]
    );
    assert_eq!(
        result.shipping,
        CapturedAmount {
            gross_minor: 1210,
            net_minor: 1000,
            vat_minor: 210
        }
    );
    assert_eq!(
        result.total,
        CapturedAmount {
            gross_minor: 1417,
            net_minor: 1180,
            vat_minor: 237
        }
    );
    let json = serde_json::to_value(&result).expect("JSON");
    assert!(serde_json::from_value::<QuoteSnapshot>(json.clone()).is_err());
    let round: PromotedQuoteSnapshot = serde_json::from_value(json).expect("closed snapshot");
    assert!(round == result);
    assert!(round.validate(&policy(), 4).is_ok());
    assert!(round.check_at(&policy(), 4, 1399).is_ok());
    assert!(matches!(
        round.check_at(&policy(), 4, 1400),
        Err(SnapshotError::Expired)
    ));
    assert!(matches!(
        round.check_at(&policy(), 4, 1099),
        Err(SnapshotError::InvalidClock)
    ));
    // Original v1 remains independently readable; it is not a promoted order permit.
    let v1: QuoteSnapshot =
        serde_json::from_value(serde_json::to_value(original()).expect("v1 JSON")).expect("v1");
    assert!(v1.totals().is_ok());
}
#[test]
fn changed_captured_fields_and_trusted_source_are_rejected() {
    let edits: [fn(&mut PromotedQuoteSnapshot); 14] = [
        |s| s.schema_version = 1,
        |s| s.allocation_version = 2,
        |s| s.promotion.id = "OTHER".into(),
        |s| s.promotion.revision += 1,
        |s| s.promotion.review_revision += 1,
        |s| s.discount_minor += 1,
        |s| s.lines[0].sku = "B".into(),
        |s| s.lines[0].discount_minor += 1,
        |s| s.lines[0].amounts.gross_minor += 1,
        |s| s.lines[0].amounts.net_minor += 1,
        |s| s.lines[0].amounts.vat_minor += 1,
        |s| s.lines.reverse(),
        |s| s.shipping.net_minor += 1,
        |s| s.total.vat_minor += 1,
    ];
    for edit in edits {
        let mut value = snapshot();
        edit(&mut value);
        assert!(matches!(
            value.validate(&policy(), 4),
            Err(SnapshotError::InvalidSnapshot)
        ));
        assert!(matches!(
            value.validate_verified(&policy(), 4),
            Err(SnapshotError::InvalidSnapshot)
        ));
    }
    let value = snapshot();
    assert!(matches!(
        value.validate(&policy(), 5),
        Err(SnapshotError::InvalidSnapshot)
    ));
    let mut changed = policy();
    changed.promotion.discount = DiscountKind::Fixed { minor_units: 24 };
    assert!(matches!(
        value.validate(&changed, 4),
        Err(SnapshotError::InvalidSnapshot)
    ));
    let mut invalid_original = original();
    invalid_original.total.net_minor += 1;
    assert!(matches!(
        PromotedQuoteSnapshot::capture(invalid_original, &policy(), 4),
        Err(SnapshotError::InvalidSnapshot)
    ));
    for path in ["top", "reference", "line"] {
        let mut value = serde_json::to_value(snapshot()).expect("JSON");
        match path {
            "top" => value["authority"] = true.into(),
            "reference" => value["promotion"]["approved"] = true.into(),
            _ => value["lines"][0]["extra"] = true.into(),
        }
        assert!(serde_json::from_value::<PromotedQuoteSnapshot>(value).is_err());
    }
}
#[test]
fn guest_policy_and_validity_windows_fail_closed() {
    let value = snapshot();
    assert!(value.validate(&policy(), 4).is_ok());
    assert!(matches!(
        PromotedQuoteSnapshot::capture(original(), &policy(), 0),
        Err(SnapshotError::InvalidSnapshot)
    ));
    let mut invalid = policy();
    invalid.limits.total_redemptions = 0;
    assert!(matches!(
        PromotedQuoteSnapshot::capture(original(), &invalid, 4),
        Err(SnapshotError::InvalidSnapshot)
    ));
    let mut changed = policy();
    changed.limits.per_customer_redemptions = Some(1);
    assert!(matches!(
        PromotedQuoteSnapshot::capture(original(), &changed, 4),
        Err(SnapshotError::InvalidSnapshot)
    ));
    let mut changed = policy();
    changed.promotion.expires_at = 1399;
    assert!(matches!(
        PromotedQuoteSnapshot::capture(original(), &changed, 4),
        Err(SnapshotError::InvalidSnapshot)
    ));
    let mut changed = policy();
    changed.promotion.valid_from = 1101;
    assert!(matches!(
        PromotedQuoteSnapshot::capture(original(), &changed, 4),
        Err(SnapshotError::InvalidSnapshot)
    ));
    let mut changed = policy();
    changed.promotion.countries = vec!["DE".into()];
    assert!(matches!(
        PromotedQuoteSnapshot::capture(original(), &changed, 4),
        Err(SnapshotError::InvalidSnapshot)
    ));
}
