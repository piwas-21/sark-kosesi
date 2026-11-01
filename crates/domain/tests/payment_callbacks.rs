//! Independent closed callback receipt field and boundary oracles.
use commerce_domain::{
    Currency,
    payment_attempts::{PaymentError, PaymentMode, PaymentStatus, ProviderPayment, callbacks::*},
};
fn operation() -> LookupOperationSnapshot {
    LookupOperationSnapshot {
        schema_version: 1,
        id: "1".repeat(32),
        profile_id: "pfl_SYNTHETIC".into(),
        provider_id: "tr_SYNTHETIC".into(),
        created_at: 1500,
    }
}
fn receipt() -> LookupSnapshot {
    LookupSnapshot {
        schema_version: 1,
        id: "2".repeat(32),
        operation: operation(),
        started_at: 1600,
        recorded_at: 5000,
        credential_fingerprint: "a".repeat(64),
        payment: ProviderPayment {
            resource: "payment".into(),
            id: "tr_SYNTHETIC".into(),
            profile_id: "pfl_SYNTHETIC".into(),
            mode: PaymentMode::Test,
            currency: Currency::EUR,
            amount_minor: 38245,
            metadata_schema_version: 1,
            order_id: "3".repeat(32),
            attempt_id: "4".repeat(32),
            status: PaymentStatus::Paid,
        },
    }
}
#[test]
fn callback_operation_has_closed_schema_identity_scope_and_safe_time_bounds() {
    let changes: [fn(&mut LookupOperationSnapshot); 10] = [
        |s| s.schema_version = 2,
        |s| s.id = "g".repeat(32),
        |s| s.profile_id = "pfl_".into(),
        |s| s.profile_id = "pfl_bad\n".into(),
        |s| s.provider_id = "tr_".into(),
        |s| s.provider_id = "tr_ bad ".into(),
        |s| s.provider_id = format!("tr_{}", "x".repeat(254)),
        |s| s.created_at = -1,
        |s| s.created_at = 9_007_199_254_740_992,
        |s| s.id = "1".repeat(33),
    ];
    for change in changes {
        let mut s = operation();
        change(&mut s);
        assert_eq!(s.validate(), Err(PaymentError::InvalidSnapshot));
    }
    for at in [0, 9_007_199_254_740_991] {
        let mut s = operation();
        s.created_at = at;
        assert_eq!(s.validate(), Ok(()));
    }
    assert_eq!(operation().validate(), Ok(()));
}
#[test]
fn fetched_receipt_validates_every_binding_without_inventing_original_order_authority() {
    let changes: [fn(&mut LookupSnapshot); 17] = [
        |s| s.operation.schema_version = 2,
        |s| s.schema_version = 2,
        |s| s.id = "bad".into(),
        |s| s.started_at = 1499,
        |s| s.started_at = 9_007_199_254_740_992,
        |s| s.recorded_at = 1599,
        |s| s.recorded_at = 9_007_199_254_740_992,
        |s| s.credential_fingerprint = "g".repeat(64),
        |s| s.payment.resource = "refund".into(),
        |s| s.payment.id = "tr_OTHER".into(),
        |s| s.payment.profile_id = "pfl_OTHER".into(),
        |s| s.payment.metadata_schema_version = 2,
        |s| s.payment.order_id = "bad".into(),
        |s| s.payment.attempt_id = "bad".into(),
        |s| s.payment.amount_minor = 0,
        |s| s.payment.amount_minor = -1,
        |s| s.payment.amount_minor = 9_007_199_254_740_992,
    ];
    for change in changes {
        let mut s = receipt();
        change(&mut s);
        assert_eq!(s.validate(), Err(PaymentError::InvalidSnapshot));
    }
    for minor in [1, 9_007_199_254_740_991] {
        let mut s = receipt();
        s.payment.amount_minor = minor;
        s.started_at = 1500;
        s.recorded_at = 9_007_199_254_740_991;
        assert_eq!(s.validate(), Ok(()));
    }
    let s = receipt();
    assert_eq!(s.validate(), Ok(()));
    assert_eq!(s.recorded_at, 5000);
    for status in [
        PaymentStatus::Open,
        PaymentStatus::Pending,
        PaymentStatus::Authorized,
        PaymentStatus::Paid,
        PaymentStatus::Canceled,
        PaymentStatus::Expired,
        PaymentStatus::Failed,
    ] {
        let mut s = receipt();
        s.payment.status = status;
        assert_eq!(s.validate(), Ok(()));
    }
}
#[test]
fn callback_types_reject_unknown_fields_and_live_currency_or_status_values() {
    let original = serde_json::to_value(receipt()).expect("fixture");
    for (path, value) in [
        ("/schema_version", serde_json::json!("1")),
        ("/payment/mode", serde_json::json!("live")),
        ("/payment/currency", serde_json::json!("USD")),
        ("/payment/status", serde_json::json!("unknown")),
    ] {
        let mut bad = original.clone();
        *bad.pointer_mut(path).expect("literal field") = value;
        assert!(serde_json::from_value::<LookupSnapshot>(bad).is_err());
    }
    let mut extra = original.clone();
    extra["unknown"] = true.into();
    assert!(serde_json::from_value::<LookupSnapshot>(extra).is_err());
    let mut extra = original.clone();
    extra["operation"]["unknown"] = true.into();
    assert!(serde_json::from_value::<LookupSnapshot>(extra).is_err());
    assert!(
        serde_json::from_value::<LookupSnapshot>(original).expect("healthy closed projection")
            == receipt()
    );
}
