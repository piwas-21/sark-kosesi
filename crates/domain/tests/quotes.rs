use commerce_domain::{Money, quotes::*};
fn eur(n: i64) -> Money {
    Money::eur(n).expect("nonnegative fixture")
}
fn item(sku: &str, gross: i64, quantity: u32, rate: u16) -> PricedItem {
    PricedItem {
        sku: sku.into(),
        publication_revision: 1,
        quantity,
        unit_gross: eur(gross),
        vat_basis_points: rate,
    }
}
fn destination() -> Destination {
    Destination {
        country: "NL".into(),
        postcode: "1234 AB".into(),
    }
}
fn policy() -> MarketPolicy {
    MarketPolicy {
        id: "SYNTHETIC-NL".into(),
        revision: 1,
        destination: destination(),
        valid_from: 1000,
        expires_at: 2000,
        products: vec![ProductRule {
            sku: "SET".into(),
            publication_revision: 1,
            max_quantity: 100,
            vat_basis_points: 2100,
        }],
        shipping: ShippingRule {
            gross: eur(1210),
            vat_basis_points: 2100,
            max_parcels: 10,
            max_total_weight_g: 30000,
            max_edge_mm: 1000,
        },
        evidence_ref: "synthetic fixture; not market readiness".into(),
    }
}
#[test]
fn exact_extended_line_vat_and_shipping_have_independent_oracles() {
    let q = calculate(&[item("SET", 12345, 3, 2100)], eur(1210), 2100).expect("quote");
    let line = &q.lines[0];
    assert_eq!(
        (
            line.amounts.gross.minor_units(),
            line.amounts.net.minor_units(),
            line.amounts.vat.minor_units()
        ),
        (37035, 30607, 6428)
    );
    assert_eq!(
        (
            q.shipping.gross.minor_units(),
            q.shipping.net.minor_units(),
            q.shipping.vat.minor_units()
        ),
        (1210, 1000, 210)
    );
    assert_eq!(
        (
            q.total.gross.minor_units(),
            q.total.net.minor_units(),
            q.total.vat.minor_units()
        ),
        (38245, 31607, 6638)
    );
    assert_eq!(
        (
            line.unit_gross.minor_units(),
            line.quantity,
            line.publication_revision,
            line.vat_basis_points
        ),
        (12345, 3, 1, 2100)
    );
    assert_eq!(q.shipping_vat_basis_points, 2100);
}
#[test]
fn half_cent_mixed_rates_zero_and_per_extended_line_rounding_are_explicit() {
    for (gross, qty, rate, net, vat) in [
        (1, 1, 10000, 0, 1),
        (1, 3, 10000, 1, 2),
        (3, 1, 2100, 2, 1),
        (2, 1, 2100, 2, 0),
        (100, 1, 0, 100, 0),
        (0, 100, 2100, 0, 0),
        (109, 1, 900, 100, 9),
    ] {
        let q = calculate(&[item("SET", gross, qty, rate)], eur(0), 0).expect("quote");
        assert_eq!(
            (q.total.net.minor_units(), q.total.vat.minor_units()),
            (net, vat)
        );
    }
    let lines = [
        item("B", 109, 1, 900),
        item("A", 121, 1, 2100),
        item("C", 10, 1, 0),
    ];
    let q = calculate(&lines, eur(121), 2100).expect("mixed");
    assert_eq!(
        (
            q.total.gross.minor_units(),
            q.total.net.minor_units(),
            q.total.vat.minor_units()
        ),
        (361, 310, 51)
    );
    assert_eq!(
        q.lines.iter().map(|l| l.sku.as_str()).collect::<Vec<_>>(),
        vec!["A", "B", "C"]
    );
    assert_eq!(
        q,
        calculate(
            &[lines[2].clone(), lines[1].clone(), lines[0].clone()],
            eur(121),
            2100
        )
        .expect("order independent")
    );
}
#[test]
fn extreme_amounts_never_wrap_or_escape_exact_integer_range() {
    for (gross, qty, shipping) in [
        (i64::MAX, 2, 0),
        (MAX_MINOR_UNITS + 1, 1, 0),
        (MAX_MINOR_UNITS, 2, 0),
        (MAX_MINOR_UNITS, 1, 1),
        (1, 1, MAX_MINOR_UNITS + 1),
    ] {
        assert_eq!(
            calculate(&[item("SET", gross, qty, 2100)], eur(shipping), 0),
            Err(QuoteError::AmountTooLarge)
        );
    }
    let q = calculate(&[item("SET", MAX_MINOR_UNITS, 1, 10000)], eur(0), 0).expect("exact bound");
    assert_eq!(
        (q.total.net.minor_units(), q.total.vat.minor_units()),
        (4_503_599_627_370_495, 4_503_599_627_370_496)
    );
}
#[test]
fn invalid_lines_and_tax_rates_fail_closed() {
    let valid = item("SET", 10, 1, 2100);
    for lines in [
        vec![],
        vec![valid.clone(); 21],
        vec![valid.clone(), valid.clone()],
        vec![item("DEMO-SET", 10, 1, 2100)],
        vec![item("bad", 10, 1, 2100)],
        vec![item("SET", 10, 0, 2100)],
        vec![item("SET", 10, 101, 2100)],
        vec![PricedItem {
            publication_revision: 0,
            ..valid.clone()
        }],
    ] {
        assert_eq!(
            calculate(&lines, eur(0), 0),
            Err(QuoteError::InvalidRequest)
        );
    }
    assert_eq!(
        calculate(&[item("SET", 10, 1, 10001)], eur(0), 0),
        Err(QuoteError::InvalidPolicy)
    );
    assert_eq!(
        calculate(&[valid], eur(0), 10001),
        Err(QuoteError::InvalidPolicy)
    );
}
#[test]
fn request_has_explicit_country_postcode_sku_and_quantity_bounds() {
    let valid = QuoteRequest {
        destination: destination(),
        lines: vec![BasketLine {
            sku: "SET".into(),
            quantity: 1,
        }],
    };
    assert_eq!(valid.validate(), Ok(()));
    for code in [
        "AT", "BE", "BG", "HR", "CY", "CZ", "DK", "EE", "FI", "FR", "DE", "GR", "HU", "IE", "IT",
        "LV", "LT", "LU", "MT", "NL", "PL", "PT", "RO", "SK", "SI", "ES", "SE",
    ] {
        let mut r = valid.clone();
        r.destination.country = code.into();
        assert_eq!(r.validate(), Ok(()));
    }
    for country in ["", "nl", "UK", "US", "NLD"] {
        let mut r = valid.clone();
        r.destination.country = country.into();
        assert_eq!(r.validate(), Err(QuoteError::InvalidRequest));
    }
    for postcode in [
        "",
        " 1234 AB",
        "1234 AB ",
        "1234 ab",
        "1234\nAB",
        "--",
        "é",
        "12345678901234567",
    ] {
        let mut r = valid.clone();
        r.destination.postcode = postcode.into();
        assert_eq!(r.validate(), Err(QuoteError::InvalidRequest));
    }
    for lines in [
        vec![],
        vec![valid.lines[0].clone(); 21],
        vec![valid.lines[0].clone(); 2],
        vec![BasketLine {
            sku: "DEMO-SET".into(),
            quantity: 1,
        }],
        vec![BasketLine {
            sku: "SET".into(),
            quantity: 0,
        }],
        vec![BasketLine {
            sku: "SET".into(),
            quantity: 101,
        }],
    ] {
        let r = QuoteRequest {
            lines,
            ..valid.clone()
        };
        assert_eq!(r.validate(), Err(QuoteError::InvalidRequest));
    }
    let r = QuoteRequest {
        destination: Destination {
            country: "IE".into(),
            postcode: "A1B-2CD".into(),
        },
        ..valid
    };
    assert_eq!(r.validate(), Ok(()));
}
#[test]
fn exact_policy_time_destination_and_source_revisions_are_required() {
    let p = policy();
    assert_eq!(p.validate(), Ok(()));
    for now in [1000, 1999] {
        assert_eq!(p.permits(&destination(), now), Ok(()));
    }
    for now in [-1, 999, 2000, i64::MAX] {
        assert_eq!(
            p.permits(&destination(), now),
            Err(QuoteError::MarketUnavailable)
        );
    }
    for d in [
        Destination {
            country: "DE".into(),
            ..destination()
        },
        Destination {
            postcode: "1234 AC".into(),
            ..destination()
        },
    ] {
        assert_eq!(p.permits(&d, 1100), Err(QuoteError::MarketUnavailable));
    }
    assert!(p.product("SET", 1, 100).is_ok());
    for (sku, revision, quantity) in [
        ("OTHER", 1, 1),
        ("SET", 2, 1),
        ("SET", 1, 101),
        ("SET", 1, 0),
    ] {
        assert!(matches!(
            p.product(sku, revision, quantity),
            Err(QuoteError::ProductUnavailable)
        ));
    }
}
#[test]
fn malformed_policy_configuration_never_grants_readiness() {
    type Mutation = Box<dyn Fn(&mut MarketPolicy)>;
    let mutations: Vec<Mutation> = vec![
        Box::new(|p| p.id = "".into()),
        Box::new(|p| p.revision = 0),
        Box::new(|p| p.destination.country = "GB".into()),
        Box::new(|p| p.destination.postcode = "*".into()),
        Box::new(|p| p.valid_from = -1),
        Box::new(|p| p.expires_at = p.valid_from),
        Box::new(|p| p.evidence_ref = " ".into()),
        Box::new(|p| p.evidence_ref = "x".repeat(501)),
        Box::new(|p| p.evidence_ref = "bad\0ref".into()),
        Box::new(|p| p.products.clear()),
        Box::new(|p| p.products = vec![p.products[0].clone(); 21]),
        Box::new(|p| p.products.push(p.products[0].clone())),
        Box::new(|p| p.products[0].sku = "DEMO-SET".into()),
        Box::new(|p| p.products[0].publication_revision = 0),
        Box::new(|p| p.products[0].max_quantity = 0),
        Box::new(|p| p.products[0].max_quantity = 101),
        Box::new(|p| p.products[0].vat_basis_points = 10001),
        Box::new(|p| p.shipping.gross = eur(MAX_MINOR_UNITS + 1)),
        Box::new(|p| p.shipping.vat_basis_points = 10001),
        Box::new(|p| p.shipping.max_parcels = 0),
        Box::new(|p| p.shipping.max_parcels = 1001),
        Box::new(|p| p.shipping.max_total_weight_g = 0),
        Box::new(|p| p.shipping.max_edge_mm = 0),
    ];
    for mutate in mutations {
        let mut p = policy();
        mutate(&mut p);
        assert_eq!(p.validate(), Err(QuoteError::InvalidPolicy));
        assert_eq!(
            p.permits(&destination(), 1100),
            Err(QuoteError::InvalidPolicy)
        );
    }
}
