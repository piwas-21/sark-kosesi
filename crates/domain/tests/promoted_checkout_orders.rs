//! Synthetic accepted allocation; no seller or fiscal evidence.
use commerce_domain::{
    Money,
    checkout_orders::{OrderError, OrderSnapshot},
    checkout_quotes::{CapturedAmount, PromotedLine, PromotedQuoteSnapshot, PromotionReference},
    order_refunds::{RefundAllocationError, RefundAmounts, SkuRefund, allocate_order_refund},
};
#[path = "../../../tests/fixtures/checkout_order_snapshot.rs"]
mod fixture;

fn promoted_order() -> OrderSnapshot {
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
    order
}

#[test]
fn legacy_and_promoted_wire_roundtrips_preserve_exact_owned_quotes() {
    let legacy = fixture::snapshot();
    let value = serde_json::to_value(&legacy).expect("legacy JSON");
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["quote"]["total"]["gross_minor"], 1200);
    assert!(value.get("promoted_quote").is_none());
    assert!(serde_json::from_value::<OrderSnapshot>(value).expect("legacy") == legacy);

    let order = promoted_order();
    assert_eq!(order.validate(), Ok(()));
    assert_eq!(order.payable_total().gross_minor, 1170);
    assert_eq!(
        order.payable_line_amounts(0).expect("line").gross_minor,
        270
    );
    assert!(order.payable_line_amounts(1).is_none());
    let value = serde_json::to_value(&order).expect("promoted JSON");
    assert_eq!(value["schema_version"], 2);
    assert_eq!(value["quote"]["original"]["total"]["gross_minor"], 1200);
    assert_eq!(value["quote"]["total"]["gross_minor"], 1170);
    assert_eq!(value["quote"]["lines"][0]["discount_minor"], 30);
    assert!(serde_json::from_value::<OrderSnapshot>(value.clone()).expect("promoted") == order);
    let mut unknown = value;
    unknown["quote"]["extra"] = true.into();
    assert!(serde_json::from_value::<OrderSnapshot>(unknown).is_err());
}

#[test]
fn promoted_order_rejects_mismatched_source_and_discount_allocations() {
    let edits: [fn(&mut OrderSnapshot); 16] = [
        |s| s.schema_version = 1,
        |s| s.promoted_quote.as_mut().unwrap().schema_version = 1,
        |s| s.promoted_quote.as_mut().unwrap().original.binding.quote_id = "5".repeat(32),
        |s| s.promoted_quote.as_mut().unwrap().allocation_version = 2,
        |s| s.promoted_quote.as_mut().unwrap().promotion.id = "DEMO".into(),
        |s| s.promoted_quote.as_mut().unwrap().promotion.revision = 0,
        |s| s.promoted_quote.as_mut().unwrap().promotion.review_revision = 0,
        |s| s.promoted_quote.as_mut().unwrap().discount_minor = 0,
        |s| s.promoted_quote.as_mut().unwrap().discount_minor = 31,
        |s| s.promoted_quote.as_mut().unwrap().lines.clear(),
        |s| s.promoted_quote.as_mut().unwrap().lines[0].sku = "OTHER".into(),
        |s| s.promoted_quote.as_mut().unwrap().lines[0].discount_minor = -1,
        |s| {
            s.promoted_quote.as_mut().unwrap().lines[0]
                .amounts
                .gross_minor += 1
        },
        |s| {
            s.promoted_quote.as_mut().unwrap().lines[0]
                .amounts
                .vat_minor += 1
        },
        |s| s.promoted_quote.as_mut().unwrap().shipping.net_minor += 1,
        |s| s.promoted_quote.as_mut().unwrap().total.gross_minor += 1,
    ];
    for edit in edits {
        let mut order = promoted_order();
        edit(&mut order);
        assert_eq!(order.validate(), Err(OrderError::InvalidSnapshot));
    }
}

#[test]
fn refunds_are_capped_to_discounted_line_and_preserve_its_captured_vat() {
    fn amount(gross: i64) -> RefundAmounts {
        RefundAmounts {
            already_refunded_gross: Money::eur(0).unwrap(),
            requested_gross: Money::eur(gross).unwrap(),
        }
    }
    let order = promoted_order();
    let line = |gross| SkuRefund {
        sku: "SET-FILLED".into(),
        amounts: amount(gross),
    };
    let result = allocate_order_refund(&order, &[line(270)], &amount(900)).expect("captured total");
    assert_eq!(
        (
            result.lines[0].1.gross.minor_units(),
            result.lines[0].1.net.minor_units(),
            result.lines[0].1.vat.minor_units()
        ),
        (270, 223, 47)
    );
    assert_eq!(
        (
            result.total.gross.minor_units(),
            result.total.net.minor_units(),
            result.total.vat.minor_units()
        ),
        (1170, 967, 203)
    );
    assert_eq!(
        allocate_order_refund(&order, &[line(271)], &amount(0)),
        Err(RefundAllocationError::InvalidAllocation)
    );
}

#[test]
fn captured_amount_conversion_rejects_each_negative_component() {
    let amount = CapturedAmount {
        gross_minor: 270,
        net_minor: 223,
        vat_minor: 47,
    };
    let converted = amount.breakdown().expect("nonnegative captured amount");
    assert_eq!(
        (
            converted.gross.minor_units(),
            converted.net.minor_units(),
            converted.vat.minor_units()
        ),
        (270, 223, 47)
    );
    for invalid in [
        CapturedAmount {
            gross_minor: -1,
            ..amount.clone()
        },
        CapturedAmount {
            net_minor: -1,
            ..amount.clone()
        },
        CapturedAmount {
            vat_minor: -1,
            ..amount.clone()
        },
    ] {
        assert!(invalid.breakdown().is_err());
    }
}
