//! Invented orders and quantities; no physical or delivery evidence.
use commerce_domain::{
    checkout_orders::OrderSnapshot,
    checkout_quotes::{CapturedAmount, PromotedLine, PromotedQuoteSnapshot, PromotionReference},
    order_shipments::{ShipmentLineQuantities, ShipmentQuantityError, SkuUnits, plan_units},
};

#[path = "../../../tests/fixtures/checkout_order_snapshot.rs"]
mod fixture;

fn units(sku: &str, count: u32) -> SkuUnits {
    SkuUnits {
        sku: sku.into(),
        units: count,
    }
}

fn mixed_order() -> OrderSnapshot {
    let mut order = fixture::snapshot();
    let mut line = order.quote.lines[0].clone();
    line.sku = "SET-Z-COVERS".into();
    line.quantity = 2;
    line.vat_basis_points = 900;
    line.amounts = CapturedAmount {
        gross_minor: 200,
        net_minor: 183,
        vat_minor: 17,
    };
    order.quote.lines.push(line);
    order.quote.total = CapturedAmount {
        gross_minor: 1400,
        net_minor: 1175,
        vat_minor: 225,
    };
    let mut product = order.products[0].clone();
    product.sku = "SET-Z-COVERS".into();
    product.fill = "covers-only".into();
    order.products.push(product);
    let mut rule = order.market.products[0].clone();
    rule.sku = "SET-Z-COVERS".into();
    rule.vat_basis_points = 900;
    order.market.products.push(rule);
    assert!(order.validate().is_ok());
    order
}

#[test]
fn partial_and_final_units_follow_accepted_quote_order() {
    let order = mixed_order();
    let planned = plan_units(
        &order,
        &[units("SET-Z-COVERS", 0), units("SET-FILLED", 1)],
        &[units("SET-Z-COVERS", 1), units("SET-FILLED", 2)],
    )
    .expect("valid partial proposal");
    assert_eq!(
        planned,
        vec![
            ShipmentLineQuantities {
                sku: "SET-FILLED".into(),
                ordered: 3,
                prior: 1,
                proposed: 2,
                remaining: 0,
            },
            ShipmentLineQuantities {
                sku: "SET-Z-COVERS".into(),
                ordered: 2,
                prior: 0,
                proposed: 1,
                remaining: 1,
            },
        ]
    );
    let final_units = plan_units(
        &order,
        &[units("SET-FILLED", 3), units("SET-Z-COVERS", 1)],
        &[units("SET-Z-COVERS", 1)],
    )
    .expect("remaining whole unit");
    assert_eq!(final_units[0].proposed, 0);
    assert_eq!(final_units[0].remaining, 0);
    assert_eq!(final_units[1].proposed, 1);
    assert_eq!(final_units[1].remaining, 0);
}

#[test]
fn incomplete_duplicate_unknown_zero_and_excess_counts_fail_closed() {
    let order = mixed_order();
    let cases = [
        (vec![], vec![units("SET-FILLED", 1)]),
        (vec![units("SET-FILLED", 0)], vec![units("SET-FILLED", 1)]),
        (
            vec![units("SET-FILLED", 0), units("SET-FILLED", 0)],
            vec![units("SET-FILLED", 1)],
        ),
        (
            vec![units("SET-FILLED", 0), units("UNKNOWN", 0)],
            vec![units("SET-FILLED", 1)],
        ),
        (
            vec![units("SET-FILLED", 0), units(&"X".repeat(66), 0)],
            vec![units("SET-FILLED", 1)],
        ),
        (
            vec![units("SET-FILLED", 0), units("SET-Z-COVERS", 0)],
            vec![],
        ),
        (
            vec![units("SET-FILLED", 0), units("SET-Z-COVERS", 0)],
            vec![units("SET-FILLED", 0)],
        ),
        (
            vec![units("SET-FILLED", 0), units("SET-Z-COVERS", 0)],
            vec![units("SET-FILLED", 1), units("SET-FILLED", 1)],
        ),
        (
            vec![units("SET-FILLED", 0), units("SET-Z-COVERS", 0)],
            vec![units("UNKNOWN", 1)],
        ),
        (
            vec![units("SET-FILLED", 0), units("SET-Z-COVERS", 0)],
            vec![units(&"X".repeat(66), 1)],
        ),
        (
            vec![units("SET-FILLED", 0), units("SET-Z-COVERS", 0)],
            vec![
                units("SET-FILLED", 1),
                units("SET-Z-COVERS", 1),
                units("UNKNOWN", 1),
            ],
        ),
        (
            vec![units("SET-FILLED", 4), units("SET-Z-COVERS", 0)],
            vec![units("SET-Z-COVERS", 1)],
        ),
        (
            vec![units("SET-FILLED", 2), units("SET-Z-COVERS", 0)],
            vec![units("SET-FILLED", 2)],
        ),
        (
            vec![units("SET-FILLED", u32::MAX), units("SET-Z-COVERS", 0)],
            vec![units("SET-FILLED", 1)],
        ),
    ];
    for (prior, proposed) in cases {
        assert_eq!(
            plan_units(&order, &prior, &proposed),
            Err(ShipmentQuantityError::InvalidQuantities)
        );
    }
}

#[test]
fn invalid_order_cannot_supply_a_quantity_plan() {
    let mut order = fixture::snapshot();
    order.products[0].sku = "DIFFERENT".into();
    assert_eq!(
        plan_units(&order, &[units("SET-FILLED", 0)], &[units("SET-FILLED", 1)]),
        Err(ShipmentQuantityError::InvalidOrder)
    );
}

#[test]
fn promoted_order_uses_the_same_original_units_not_discounted_money() {
    let mut order = fixture::snapshot();
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
        discount_minor: 30,
        lines: vec![PromotedLine {
            sku: "SET-FILLED".into(),
            discount_minor: 30,
            amounts: CapturedAmount {
                gross_minor: 270,
                net_minor: 223,
                vat_minor: 47,
            },
        }],
        shipping: CapturedAmount {
            gross_minor: 900,
            net_minor: 744,
            vat_minor: 156,
        },
        total: CapturedAmount {
            gross_minor: 1170,
            net_minor: 967,
            vat_minor: 203,
        },
    });
    assert!(order.validate().is_ok());
    let result = plan_units(&order, &[units("SET-FILLED", 1)], &[units("SET-FILLED", 1)])
        .expect("promoted order uses captured quantity");
    assert_eq!((result[0].ordered, result[0].remaining), (3, 1));
}
