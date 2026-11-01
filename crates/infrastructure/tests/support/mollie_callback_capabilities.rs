//! Internal capability defenses: these constructors remain inaccessible to callers.
use super::*;
use commerce_domain::{
    Currency,
    payment_attempts::{
        PaymentMode, PaymentStatus,
        callbacks::{LookupOperationSnapshot, LookupSnapshot},
    },
};
fn saved() -> LookupSnapshot {
    LookupSnapshot {
        schema_version: 1,
        id: "b".repeat(32),
        operation: LookupOperationSnapshot {
            schema_version: 1,
            id: "a".repeat(32),
            profile_id: "pfl_SYNTHETIC_Capability".into(),
            provider_id: "tr_SYNTHETIC_Capability".into(),
            created_at: 1500,
        },
        started_at: 1501,
        recorded_at: 1502,
        payment: ProviderPayment {
            resource: "payment".into(),
            id: "tr_SYNTHETIC_Capability".into(),
            profile_id: "pfl_SYNTHETIC_Capability".into(),
            mode: PaymentMode::Test,
            currency: Currency::EUR,
            amount_minor: 38245,
            metadata_schema_version: 1,
            order_id: "2".repeat(32),
            attempt_id: "1".repeat(32),
            status: PaymentStatus::Paid,
        },
        credential_fingerprint: "c".repeat(64),
    }
}
#[test]
fn bound_lookup_capability_rejects_other_operation_start_payment_or_fetched_key() {
    let original = saved();
    let bound = CallbackLookupPayment {
        operation: original.operation.clone(),
        fetched: CallbackPayment::from_custody(&original).expect("healthy minimal custody"),
        started_at: original.started_at,
    };
    assert!(bound.matches(&original), "exact original fact replays");
    for field in ["operation", "start", "payment", "key"] {
        let mut altered = original.clone();
        match field {
            "operation" => altered.operation.id = "d".repeat(32),
            "start" => altered.started_at = 1500,
            "payment" => altered.payment.status = PaymentStatus::Open,
            "key" => altered.credential_fingerprint = "e".repeat(64),
            _ => unreachable!("literal fixture cases"),
        }
        altered
            .validate()
            .expect("different but independently valid fact");
        assert!(
            !bound.matches(&altered),
            "changed {field} cannot replay old fact"
        );
        assert!(
            bound.matches(&original),
            "unchanged original remains replayable"
        );
    }
}
#[test]
fn custody_reconstruction_revalidates_projection_before_restoring_opaque_evidence() {
    let original = saved();
    let restored = CallbackPayment::from_custody(&original).expect("healthy minimal custody");
    assert_eq!(restored.payment_id(), "tr_SYNTHETIC_Capability");
    assert_eq!(restored.attempt_id_hint(), "1".repeat(32));
    assert_eq!(restored.credential_fingerprint, "c".repeat(64));
    let mut malformed = original.clone();
    malformed.payment.metadata_schema_version = 2;
    assert!(matches!(
        CallbackPayment::from_custody(&malformed),
        Err(MollieError::PaymentMismatch)
    ));
    assert!(
        CallbackPayment::from_custody(&original).is_ok(),
        "exact repair restores custody"
    );
}
