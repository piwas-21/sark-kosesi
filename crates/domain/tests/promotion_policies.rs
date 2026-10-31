use commerce_domain::{promotion_policies::*, promotions::*};
fn policy() -> PromotionPolicy {
    PromotionPolicy {
        currency: commerce_domain::Currency::EUR,
        promotion: Promotion {
            id: "COURTYARD".into(),
            revision: 1,
            valid_from: 1000,
            expires_at: 2000,
            countries: vec!["NL".into(), "DE".into()],
            products: vec![
                EligibleProduct {
                    sku: "SET-B".into(),
                    publication_revision: 2,
                },
                EligibleProduct {
                    sku: "SET-A".into(),
                    publication_revision: 1,
                },
            ],
            minimum_eligible_gross_minor: 1000,
            maximum_discount_minor: 500,
            discount: DiscountKind::Fixed { minor_units: 500 },
            evidence_ref: "synthetic configuration only".into(),
        },
        limits: RedemptionLimits {
            total_discount_minor: 10000,
            total_redemptions: 20,
            per_customer_redemptions: Some(1),
        },
    }
}
#[test]
fn reviewed_terms_keep_explicit_lifetime_and_verified_customer_limits() {
    let mut p = policy();
    assert!(p.validate().is_ok());
    p.canonicalize();
    assert_eq!(p.promotion.countries, vec!["DE", "NL"]);
    assert_eq!(
        p.promotion
            .products
            .iter()
            .map(|v| v.sku.as_str())
            .collect::<Vec<_>>(),
        vec!["SET-A", "SET-B"]
    );
    assert_eq!(p.limits.total_discount_minor, 10000);
    assert_eq!(p.limits.per_customer_redemptions, Some(1));
    assert!(p.permits("NL", 1000).is_ok());
    assert!(p.permits("DE", 1999).is_ok());
    for (country, at) in [("NL", 999), ("NL", 2000), ("BE", 1100)] {
        assert_eq!(p.permits(country, at), Err(PromotionError::Ineligible));
    }
    p.limits.per_customer_redemptions = None;
    assert!(p.validate().is_ok());
    p.promotion.revision = 2;
    assert_eq!(p.limits.total_discount_minor, 10000);
    assert_eq!(p.limits.total_redemptions, 20);
}
#[test]
fn malformed_or_unbounded_limits_never_become_a_reviewable_policy() {
    let changes: [fn(&mut PromotionPolicy); 10] = [
        |p| p.promotion.id = "DEMO".into(),
        |p| p.promotion.expires_at = 9_007_199_254_740_992,
        |p| p.limits.total_discount_minor = 0,
        |p| p.limits.total_discount_minor = -1,
        |p| p.limits.total_discount_minor = 9_007_199_254_740_992,
        |p| p.limits.total_redemptions = 0,
        |p| p.limits.total_redemptions = 1_000_001,
        |p| p.limits.per_customer_redemptions = Some(0),
        |p| p.limits.per_customer_redemptions = Some(21),
        |p| p.promotion.discount = DiscountKind::Percentage { basis_points: 0 },
    ];
    for change in changes {
        let mut p = policy();
        change(&mut p);
        assert_eq!(p.validate(), Err(PromotionError::InvalidPolicy));
        assert_eq!(p.permits("NL", 1100), Err(PromotionError::InvalidPolicy));
    }
    let mut p = policy();
    p.limits.total_discount_minor = 9_007_199_254_740_991;
    p.limits.total_redemptions = 1_000_000;
    p.limits.per_customer_redemptions = Some(1_000_000);
    assert!(p.validate().is_ok());
}
#[test]
fn nullable_customer_limit_is_an_explicit_required_choice_in_persisted_terms() {
    let mut p = policy();
    p.limits.per_customer_redemptions = None;
    let mut value = serde_json::to_value(&p).expect("policy");
    assert_eq!(value["currency"], "EUR");
    assert!(value["limits"]["per_customer_redemptions"].is_null());
    assert_eq!(
        serde_json::from_value::<PromotionPolicy>(value.clone()).expect("explicit null"),
        p
    );
    value["limits"]
        .as_object_mut()
        .expect("limits")
        .remove("per_customer_redemptions");
    assert!(serde_json::from_value::<PromotionPolicy>(value).is_err());
    let mut value = serde_json::to_value(&p).expect("policy");
    value["currency"] = "USD".into();
    assert!(serde_json::from_value::<PromotionPolicy>(value).is_err());
}
