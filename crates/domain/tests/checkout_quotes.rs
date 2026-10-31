use commerce_domain::{
    Money,
    checkout_quotes::*,
    quotes::{PricedItem, calculate},
};
fn binding() -> QuoteBinding {
    QuoteBinding {
        quote_id: "11111111111111111111111111111111".into(),
        basket_id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
        basket_revision: 1,
        locale: "nl".into(),
        created_at: 1100,
        expires_at: 1400,
        country: "NL".into(),
        postcode: "1234AB".into(),
        policy_id: "NL-POSTCODE".into(),
        policy_revision: 2,
        policy_review_revision: 3,
    }
}
fn snapshot() -> QuoteSnapshot {
    let items = [PricedItem {
        sku: "SET-COVERS".into(),
        publication_revision: 7,
        quantity: 3,
        unit_gross: Money::eur(100).expect("fixture"),
        vat_basis_points: 2100,
    }];
    let totals =
        calculate(&items, Money::eur(900).expect("fixture"), 2100).expect("trusted source fixture");
    QuoteSnapshot::capture(binding(), &totals, &[("SET-COVERS".into(), 4)])
        .expect("captured source")
}
#[test]
fn captures_independent_literal_cent_allocations_and_exact_source_versions() {
    let value = snapshot();
    assert_eq!(value.currency, "EUR");
    assert_eq!(value.schema_version, 1);
    assert_eq!(value.lines[0].bom_revision, 4);
    assert_eq!(value.lines[0].publication_revision, 7);
    assert_eq!(
        value.lines[0].amounts,
        CapturedAmount {
            gross_minor: 300,
            net_minor: 248,
            vat_minor: 52
        }
    );
    assert_eq!(
        value.shipping,
        CapturedAmount {
            gross_minor: 900,
            net_minor: 744,
            vat_minor: 156
        }
    );
    assert_eq!(
        value.total,
        CapturedAmount {
            gross_minor: 1200,
            net_minor: 992,
            vat_minor: 208
        }
    );
    assert_eq!(
        value
            .totals()
            .expect("reconstructed")
            .total
            .gross
            .minor_units(),
        1200
    );
    let round: QuoteSnapshot =
        serde_json::from_str(&serde_json::to_string(&value).expect("stored")).expect("typed read");
    assert!(round == value);
    assert!(round.check_at(1100).is_ok());
    assert!(round.check_at(1399).is_ok());
    assert_eq!(round.check_at(1400), Err(SnapshotError::Expired));
    assert_eq!(round.check_at(1099), Err(SnapshotError::InvalidClock));
}
#[test]
fn corrupt_money_versions_identity_and_time_never_supply_a_quote() {
    let changes: [fn(&mut QuoteSnapshot); 31] = [
        |s| s.schema_version = 2,
        |s| s.currency = "USD".into(),
        |s| s.binding.quote_id = "bad".into(),
        |s| s.binding.quote_id = "g".repeat(32),
        |s| s.binding.basket_id = "bad".into(),
        |s| s.binding.basket_revision = 0,
        |s| s.binding.locale = "ku".into(),
        |s| s.binding.created_at = -1,
        |s| s.binding.expires_at = 1100,
        |s| s.binding.expires_at = 1401,
        |s| {
            s.binding.created_at = i64::MAX - 1;
            s.binding.expires_at = i64::MAX;
        },
        |s| s.binding.policy_id = "DEMO-POLICY".into(),
        |s| s.binding.policy_revision = 0,
        |s| s.binding.policy_review_revision = 0,
        |s| s.binding.country = "GB".into(),
        |s| s.binding.postcode = " lower ".into(),
        |s| s.lines[0].bom_revision = 0,
        |s| s.lines.clear(),
        |s| s.lines[0].quantity = 0,
        |s| s.lines[0].publication_revision = 0,
        |s| s.lines[0].unit_gross_minor = -1,
        |s| s.lines[0].unit_gross_minor = i64::MAX,
        |s| s.lines[0].vat_basis_points = 10001,
        |s| s.shipping.gross_minor = -1,
        |s| s.shipping_vat_basis_points = 10001,
        |s| s.lines[0].amounts.gross_minor = 299,
        |s| s.lines[0].amounts.net_minor = 249,
        |s| s.lines[0].amounts.vat_minor = 51,
        |s| s.shipping.net_minor = 743,
        |s| s.total.gross_minor = 1199,
        |s| s.total.vat_minor = 207,
    ];
    for change in changes {
        let mut value = snapshot();
        change(&mut value);
        assert!(matches!(
            value.totals(),
            Err(SnapshotError::InvalidSnapshot)
        ));
        assert!(matches!(
            value.check_at(1100),
            Err(SnapshotError::InvalidSnapshot)
        ));
    }
    for field in ["actor", "stock", "provider_id", "ready", "discount", "role"] {
        let mut raw = serde_json::to_value(snapshot()).expect("fixture JSON");
        raw[field] = serde_json::json!(true);
        assert!(serde_json::from_value::<QuoteSnapshot>(raw).is_err());
    }
}
#[test]
fn capture_rejects_missing_duplicate_wrong_bom_sources_and_noncanonical_price_lines() {
    let items = [
        PricedItem {
            sku: "A".into(),
            publication_revision: 1,
            quantity: 1,
            unit_gross: Money::eur(1).expect("money"),
            vat_basis_points: 0,
        },
        PricedItem {
            sku: "B".into(),
            publication_revision: 2,
            quantity: 1,
            unit_gross: Money::eur(2).expect("money"),
            vat_basis_points: 0,
        },
    ];
    let mut totals = calculate(&items, Money::eur(0).expect("money"), 0).expect("totals");
    for boms in [
        vec![],
        vec![("A".into(), 1)],
        vec![("A".into(), 1), ("A".into(), 2)],
        vec![("A".into(), 1), ("B".into(), 0)],
    ] {
        assert!(matches!(
            QuoteSnapshot::capture(binding(), &totals, &boms),
            Err(SnapshotError::InvalidSnapshot)
        ));
    }
    let boms = vec![("A".into(), 1), ("B".into(), 2)];
    let good = QuoteSnapshot::capture(binding(), &totals, &boms).expect("source selection");
    assert_eq!(good.total.gross_minor, 3);
    totals.lines.reverse();
    assert!(matches!(
        QuoteSnapshot::capture(binding(), &totals, &boms),
        Err(SnapshotError::InvalidSnapshot)
    ));
}
