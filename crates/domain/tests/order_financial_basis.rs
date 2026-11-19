//! Synthetic financial history, not fiscal or payment evidence.
use commerce_domain::{
    Money,
    checkout_orders::OrderSnapshot,
    checkout_quotes::{CapturedAmount, PromotedLine, PromotedQuoteSnapshot, PromotionReference},
    order_financial_basis::{BasisError, order_financial_basis},
};
#[path = "../../../tests/fixtures/checkout_order_snapshot.rs"]
mod fixture;

fn amount(gross: i64, net: i64, vat: i64) -> CapturedAmount {
    CapturedAmount {
        gross_minor: gross,
        net_minor: net,
        vat_minor: vat,
    }
}

fn mixed() -> OrderSnapshot {
    let mut order = fixture::snapshot();
    let mut line = order.quote.lines[0].clone();
    line.sku = "SET-OTHER".into();
    line.quantity = 2;
    line.vat_basis_points = 900;
    line.amounts = amount(200, 183, 17);
    order.quote.lines.push(line);
    order.quote.total = amount(1400, 1175, 225);
    let mut product = order.products[0].clone();
    product.sku = "SET-OTHER".into();
    order.products.push(product);
    let mut rule = order.market.products[0].clone();
    rule.sku = "SET-OTHER".into();
    rule.vat_basis_points = 900;
    order.market.products.push(rule);
    order
}

fn promoted() -> OrderSnapshot {
    let mut order = mixed();
    order.schema_version = 2;
    order.promoted_quote = Some(PromotedQuoteSnapshot {
        schema_version: 2,
        original: order.quote.clone(),
        promotion: PromotionReference {
            id: "SYNTHETIC-OFFER".into(),
            revision: 7,
            review_revision: 4,
        },
        allocation_version: 1,
        discount_minor: 40,
        lines: vec![
            PromotedLine {
                sku: "SET-FILLED".into(),
                discount_minor: 30,
                amounts: amount(270, 223, 47),
            },
            PromotedLine {
                sku: "SET-OTHER".into(),
                discount_minor: 10,
                amounts: amount(190, 174, 16),
            },
        ],
        shipping: amount(900, 744, 156),
        total: amount(1360, 1141, 219),
    });
    order
}

#[test]
fn original_and_promoted_mixed_rates_keep_exact_captured_components() {
    let plain = order_financial_basis(&mixed()).expect("valid mixed order");
    assert_eq!(plain.currency, "EUR");
    assert_eq!(plain.lines.len(), 2);
    assert_eq!(
        (
            plain.lines[0].sku.as_str(),
            plain.lines[0].quantity,
            plain.lines[0].unit_gross_minor
        ),
        ("SET-FILLED", 3, 100)
    );
    assert_eq!(plain.lines[1].original, amount(200, 183, 17));
    assert_eq!(plain.lines[1].payable, amount(200, 183, 17));
    assert_eq!(plain.discount_minor, 0);
    assert_eq!(plain.original_total, amount(1400, 1175, 225));
    assert_eq!(plain.payable_total, amount(1400, 1175, 225));
    assert_eq!(plain.shipping.original, amount(900, 744, 156));
    assert_eq!(plain.shipping.payable, amount(900, 744, 156));
    assert_eq!(
        plain
            .rates
            .iter()
            .map(|r| r.vat_basis_points)
            .collect::<Vec<_>>(),
        vec![900, 2100]
    );
    assert_eq!(plain.rates[0].original, amount(200, 183, 17));
    assert_eq!(plain.rates[1].original, amount(1200, 992, 208));

    let basis = order_financial_basis(&promoted()).expect("valid promoted order");
    assert_eq!(basis.lines[0].original, amount(300, 248, 52));
    assert_eq!(basis.lines[0].payable, amount(270, 223, 47));
    assert_eq!(basis.lines[0].discount_minor, 30);
    assert_eq!(basis.lines[1].payable, amount(190, 174, 16));
    assert_eq!(basis.lines[1].discount_minor, 10);
    assert_eq!(basis.rates[0].payable, amount(190, 174, 16));
    assert_eq!(basis.rates[0].discount_minor, 10);
    assert_eq!(basis.rates[1].payable, amount(1170, 967, 203));
    assert_eq!(basis.rates[1].discount_minor, 30);
    assert_eq!(basis.original_total, amount(1400, 1175, 225));
    assert_eq!(basis.payable_total, amount(1360, 1141, 219));
    assert_eq!(basis.discount_minor, 40);
}

#[test]
fn grouping_sums_individually_rounded_lines_and_zero_shipping() {
    let mut order = mixed();
    for line in &mut order.quote.lines {
        line.quantity = 1;
        line.unit_gross_minor = 3;
        line.vat_basis_points = 2100;
        line.amounts = amount(3, 2, 1);
    }
    order.market.products[1].vat_basis_points = 2100;
    order.market.shipping.gross = Money::eur(0).expect("zero");
    order.quote.shipping = amount(0, 0, 0);
    order.quote.total = amount(6, 4, 2);
    let basis = order_financial_basis(&order).expect("valid independently rounded lines");
    assert_eq!(basis.rates.len(), 1);
    assert_eq!(basis.rates[0].original, amount(6, 4, 2));
    assert_eq!(basis.shipping.payable, amount(0, 0, 0));
}

#[test]
fn shipping_is_grouped_at_its_own_captured_rate() {
    let mut order = mixed();
    order.market.shipping.vat_basis_points = 900;
    order.quote.shipping_vat_basis_points = 900;
    order.quote.shipping = amount(900, 826, 74);
    order.quote.total = amount(1400, 1257, 143);
    let basis = order_financial_basis(&order).expect("valid shipping rate");
    assert_eq!(basis.shipping.vat_basis_points, 900);
    assert_eq!(basis.rates[0].original, amount(1100, 1009, 91));
    assert_eq!(basis.rates[1].original, amount(300, 248, 52));
}

#[test]
fn rejects_forged_or_incomplete_orders() {
    let edits: [fn(&mut OrderSnapshot); 7] = [
        |o| o.quote.total.vat_minor += 1,
        |o| o.quote.lines[0].amounts.net_minor += 1,
        |o| o.quote.lines[1].sku = o.quote.lines[0].sku.clone(),
        |o| o.quote.shipping_vat_basis_points = 900,
        |o| {
            let _ = o.promoted_quote.as_mut().expect("promotion").lines.pop();
        },
        |o| o.promoted_quote.as_mut().expect("promotion").lines[0].discount_minor += 1,
        |o| {
            o.promoted_quote
                .as_mut()
                .expect("promotion")
                .total
                .gross_minor += 1
        },
    ];
    for edit in edits {
        let mut order = promoted();
        edit(&mut order);
        assert_eq!(order_financial_basis(&order), Err(BasisError::InvalidOrder));
    }
}

#[test]
fn private_contact_and_seller_values_do_not_enter_basis() {
    let order = mixed();
    let expected = order_financial_basis(&order).expect("valid order");
    let mut changed = order;
    changed.buyer.name = "Other Synthetic Buyer".into();
    changed.buyer.email = "other@example.test".into();
    changed.billing_address.line1 = "Other billing address".into();
    changed.shipping_address.line1 = "Other shipping address".into();
    changed.seller.legal_name = "Other Synthetic Seller".into();
    assert_eq!(order_financial_basis(&changed), Ok(expected));
}
