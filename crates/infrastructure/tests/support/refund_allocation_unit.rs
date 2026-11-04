//! Independent literal controls for the private reservation allocation boundary.
use super::*;
use crate::payment_test_fixture::order::snapshot_fixture;

fn allocation(line: (i64, i64, i64), shipping: (i64, i64, i64)) -> RefundAllocation {
    RefundAllocation {
        schema_version: 1,
        lines: vec![RefundAllocationLine {
            sku: "SET-FILLED".into(),
            gross_minor: line.0,
            net_minor: line.1,
            vat_minor: line.2,
        }],
        shipping: RefundAllocationAmounts {
            gross_minor: shipping.0,
            net_minor: shipping.1,
            vat_minor: shipping.2,
        },
        total: RefundAllocationAmounts {
            gross_minor: line.0 + shipping.0,
            net_minor: line.1 + shipping.1,
            vat_minor: line.2 + shipping.2,
        },
    }
}

fn receipt(generation: i16, reserved_at: i64, allocation: RefundAllocation) -> RefundReceipt {
    RefundReceipt {
        id: "reservation".into(),
        order_id: "3".repeat(32),
        attempt_id: "attempt".into(),
        recognition_id: "recognition".into(),
        actor_id: "actor".into(),
        effect_key: "effect".into(),
        generation,
        reason: "fixture".into(),
        reserved_at,
        allocation,
        replayed: false,
    }
}

fn first() -> RefundReceipt {
    receipt(1, 1200, allocation((100, 83, 17), (100, 83, 17)))
}

fn second() -> RefundReceipt {
    receipt(2, 1201, allocation((200, 165, 35), (800, 661, 139)))
}

fn request<'a>(lines: &'a [RefundLine<'a>], shipping: i64) -> RefundRequest<'a> {
    RefundRequest {
        order_id: "order",
        expected_recognition_id: "recognition",
        effect_key: "effect",
        lines,
        shipping_gross_minor: shipping,
        reason: "fixture",
        confirmed: true,
    }
}

#[test]
fn history_reconstructs_each_tax_delta_and_rejects_identity_or_clock_changes() {
    let order = snapshot_fixture::snapshot();
    assert_eq!(
        validate_history(&order, "attempt", &[first(), second()]),
        Ok(())
    );

    let mut wrong_order = first();
    wrong_order.order_id = "other".into();
    let mut wrong_attempt = first();
    wrong_attempt.attempt_id = "other".into();
    let mut wrong_generation = first();
    wrong_generation.generation = 2;
    let mut before_order = first();
    before_order.reserved_at = 1199;
    for corrupt in [wrong_order, wrong_attempt, wrong_generation, before_order] {
        assert_eq!(
            validate_history(&order, "attempt", &[corrupt]),
            Err(RefundError::InvalidStoredData)
        );
    }
    let mut later_first = first();
    later_first.reserved_at = 1300;
    let mut backwards = second();
    backwards.reserved_at = 1299;
    assert_eq!(
        validate_history(&order, "attempt", &[later_first, backwards]),
        Err(RefundError::InvalidStoredData)
    );
    let mut wrong_tax = second();
    wrong_tax.allocation.lines[0].net_minor += 1;
    wrong_tax.allocation.lines[0].vat_minor -= 1;
    assert_eq!(
        validate_history(&order, "attempt", &[first(), wrong_tax]),
        Err(RefundError::InvalidStoredData)
    );
    let mut over_capture = second();
    over_capture.allocation.lines[0].gross_minor += 1;
    assert_eq!(
        validate_history(&order, "attempt", &[first(), over_capture]),
        Err(RefundError::InvalidStoredData)
    );
    let mut negative = first();
    negative.allocation.shipping.gross_minor = -1;
    assert_eq!(
        validate_history(&order, "attempt", &[negative]),
        Err(RefundError::InvalidStoredData)
    );
    let mut unknown = first();
    unknown.allocation.lines[0].sku = "UNKNOWN".into();
    assert_eq!(
        validate_history(&order, "attempt", &[unknown]),
        Err(RefundError::InvalidStoredData)
    );
    let mut wrong_total = first();
    wrong_total.allocation.total.gross_minor += 1;
    assert_eq!(
        validate_history(&order, "attempt", &[wrong_total]),
        Err(RefundError::InvalidStoredData)
    );
    let mut wrong_schema = first();
    wrong_schema.allocation.schema_version = 2;
    assert_eq!(
        validate_history(&order, "attempt", &[wrong_schema]),
        Err(RefundError::InvalidStoredData)
    );
}

#[test]
fn next_reservation_uses_captured_remainder_and_rejects_excess() {
    let order = snapshot_fixture::snapshot();
    let remaining = [RefundLine {
        sku: "SET-FILLED",
        gross_minor: 200,
    }];
    assert_eq!(
        calculate(&order, &[first()], &request(&remaining, 800)),
        Ok(second().allocation)
    );
    let excess = [RefundLine {
        sku: "SET-FILLED",
        gross_minor: 201,
    }];
    assert_eq!(
        calculate(&order, &[first()], &request(&excess, 0)),
        Err(RefundError::InvalidRequest)
    );
    let negative = [RefundLine {
        sku: "SET-FILLED",
        gross_minor: -1,
    }];
    assert_eq!(
        calculate(&order, &[first()], &request(&negative, 0)),
        Err(RefundError::InvalidRequest)
    );
}

#[test]
fn replay_compares_complete_unique_sku_and_shipping_amounts() {
    let saved = allocation((100, 83, 17), (100, 83, 17));
    let exact = [RefundLine {
        sku: "SET-FILLED",
        gross_minor: 100,
    }];
    assert!(same_request(&saved, &request(&exact, 100)));
    assert!(!same_request(&saved, &request(&exact, 99)));
    assert!(!same_request(&saved, &request(&[], 100)));
    let changed = [RefundLine {
        sku: "SET-FILLED",
        gross_minor: 99,
    }];
    assert!(!same_request(&saved, &request(&changed, 100)));
    let unknown = [RefundLine {
        sku: "UNKNOWN",
        gross_minor: 100,
    }];
    assert!(!same_request(&saved, &request(&unknown, 100)));
    let mut two = saved;
    two.lines.push(RefundAllocationLine {
        sku: "OTHER".into(),
        gross_minor: 0,
        net_minor: 0,
        vat_minor: 0,
    });
    let duplicate = [
        RefundLine {
            sku: "SET-FILLED",
            gross_minor: 100,
        },
        RefundLine {
            sku: "SET-FILLED",
            gross_minor: 0,
        },
    ];
    assert!(!same_request(&two, &request(&duplicate, 100)));
}

#[test]
fn decoding_is_closed_at_all_allocation_levels() {
    let good = serde_json::json!({
        "schema_version": 1,
        "lines": [{"sku":"SET-FILLED", "gross_minor":100, "net_minor":83, "vat_minor":17}],
        "shipping": {"gross_minor":100, "net_minor":83, "vat_minor":17},
        "total": {"gross_minor":200, "net_minor":166, "vat_minor":34}
    });
    assert_eq!(
        serde_json::from_value::<RefundAllocation>(good.clone()).expect("closed allocation"),
        first().allocation
    );
    for path in ["root", "line", "shipping", "total"] {
        let mut bad = good.clone();
        match path {
            "root" => bad["extra"] = true.into(),
            "line" => bad["lines"][0]["extra"] = true.into(),
            "shipping" => bad["shipping"]["extra"] = true.into(),
            "total" => bad["total"]["extra"] = true.into(),
            _ => unreachable!(),
        }
        assert!(
            serde_json::from_value::<RefundAllocation>(bad).is_err(),
            "{path}"
        );
    }
}
