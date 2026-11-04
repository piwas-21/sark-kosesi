//! Literal controls for the refund command's source, time and write bindings.
use super::*;
use crate::payment_test_fixture::order::snapshot_fixture;
use commerce_domain::payment_attempts::{PaymentMode, ProfileBinding};

fn binding(order: &OrderSnapshot) -> AttemptBinding {
    AttemptBinding {
        id: "attempt".into(),
        order_id: order.identity.id.clone(),
        basket_id: order.identity.basket_id.clone(),
        stock_group_id: order.identity.stock_group_id.clone(),
        profile: ProfileBinding {
            id: "pfl_fixture".into(),
            credential_fingerprint: "a".repeat(64),
            mode: PaymentMode::Test,
        },
        configuration_sha256: "b".repeat(64),
        currency: Currency::EUR,
        amount_minor: order
            .quote
            .totals()
            .expect("valid fixture quote")
            .total
            .gross
            .minor_units(),
        created_at: 1200,
        dispatch_before: 1800,
    }
}
fn received(order: &OrderSnapshot, binding: &AttemptBinding) -> recognition::Recognition {
    recognition::Recognition {
        id: "recognition".into(),
        order_id: order.identity.id.clone(),
        attempt_id: binding.id.clone(),
        resolution_id: "resolution".into(),
        through_observation: 1,
        recorded_at: 1300,
        state: recognition::State::Received,
        currency: Currency::EUR,
        amount_minor: binding.amount_minor,
    }
}
fn receipt(at: i64) -> RefundReceipt {
    RefundReceipt {
        id: "receipt".into(),
        order_id: "order".into(),
        attempt_id: "attempt".into(),
        recognition_id: "recognition".into(),
        actor_id: "actor".into(),
        effect_key: "effect".into(),
        generation: 1,
        reason: "fixture".into(),
        reserved_at: at,
        allocation: RefundAllocation {
            schema_version: 1,
            lines: vec![],
            shipping: RefundAllocationAmounts {
                gross_minor: 0,
                net_minor: 0,
                vat_minor: 0,
            },
            total: RefundAllocationAmounts {
                gross_minor: 0,
                net_minor: 0,
                vat_minor: 0,
            },
        },
        replayed: false,
    }
}

#[test]
fn exact_capture_and_current_intent_reject_independent_mismatches() {
    let order = snapshot_fixture::snapshot();
    let binding = binding(&order);
    let received = received(&order, &binding);
    assert_eq!(
        validate_capture(&order, &binding, &received, "attempt"),
        Ok(())
    );
    let mut wrong = received.clone();
    wrong.order_id = "other-order".into();
    assert_eq!(
        validate_capture(&order, &binding, &wrong, "attempt"),
        Err(RefundError::InvalidStoredData)
    );
    let mut wrong = received.clone();
    wrong.attempt_id = "other-attempt".into();
    assert_eq!(
        validate_capture(&order, &binding, &wrong, "attempt"),
        Err(RefundError::InvalidStoredData)
    );
    let mut wrong = received.clone();
    wrong.amount_minor += 1;
    assert_eq!(
        validate_capture(&order, &binding, &wrong, "attempt"),
        Err(RefundError::InvalidStoredData)
    );
    let mut wrong_binding = binding.clone();
    wrong_binding.amount_minor += 1;
    assert_eq!(
        validate_capture(&order, &wrong_binding, &received, "attempt"),
        Err(RefundError::InvalidStoredData)
    );
    let mut corrupt_order = order.clone();
    corrupt_order.quote.currency = "USD".into();
    assert_eq!(
        validate_capture(&corrupt_order, &binding, &received, "attempt"),
        Err(RefundError::InvalidStoredData)
    );

    let lines = [RefundLine {
        sku: "SET-FILLED",
        gross_minor: 1,
    }];
    let input = RefundRequest {
        order_id: &order.identity.id,
        expected_recognition_id: &received.id,
        effect_key: "effect",
        lines: &lines,
        shipping_gross_minor: 0,
        reason: "fixture",
        confirmed: true,
    };
    assert_eq!(validate_current_intent(&order, &received, &input), Ok(()));
    let wrong_order = RefundRequest {
        order_id: "other",
        ..input
    };
    assert_eq!(
        validate_current_intent(&order, &received, &wrong_order),
        Err(RefundError::Conflict)
    );
    let wrong_proof = RefundRequest {
        order_id: &order.identity.id,
        expected_recognition_id: "other",
        ..wrong_order
    };
    assert_eq!(
        validate_current_intent(&order, &received, &wrong_proof),
        Err(RefundError::Conflict)
    );
}

#[test]
fn reservation_time_and_written_receipt_require_each_original_boundary() {
    let order = snapshot_fixture::snapshot();
    let binding = binding(&order);
    let received = received(&order, &binding);
    assert_eq!(order.identity.created_at, 1200);
    assert_eq!(
        validate_reservation_time(1300, &order, &received, &[]),
        Ok(())
    );
    assert_eq!(
        validate_reservation_time(1299, &order, &received, &[]),
        Err(RefundError::ClockUnavailable)
    );
    let mut early_received = received.clone();
    early_received.recorded_at = 1100;
    assert_eq!(
        validate_reservation_time(1199, &order, &early_received, &[]),
        Err(RefundError::ClockUnavailable)
    );
    assert_eq!(
        validate_reservation_time(1400, &order, &received, &[receipt(1400)]),
        Ok(())
    );
    assert_eq!(
        validate_reservation_time(1399, &order, &received, &[receipt(1400)]),
        Err(RefundError::ClockUnavailable)
    );
    let expected = receipt(1400);
    assert_eq!(validate_written(&[receipt(1400)], &expected), Ok(()));
    assert_eq!(
        validate_written(&[], &expected),
        Err(RefundError::InvalidStoredData)
    );
    assert_eq!(
        validate_written(&[receipt(1401)], &expected),
        Err(RefundError::InvalidStoredData)
    );
}

#[test]
fn boundary_errors_preserve_classification_and_redact_database_detail() {
    assert_eq!(staff(StaffError::Denied), RefundError::Denied);
    assert_eq!(staff(StaffError::InvalidRequest), RefundError::Denied);
    assert_eq!(
        staff(StaffError::ClockUnavailable),
        RefundError::ClockUnavailable
    );
    let marker = "private-sql-or-provider-diagnostic";
    let private = sqlx::Error::Protocol(marker.into());
    assert!(format!("{private:?}").contains(marker));
    let public = staff(StaffError::Database(private));
    assert_eq!(public, RefundError::Database);
    assert!(!format!("{public:?}").contains(marker));
    let public = RefundError::from(sqlx::Error::Protocol(marker.into()));
    assert_eq!(public, RefundError::Database);
    assert!(!format!("{public:?}").contains(marker));
}
