use commerce_domain::{Money, promotions::*, quotes::*};
fn eur(value: i64) -> Money {
    Money::eur(value).expect("nonnegative fixture")
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
fn terms() -> Promotion {
    Promotion {
        id: "SYNTHETIC-OFFER".into(),
        revision: 1,
        valid_from: 1000,
        expires_at: 2000,
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
            EligibleProduct {
                sku: "C".into(),
                publication_revision: 1,
            },
        ],
        minimum_eligible_gross_minor: 0,
        maximum_discount_minor: MAX_MINOR_UNITS,
        discount: DiscountKind::Fixed { minor_units: 100 },
        evidence_ref: "synthetic economics; not merchant/accountant approval".into(),
    }
}
fn original(items: &[PricedItem], shipping: i64, rate: u16) -> QuoteTotals {
    calculate(items, eur(shipping), rate).expect("independent quote fixture")
}
fn price(quote: &QuoteTotals, p: &Promotion) -> Result<PromotionQuote, PromotionError> {
    apply(quote, p, "NL", 1100)
}
#[test]
fn largest_remainder_has_exact_cent_oracles_and_canonical_sku_ties() {
    let q = original(
        &[
            item("C", 101, 1, 0),
            item("A", 101, 1, 0),
            item("B", 101, 1, 0),
        ],
        0,
        0,
    );
    let result = price(&q, &terms()).expect("fixed allocation");
    assert_eq!(result.allocation_version, 1);
    assert_eq!(result.discount.minor_units(), 100);
    assert_eq!(
        result
            .lines
            .iter()
            .map(|l| (
                l.original.sku.as_str(),
                l.discount.minor_units(),
                l.amounts.gross.minor_units()
            ))
            .collect::<Vec<_>>(),
        vec![("A", 34, 67), ("B", 33, 68), ("C", 33, 68)]
    );
    assert_eq!(
        (
            result.total.gross.minor_units(),
            result.total.net.minor_units(),
            result.total.vat.minor_units()
        ),
        (203, 203, 0)
    );
    assert_eq!(result.lines[0].original, q.lines[0]);
    assert_eq!(
        (result.promotion_id.as_str(), result.promotion_revision),
        ("SYNTHETIC-OFFER", 1)
    );
    let weighted = original(
        &[
            item("A", 100, 1, 0),
            item("B", 300, 1, 0),
            item("C", 600, 1, 0),
        ],
        0,
        0,
    );
    let mut p = terms();
    p.discount = DiscountKind::Fixed { minor_units: 101 };
    let result = price(&weighted, &p).expect("weighted");
    assert_eq!(
        result
            .lines
            .iter()
            .map(|l| l.discount.minor_units())
            .collect::<Vec<_>>(),
        vec![10, 30, 61]
    );
}
#[test]
fn discounts_precede_each_line_vat_and_do_not_discount_shipping() {
    let q = original(
        &[item("A", 121, 1, 2100), item("B", 109, 1, 900)],
        1210,
        2100,
    );
    let mut p = terms();
    p.discount = DiscountKind::Fixed { minor_units: 23 };
    let r = price(&q, &p).expect("synthetic mixed rates");
    assert_eq!(
        r.lines
            .iter()
            .map(|l| (
                l.discount.minor_units(),
                l.amounts.gross.minor_units(),
                l.amounts.net.minor_units(),
                l.amounts.vat.minor_units()
            ))
            .collect::<Vec<_>>(),
        vec![(12, 109, 90, 19), (11, 98, 90, 8)]
    );
    assert_eq!(
        (
            r.shipping.gross.minor_units(),
            r.shipping.net.minor_units(),
            r.shipping.vat.minor_units(),
            r.shipping_vat_basis_points
        ),
        (1210, 1000, 210, 2100)
    );
    assert_eq!(
        (
            r.total.gross.minor_units(),
            r.total.net.minor_units(),
            r.total.vat.minor_units()
        ),
        (1417, 1180, 237)
    );
}
#[test]
fn eligibility_is_exact_and_zero_or_unselected_lines_receive_no_discount() {
    let q = original(
        &[
            item("A", 50, 2, 2100),
            item("B", 0, 1, 0),
            item("OUTSIDE", 200, 1, 0),
        ],
        10,
        0,
    );
    let r = price(&q, &terms()).expect("eligible A only");
    assert_eq!(
        r.lines
            .iter()
            .map(|l| l.discount.minor_units())
            .collect::<Vec<_>>(),
        vec![100, 0, 0]
    );
    assert_eq!(r.total.gross.minor_units(), 210);
    let mut p = terms();
    p.products[0].publication_revision = 2;
    assert_eq!(price(&q, &p), Err(PromotionError::Ineligible));
    let no_match = original(&[item("OUTSIDE", 100, 1, 0)], 0, 0);
    assert_eq!(price(&no_match, &terms()), Err(PromotionError::Ineligible));
    let zero = original(&[item("A", 0, 1, 0)], 0, 0);
    assert_eq!(price(&zero, &terms()), Err(PromotionError::Ineligible));
    p = terms();
    p.minimum_eligible_gross_minor = 101;
    assert_eq!(price(&q, &p), Err(PromotionError::Ineligible));
    p.minimum_eligible_gross_minor = 100;
    assert!(price(&q, &p).is_ok());
    for (country, now) in [
        ("DE", 1100),
        ("nl", 1100),
        ("NL", 999),
        ("NL", 2000),
        ("NL", -1),
    ] {
        assert_eq!(
            apply(&q, &terms(), country, now),
            Err(PromotionError::Ineligible)
        );
    }
    assert!(apply(&q, &terms(), "NL", 1000).is_ok());
    assert!(apply(&q, &terms(), "NL", 1999).is_ok());
}
#[test]
fn percentage_half_cent_caps_and_full_discounts_are_explicit() {
    let mut p = terms();
    p.discount = DiscountKind::Percentage { basis_points: 5000 };
    let q = original(&[item("A", 1, 1, 10000)], 7, 0);
    let r = price(&q, &p).expect("half-up discount");
    assert_eq!(
        (
            r.discount.minor_units(),
            r.lines[0].amounts.gross.minor_units(),
            r.lines[0].amounts.vat.minor_units(),
            r.total.gross.minor_units()
        ),
        (1, 0, 0, 7)
    );
    p.discount = DiscountKind::Percentage { basis_points: 1 };
    assert_eq!(price(&q, &p), Err(PromotionError::Ineligible));
    let q = original(&[item("A", 100, 1, 2100)], 0, 0);
    p.discount = DiscountKind::Percentage { basis_points: 5000 };
    p.maximum_discount_minor = 7;
    assert_eq!(price(&q, &p).expect("cap").discount.minor_units(), 7);
    p = terms();
    p.discount = DiscountKind::Fixed { minor_units: 1000 };
    let r = price(&q, &p).expect("clamp to eligible gross");
    assert_eq!(
        (
            r.discount.minor_units(),
            r.total.gross.minor_units(),
            r.total.vat.minor_units()
        ),
        (100, 0, 0)
    );
}
#[test]
fn invalid_policy_has_no_discount_authority() {
    for case in 0..24 {
        let mut p = terms();
        match case {
            0 => p.id = "".into(),
            1 => p.revision = 0,
            2 => p.valid_from = -1,
            3 => p.expires_at = 1000,
            4 => p.discount = DiscountKind::Fixed { minor_units: 0 },
            5 => {
                p.discount = DiscountKind::Fixed {
                    minor_units: MAX_MINOR_UNITS + 1,
                }
            }
            6 => p.discount = DiscountKind::Percentage { basis_points: 0 },
            7 => {
                p.discount = DiscountKind::Percentage {
                    basis_points: 10001,
                }
            }
            8 => p.minimum_eligible_gross_minor = -1,
            9 => p.minimum_eligible_gross_minor = MAX_MINOR_UNITS + 1,
            10 => p.maximum_discount_minor = 0,
            11 => p.maximum_discount_minor = MAX_MINOR_UNITS + 1,
            12 => p.countries.clear(),
            13 => p.countries = vec!["NL".into(); 28],
            14 => p.countries = vec!["US".into()],
            15 => p.countries = vec!["NL".into(); 2],
            16 => p.products.clear(),
            17 => p.products = vec![p.products[0].clone(); 21],
            18 => p.products[0].sku = "DEMO-INVALID".into(),
            19 => p.products[0].publication_revision = 0,
            20 => p.products[1] = p.products[0].clone(),
            21 => p.evidence_ref = "  ".into(),
            22 => p.evidence_ref = "a".repeat(501),
            _ => p.evidence_ref = "bad\nref".into(),
        }
        assert_eq!(
            p.validate(),
            Err(PromotionError::InvalidPolicy),
            "case {case}"
        );
    }
    assert!(terms().validate().is_ok());
}
#[test]
fn corrupt_quote_snapshots_cannot_get_new_discounted_totals() {
    let base = original(&[item("A", 100, 2, 2100), item("B", 100, 1, 0)], 10, 0);
    for case in 0..9 {
        let mut q = base.clone();
        match case {
            0 => q.total.gross = eur(0),
            1 => q.lines[0].amounts.vat = eur(0),
            2 => q.lines[0].quantity = 0,
            3 => q.lines[0].unit_gross = eur(1),
            4 => q.shipping_vat_basis_points = 10001,
            5 => q.lines.reverse(),
            6 => q.lines[1] = q.lines[0].clone(),
            7 => q.lines.clear(),
            _ => q.lines = vec![q.lines[0].clone(); 21],
        }
        assert_eq!(
            price(&q, &terms()),
            Err(PromotionError::InvalidQuote),
            "corruption {case}"
        );
    }
    assert!(price(&base, &terms()).is_ok());
}
#[test]
fn exact_integer_ceiling_never_uses_floating_point_or_overflows() {
    let q = original(&[item("A", MAX_MINOR_UNITS, 1, 2100)], 0, 0);
    let mut p = terms();
    p.discount = DiscountKind::Percentage { basis_points: 1 };
    assert_eq!(
        price(&q, &p)
            .expect("exact max percentage")
            .discount
            .minor_units(),
        900_719_925_474
    );
    p.discount = DiscountKind::Percentage {
        basis_points: 10000,
    };
    let r = price(&q, &p).expect("all");
    assert_eq!(
        (r.discount.minor_units(), r.total.gross.minor_units()),
        (MAX_MINOR_UNITS, 0)
    );
    p.discount = DiscountKind::Fixed {
        minor_units: MAX_MINOR_UNITS,
    };
    assert_eq!(price(&q, &p).expect("fixed").total.gross.minor_units(), 0);
}
#[test]
fn allocations_conserve_every_cent_and_never_make_a_line_negative() {
    for a in 1..=12 {
        for b in 1..=12 {
            for discount in 1..=a + b {
                let q = original(&[item("A", a, 1, 0), item("B", b, 1, 0)], 0, 0);
                let mut p = terms();
                p.discount = DiscountKind::Fixed {
                    minor_units: discount,
                };
                let r = price(&q, &p).expect("bounded allocation");
                assert_eq!(
                    r.lines
                        .iter()
                        .map(|l| l.discount.minor_units())
                        .sum::<i64>(),
                    discount
                );
                assert_eq!(r.total.gross.minor_units(), a + b - discount);
                assert!(r.lines.iter().all(|l|l.discount.minor_units()<=l.original.amounts.gross.minor_units()));
            }
        }
    }
}
fn amounts(gross: i64, net: i64, vat: i64) -> AmountBreakdown {
    AmountBreakdown {
        gross: eur(gross),
        net: eur(net),
        vat: eur(vat),
    }
}
#[test]
fn cumulative_refunds_keep_captured_vat_instead_of_repeating_rounding() {
    let snapshot = amounts(121, 100, 21);
    let mut prior = eur(0);
    for (gross, net, vat) in [(40, 33, 7), (40, 33, 7), (41, 34, 7)] {
        assert_eq!(
            allocate_refund(&snapshot, prior, eur(gross)).expect("credit allocation"),
            amounts(gross, net, vat)
        );
        prior = prior.checked_add(eur(gross)).expect("fixture sum");
    }
    let snapshot = amounts(3, 2, 1);
    assert_eq!(
        allocate_refund(&snapshot, eur(0), eur(1)).expect("first"),
        amounts(1, 1, 0)
    );
    assert_eq!(
        allocate_refund(&snapshot, eur(1), eur(1)).expect("second"),
        amounts(1, 0, 1)
    );
    assert_eq!(
        allocate_refund(&snapshot, eur(2), eur(1)).expect("last"),
        amounts(1, 1, 0)
    );
    let snapshot = amounts(6, 3, 3);
    assert_eq!(
        allocate_refund(&snapshot, eur(0), eur(3)).expect("half-up"),
        amounts(3, 1, 2)
    );
    assert_eq!(
        allocate_refund(&snapshot, eur(3), eur(3)).expect("remaining"),
        amounts(3, 2, 1)
    );
}
#[test]
fn malformed_exhausted_zero_and_overflow_refunds_fail_closed() {
    for (snapshot, prior, requested) in [
        (amounts(0, 0, 0), 0, 1),
        (amounts(121, 100, 21), 0, 0),
        (amounts(121, 99, 21), 0, 1),
        (amounts(MAX_MINOR_UNITS + 1, MAX_MINOR_UNITS + 1, 0), 0, 1),
        (amounts(1, i64::MAX, 1), 0, 1),
        (amounts(121, 100, 21), 121, 1),
        (amounts(121, 100, 21), 0, 122),
        (amounts(121, 100, 21), i64::MAX, 1),
        (amounts(121, 100, 21), 1, MAX_MINOR_UNITS),
    ] {
        assert!(allocate_refund(&snapshot, eur(prior), eur(requested)).is_err());
    }
    assert_eq!(
        allocate_refund(
            &amounts(MAX_MINOR_UNITS, MAX_MINOR_UNITS, 0),
            eur(0),
            eur(MAX_MINOR_UNITS)
        )
        .expect("exact max refund"),
        amounts(MAX_MINOR_UNITS, MAX_MINOR_UNITS, 0)
    );
}
#[test]
fn every_partial_refund_partition_sums_to_immutable_original_tax() {
    for gross in 1..=48 {
        for vat in 0..=gross {
            let snapshot = amounts(gross, gross - vat, vat);
            let mut net_sum = 0;
            let mut vat_sum = 0;
            for previous in 0..gross {
                let r = allocate_refund(&snapshot, eur(previous), eur(1)).expect("one cent");
                assert_eq!(r.gross.minor_units(), 1);
                assert!(r.vat.minor_units() <= 1);
                net_sum += r.net.minor_units();
                vat_sum += r.vat.minor_units();
            }
            assert_eq!((net_sum, vat_sum), (gross - vat, vat));
        }
    }
}
