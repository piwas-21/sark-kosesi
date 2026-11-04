//! Synthetic captured amounts; these do not establish fiscal acceptance.
use commerce_domain::{Money, checkout_quotes::CapturedAmount, order_refunds::*};
#[path = "../../../tests/fixtures/checkout_order_snapshot.rs"]
mod fixture;

fn eur(n: i64) -> Money {
    Money::eur(n).expect("fixture")
}
fn amounts(prior: i64, requested: i64) -> RefundAmounts {
    RefundAmounts {
        already_refunded_gross: eur(prior),
        requested_gross: eur(requested),
    }
}
fn line(sku: &str, prior: i64, requested: i64) -> SkuRefund {
    SkuRefund {
        sku: sku.into(),
        amounts: amounts(prior, requested),
    }
}
fn assert_amount(
    actual: &commerce_domain::quotes::AmountBreakdown,
    gross: i64,
    net: i64,
    vat: i64,
) {
    assert_eq!(
        (
            actual.gross.minor_units(),
            actual.net.minor_units(),
            actual.vat.minor_units()
        ),
        (gross, net, vat)
    );
}
fn mixed_order() -> commerce_domain::checkout_orders::OrderSnapshot {
    let mut order = fixture::snapshot();
    let mut second = order.quote.lines[0].clone();
    second.sku = "SET-OTHER".into();
    second.quantity = 2;
    second.vat_basis_points = 900;
    second.amounts = CapturedAmount {
        gross_minor: 200,
        net_minor: 183,
        vat_minor: 17,
    };
    order.quote.lines.push(second);
    order.quote.total = CapturedAmount {
        gross_minor: 1400,
        net_minor: 1175,
        vat_minor: 225,
    };
    let mut product = order.products[0].clone();
    product.sku = "SET-OTHER".into();
    order.products.push(product);
    let mut rule = order.market.products[0].clone();
    rule.sku = "SET-OTHER".into();
    rule.vat_basis_points = 900;
    order.market.products.push(rule);
    assert!(order.validate().is_ok());
    order
}

#[test]
fn mixed_lines_and_shipping_preserve_captured_tax_through_partial_then_full_refunds() {
    let order = mixed_order();
    let first = allocate_order_refund(
        &order,
        &[line("SET-OTHER", 0, 100), line("SET-FILLED", 0, 101)],
        &amounts(0, 100),
    )
    .expect("first allocation");
    assert_eq!(first.lines[0].0, "SET-FILLED");
    assert_amount(&first.lines[0].1, 101, 83, 18);
    assert_amount(&first.lines[1].1, 100, 91, 9);
    assert_amount(&first.shipping, 100, 83, 17);
    assert_amount(&first.total, 301, 257, 44);
    let last = allocate_order_refund(
        &order,
        &[line("SET-FILLED", 101, 199), line("SET-OTHER", 100, 100)],
        &amounts(100, 800),
    )
    .expect("remaining allocation");
    assert_amount(&last.lines[0].1, 199, 165, 34);
    assert_amount(&last.lines[1].1, 100, 92, 8);
    assert_amount(&last.shipping, 800, 661, 139);
    assert_amount(&last.total, 1099, 918, 181);
}

#[test]
fn requests_are_explicit_and_zero_shipping_does_not_require_a_refund() {
    let mut order = fixture::snapshot();
    order.market.shipping.gross = eur(0);
    order.quote.shipping = CapturedAmount {
        gross_minor: 0,
        net_minor: 0,
        vat_minor: 0,
    };
    order.quote.total = CapturedAmount {
        gross_minor: 300,
        net_minor: 248,
        vat_minor: 52,
    };
    let result = allocate_order_refund(&order, &[line("SET-FILLED", 299, 1)], &amounts(0, 0))
        .expect("final cent");
    assert_amount(&result.lines[0].1, 1, 1, 0);
    assert_amount(&result.shipping, 0, 0, 0);
    assert_amount(&result.total, 1, 1, 0);
    assert_eq!(
        allocate_order_refund(&order, &[line("SET-FILLED", 0, 0)], &amounts(0, 0)),
        Err(RefundAllocationError::InvalidAllocation)
    );
    assert_eq!(
        allocate_order_refund(&order, &[line("SET-FILLED", 0, 1)], &amounts(0, 1)),
        Err(RefundAllocationError::InvalidAllocation)
    );
}

#[test]
fn consecutive_cents_straddle_the_exact_vat_boundary_at_the_capture_ceiling() {
    let mut order = fixture::snapshot();
    let captured = CapturedAmount {
        gross_minor: 9_007_199_254_740_991,
        net_minor: 7_443_966_326_232_224,
        vat_minor: 1_563_232_928_508_767,
    };
    order.quote.lines[0].quantity = 1;
    order.quote.lines[0].unit_gross_minor = captured.gross_minor;
    order.quote.lines[0].amounts = captured.clone();
    order.quote.total = captured;
    order.quote.shipping = CapturedAmount {
        gross_minor: 0,
        net_minor: 0,
        vat_minor: 0,
    };
    order.market.shipping.gross = eur(0);

    // The first cumulative VAT numerator leaves a remainder of gross minus one.
    // The next cent alone crosses the exact rounding boundary.
    let before = allocate_order_refund(
        &order,
        &[line("SET-FILLED", 1_125_899_906_842_638, 1)],
        &amounts(0, 0),
    )
    .expect("cent before VAT boundary");
    let after = allocate_order_refund(
        &order,
        &[line("SET-FILLED", 1_125_899_906_842_639, 1)],
        &amounts(0, 0),
    )
    .expect("cent crossing VAT boundary");
    assert_amount(&before.total, 1, 1, 0);
    assert_amount(&after.total, 1, 0, 1);
}

#[test]
fn incomplete_duplicate_unknown_and_excess_balances_fail_closed() {
    let order = mixed_order();
    let bad = [
        vec![],
        vec![line("SET-FILLED", 0, 1)],
        vec![line("SET-FILLED", 0, 1), line("SET-FILLED", 0, 1)],
        vec![line("SET-FILLED", 0, 1), line("UNKNOWN", 0, 1)],
        vec![line("SET-FILLED", 301, 0), line("SET-OTHER", 0, 1)],
        vec![line("SET-FILLED", 299, 2), line("SET-OTHER", 0, 0)],
        vec![line("SET-FILLED", i64::MAX, 1), line("SET-OTHER", 0, 0)],
    ];
    for lines in bad {
        assert_eq!(
            allocate_order_refund(&order, &lines, &amounts(0, 0)),
            Err(RefundAllocationError::InvalidAllocation)
        );
    }
    assert_eq!(
        allocate_order_refund(
            &order,
            &[line("SET-FILLED", 0, 1), line("SET-OTHER", 0, 0)],
            &amounts(901, 0)
        ),
        Err(RefundAllocationError::InvalidAllocation)
    );
    assert_eq!(
        allocate_order_refund(
            &order,
            &[line("SET-FILLED", 0, 0), line("SET-OTHER", 0, 0)],
            &amounts(900, 1)
        ),
        Err(RefundAllocationError::InvalidAllocation)
    );
    let mut invalid = order;
    invalid.quote.lines[0].amounts.vat_minor = 51;
    assert_eq!(
        allocate_order_refund(
            &invalid,
            &[line("SET-FILLED", 0, 1), line("SET-OTHER", 0, 0)],
            &amounts(0, 0)
        ),
        Err(RefundAllocationError::InvalidOrder)
    );
}
