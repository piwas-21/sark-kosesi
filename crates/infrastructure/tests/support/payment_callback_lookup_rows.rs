//! Closed stored-decoder controls also cover unreachable hostile query headers.
use super::*;
use commerce_domain::{
    Currency,
    payment_attempts::{PaymentMode, PaymentStatus, ProviderPayment},
};
use serde_json::json;
fn operation() -> LookupOperationSnapshot {
    LookupOperationSnapshot {
        schema_version: 1,
        id: "1".repeat(32),
        profile_id: "pfl_SYNTHETIC".into(),
        provider_id: "tr_SYNTHETIC".into(),
        created_at: 1500,
    }
}
fn snapshot() -> LookupSnapshot {
    LookupSnapshot {
        schema_version: 1,
        id: "2".repeat(32),
        operation: operation(),
        started_at: 1600,
        recorded_at: 1700,
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
fn row() -> ReceiptRow {
    ReceiptRow {
        id: "2".repeat(32),
        operation_id: "1".repeat(32),
        recorded_at: 1700,
        snapshot: Some(serde_json::to_value(snapshot()).expect("closed fixture")),
        event_present: true,
    }
}
fn operation_row() -> OperationRow {
    OperationRow {
        id: "1".repeat(32),
        profile_id: "pfl_SYNTHETIC".into(),
        provider_id: "tr_SYNTHETIC".into(),
        created_at: 1500,
    }
}
#[test]
fn operation_decoder_binds_requested_identity_and_revalidates_every_imported_header() {
    let original = operation();
    assert!(operation_row().decode(&original.id).expect("healthy row") == original);
    assert_eq!(
        operation_row().decode(&"f".repeat(32)).err(),
        Some(PaymentStoreError::InvalidStoredData)
    );
    let changes: [fn(&mut OperationRow); 4] = [
        |r| r.id = "bad".into(),
        |r| r.profile_id = "pfl_".into(),
        |r| r.provider_id = "tr_".into(),
        |r| r.created_at = -1,
    ];
    for change in changes {
        let mut row = operation_row();
        change(&mut row);
        assert_eq!(
            row.decode(&original.id).err(),
            Some(PaymentStoreError::InvalidStoredData)
        );
    }
}
#[test]
fn receipt_decoder_rejects_root_positional_forms_and_each_mismatched_header() {
    let changes: [fn(&mut ReceiptRow); 7] = [
        |r| r.id = "3".repeat(32),
        |r| r.operation_id = "3".repeat(32),
        |r| r.recorded_at = 1701,
        |r| r.snapshot = None,
        |r| r.event_present = false,
        |r| {
            r.snapshot = Some(json!([
                1,
                "2".repeat(32),
                operation(),
                1600,
                1700,
                snapshot().payment,
                "a".repeat(64)
            ]))
        },
        |r| r.snapshot.as_mut().expect("fixture")["operation"]["profile_id"] = "pfl_OTHER".into(),
    ];
    for change in changes {
        let mut row = row();
        change(&mut row);
        assert_eq!(
            row.decode(&operation()).err(),
            Some(PaymentStoreError::InvalidStoredData)
        );
    }
    assert!(
        row()
            .decode(&operation())
            .expect("paired exact closed row")
            .snapshot
            == snapshot()
    );
}
