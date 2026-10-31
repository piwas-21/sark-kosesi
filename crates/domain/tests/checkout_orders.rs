//! Invented private snapshots; no actual seller/tax/legal/postal or delivery evidence.
use commerce_domain::{checkout_orders::*, checkout_quotes::*, specifications::DimensionsMm};
fn address() -> Address {
    Address {
        recipient: "Synthetic Recipient".into(),
        line1: "Synthetic Street 22".into(),
        line2: None,
        city: "Fixture City".into(),
        region: None,
        country: "NL".into(),
        postcode: "1234 AB".into(),
    }
}
fn seller() -> SellerPolicy {
    SellerPolicy {
        id: "SYNTHETIC-SELLER".into(),
        revision: 1,
        locale: "nl".into(),
        legal_name: "Piwas synthetic test seller".into(),
        registration_id: "synthetic-registration-not-real".into(),
        tax_registration: SellerTaxRegistration::VatRegistered {
            vat_id: "synthetic-tax-id-not-real".into(),
        },
        address: address(),
        support_email: "support@example.test".into(),
        terms_version: "synthetic-terms-v1".into(),
        terms_sha256: "a".repeat(64),
        privacy_version: "synthetic-privacy-v1".into(),
        privacy_sha256: "b".repeat(64),
        valid_from: 1000,
        expires_at: 2000,
        evidence_ref: "synthetic seller/legal/tax evidence only".into(),
    }
}
fn snapshot() -> OrderSnapshot {
    let quote:QuoteSnapshot=serde_json::from_value(serde_json::json!({"schema_version":1,"currency":"EUR","binding":{"quote_id":"11111111111111111111111111111111","basket_id":"22222222222222222222222222222222","basket_revision":1,"locale":"nl","country":"NL","postcode":"1234 AB","policy_id":"NL-POLICY","policy_revision":1,"policy_review_revision":1,"created_at":1100,"expires_at":1400},"lines":[{"sku":"SET-FILLED","publication_revision":1,"bom_revision":1,"quantity":3,"unit_gross_minor":100,"vat_basis_points":2100,"amounts":{"gross_minor":300,"net_minor":248,"vat_minor":52}}],"shipping":{"gross_minor":900,"net_minor":744,"vat_minor":156},"shipping_vat_basis_points":2100,"total":{"gross_minor":1200,"net_minor":992,"vat_minor":208}})).expect("independent literal prices");
    OrderSnapshot {
        schema_version: 1,
        identity: OrderIdentity {
            id: "3".repeat(32),
            basket_id: "2".repeat(32),
            quote_id: "1".repeat(32),
            stock_group_id: "4".repeat(32),
            created_at: 1200,
            hold_expires_at: 2100,
        },
        quote,
        buyer: BuyerContact {
            name: "Synthetic Buyer".into(),
            email: "buyer@example.test".into(),
        },
        shipping_address: address(),
        billing_address: address(),
        seller: seller(),
        market: commerce_domain::quotes::MarketPolicy {
            id: "NL-POLICY".into(),
            revision: 1,
            destination: commerce_domain::quotes::Destination {
                country: "NL".into(),
                postcode: "1234 AB".into(),
            },
            valid_from: 1000,
            expires_at: 2000,
            products: vec![commerce_domain::quotes::ProductRule {
                sku: "SET-FILLED".into(),
                publication_revision: 1,
                max_quantity: 10,
                vat_basis_points: 2100,
            }],
            shipping: commerce_domain::quotes::ShippingRule {
                gross: commerce_domain::Money::eur(900).expect("fixture money"),
                vat_basis_points: 2100,
                max_parcels: 10,
                max_total_weight_g: 30000,
                max_edge_mm: 505,
            },
            evidence_ref: "synthetic market policy".into(),
        },
        products: vec![ProductSnapshot {
            sku: "SET-FILLED".into(),
            publication_revision: 1,
            bom_revision: 1,
            title: "Synthetic test set".into(),
            family_code: "FAMILY-A".into(),
            size_code: "SIZE-A".into(),
            textile_code: "TEXTILE-A".into(),
            fill: "with-foam".into(),
            footprint: DimensionsMm {
                width_mm: 1000,
                depth_mm: 500,
                height_mm: 500,
                tolerance_mm: 5,
            },
        }],
    }
}
#[test]
fn accepted_history_retains_original_money_and_legal_sources_after_quote_deadline() {
    let s = snapshot();
    assert_eq!(s.validate(), Ok(()));
    assert_eq!(
        s.quote.total,
        CapturedAmount {
            gross_minor: 1200,
            net_minor: 992,
            vat_minor: 208
        }
    );
    assert_eq!(s.quote.check_at(1400), Err(SnapshotError::Expired));
    assert_eq!(
        s.seller.permits("nl", 2000),
        Err(OrderError::PolicyUnavailable)
    );
    assert_eq!(s.validate(), Ok(()));
    let round: OrderSnapshot =
        serde_json::from_str(&serde_json::to_string(&s).expect("snapshot")).expect("closed read");
    assert!(round == s);
    assert_eq!(round.products[0].title, "Synthetic test set");
    assert_eq!(round.products[0].bom_revision, 1);
}
#[test]
fn contacts_validate_bounded_single_mailboxes_without_equating_syntax_with_identity() {
    for email in [
        "buyer@example.test",
        "buyer+tag@example.test",
        "مستخدم@example.test",
        "şark@example.test",
    ] {
        assert!(
            BuyerContact {
                name: "اسم تجريبي".into(),
                email: email.into()
            }
            .validate()
            .is_ok(),
            "{email}"
        );
    }
    for email in [
        "",
        "missing-at",
        "a@localhost",
        "Name <buyer@example.test>",
        "a@[127.0.0.1]",
        "a@example.test\r\nBcc: victim@example.test",
        " a@example.test",
        "a@example.test ",
    ] {
        assert_eq!(
            BuyerContact {
                name: "Synthetic".into(),
                email: email.into()
            }
            .validate(),
            Err(OrderError::InvalidRequest)
        );
    }
    assert_eq!(
        BuyerContact {
            name: "Synthetic".into(),
            email: format!("{}@example.test", "x".repeat(255))
        }
        .validate(),
        Err(OrderError::InvalidRequest)
    );
    for name in ["", " leading", "trailing ", "bad\nname"] {
        assert_eq!(
            BuyerContact {
                name: name.into(),
                email: "a@example.test".into()
            }
            .validate(),
            Err(OrderError::InvalidRequest)
        );
    }
    let mut a = address();
    a.recipient = "اسم تجريبي".into();
    a.line1 = "عنوان تجريبي ٢٢".into();
    a.line2 = Some("Appartement 2".into());
    a.region = Some("Fixture Province".into());
    assert!(a.validate().is_ok());
    for n in 0..8 {
        let mut a = address();
        match n {
            0 => a.recipient = "".into(),
            1 => a.line1 = "x".repeat(201),
            2 => a.line2 = Some("".into()),
            3 => a.city = "\n".into(),
            4 => a.region = Some("".into()),
            5 => a.country = "NLD".into(),
            6 => a.postcode = "x".repeat(17),
            _ => a.country = "ZZ".into(),
        };
        assert_eq!(a.validate(), Err(OrderError::InvalidRequest));
    }
}
#[test]
fn malformed_sources_money_ownership_and_acceptance_time_cannot_form_an_order() {
    let changes: [fn(&mut OrderSnapshot); 28] = [
        |s| s.schema_version = 2,
        |s| s.identity.id = "g".repeat(32),
        |s| s.identity.stock_group_id = "bad".into(),
        |s| s.identity.basket_id = "5".repeat(32),
        |s| s.identity.quote_id = "6".repeat(32),
        |s| s.products.clear(),
        |s| s.shipping_address.postcode = "9999 ZZ".into(),
        |s| s.shipping_address.country = "DE".into(),
        |s| s.identity.created_at = 1400,
        |s| s.identity.created_at = 1099,
        |s| s.identity.hold_expires_at = 1200,
        |s| s.identity.hold_expires_at = 3001,
        |s| s.buyer.email = "invalid".into(),
        |s| s.billing_address.line1 = "".into(),
        |s| s.seller.locale = "de".into(),
        |s| s.seller.expires_at = 1200,
        |s| s.quote.total.net_minor = 991,
        |s| s.products[0].sku = "OTHER".into(),
        |s| s.products[0].publication_revision = 2,
        |s| s.products[0].bom_revision = 2,
        |s| s.products[0].title = "".into(),
        |s| s.products[0].family_code = "DEMO".into(),
        |s| s.products[0].size_code = "x".repeat(81),
        |s| s.products[0].textile_code = "".into(),
        |s| s.products[0].fill = "prepacked".into(),
        |s| s.products[0].footprint.width_mm = 0,
        |s| s.products[0].footprint.tolerance_mm = 501,
        |s| s.shipping_address.recipient = "".into(),
    ];
    for change in changes {
        let mut s = snapshot();
        change(&mut s);
        assert_eq!(s.validate(), Err(OrderError::InvalidSnapshot));
    }
    let mut s = snapshot();
    s.quote.lines[0].unit_gross_minor = 0;
    s.quote.lines[0].amounts = CapturedAmount {
        gross_minor: 0,
        net_minor: 0,
        vat_minor: 0,
    };
    s.quote.shipping = CapturedAmount {
        gross_minor: 0,
        net_minor: 0,
        vat_minor: 0,
    };
    s.quote.total = CapturedAmount {
        gross_minor: 0,
        net_minor: 0,
        vat_minor: 0,
    };
    s.market.shipping.gross = commerce_domain::Money::eur(0).expect("fixture money");
    assert!(s.quote.totals().is_ok());
    assert_eq!(s.validate(), Err(OrderError::InvalidSnapshot));
}
#[test]
fn seller_policy_needs_explicit_identity_documents_locale_validity_and_tax_choice() {
    let p = seller();
    assert!(p.validate().is_ok());
    assert_eq!(p.permits("de", 1100), Err(OrderError::PolicyUnavailable));
    assert_eq!(p.permits("nl", 999), Err(OrderError::PolicyUnavailable));
    let mut p = seller();
    p.tax_registration = SellerTaxRegistration::NotVatRegistered {
        evidence_ref: "synthetic qualified nonregistration evidence".into(),
    };
    assert!(p.validate().is_ok());
    let changes: [fn(&mut SellerPolicy); 17] = [
        |p| p.id = "DEMO".into(),
        |p| p.revision = 0,
        |p| p.locale = "ku".into(),
        |p| p.legal_name = "".into(),
        |p| p.registration_id = "".into(),
        |p| p.tax_registration = SellerTaxRegistration::VatRegistered { vat_id: "".into() },
        |p| {
            p.tax_registration = SellerTaxRegistration::NotVatRegistered {
                evidence_ref: "".into(),
            }
        },
        |p| p.address.country = "DE".into(),
        |p| p.support_email = "bad".into(),
        |p| p.terms_version = "".into(),
        |p| p.terms_sha256 = "g".repeat(64),
        |p| p.privacy_version = "bad version".into(),
        |p| p.privacy_sha256 = "bad".into(),
        |p| p.valid_from = -1,
        |p| p.expires_at = 1000,
        |p| p.expires_at = 9_007_199_254_740_992,
        |p| p.evidence_ref = "".into(),
    ];
    for change in changes {
        let mut p = seller();
        change(&mut p);
        assert_eq!(p.validate(), Err(OrderError::InvalidRequest));
    }
    let mut raw = serde_json::to_value(seller()).expect("policy");
    raw.as_object_mut()
        .expect("object")
        .remove("tax_registration");
    assert!(serde_json::from_value::<SellerPolicy>(raw).is_err());
    let mut raw = serde_json::to_value(snapshot()).expect("snapshot");
    raw["role"] = "admin".into();
    assert!(serde_json::from_value::<OrderSnapshot>(raw).is_err());
}

#[test]
fn explicit_complete_intent_and_market_proof_are_required() {
    let s = snapshot();
    let intent = OrderIntent {
        quote_id: s.identity.quote_id.clone(),
        expected_basket_revision: 1,
        buyer: s.buyer.clone(),
        shipping_address: s.shipping_address.clone(),
        billing_address: s.billing_address.clone(),
        accepted_terms_version: s.seller.terms_version.clone(),
        confirmed: true,
    };
    assert_eq!(intent.validate(), Ok(()));
    let changes: [fn(&mut OrderIntent); 7] = [
        |v| v.quote_id = "bad".into(),
        |v| v.expected_basket_revision = 0,
        |v| v.confirmed = false,
        |v| v.accepted_terms_version = "".into(),
        |v| v.buyer.email = "bad".into(),
        |v| v.shipping_address.line1 = "".into(),
        |v| v.billing_address.line1 = "".into(),
    ];
    for change in changes {
        let mut v = intent.clone();
        change(&mut v);
        assert!(v.validate().is_err());
    }
    let changes: [fn(&mut OrderSnapshot); 10] = [
        |v| v.market.id = "OTHER".into(),
        |v| v.market.revision = 2,
        |v| v.market.shipping.gross = commerce_domain::Money::eur(901).expect("fixture money"),
        |v| v.market.shipping.vat_basis_points = 2000,
        |v| v.market.destination.postcode = "1234 AC".into(),
        |v| v.market.expires_at = 1200,
        |v| v.market.products.clear(),
        |v| v.market.products[0].publication_revision = 2,
        |v| v.market.products[0].max_quantity = 2,
        |v| v.market.products[0].vat_basis_points = 2000,
    ];
    for change in changes {
        let mut v = snapshot();
        change(&mut v);
        assert_eq!(v.validate(), Err(OrderError::InvalidSnapshot));
    }
}
