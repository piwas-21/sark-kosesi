//! Internal matching guards cannot be bypassed by independently valid projections.
use super::*;
use commerce_domain::{
    Currency,
    payment_attempts::{
        PaymentMode, PaymentStatus, ProviderPayment,
        callbacks::{LookupOperationSnapshot, LookupSnapshot},
        identities::IdentitySummary,
        observations::{ObservationIntent, ObservationSnapshot},
    },
};
fn proofs() -> (
    ProcessingRow,
    LookupSnapshot,
    ObservationSnapshot,
    identities::Resolution,
) {
    let payment = ProviderPayment {
        resource: "payment".into(),
        id: "tr_PROOF".into(),
        profile_id: "pfl_PROOF".into(),
        mode: PaymentMode::Test,
        currency: Currency::EUR,
        amount_minor: 38245,
        metadata_schema_version: 1,
        order_id: "a".repeat(32),
        attempt_id: "b".repeat(32),
        status: PaymentStatus::Open,
    };
    let receipt = LookupSnapshot {
        schema_version: 1,
        id: "c".repeat(32),
        operation: LookupOperationSnapshot {
            schema_version: 1,
            id: "d".repeat(32),
            profile_id: "pfl_PROOF".into(),
            provider_id: "tr_PROOF".into(),
            created_at: 1500,
        },
        started_at: 1500,
        recorded_at: 1501,
        payment: payment.clone(),
        credential_fingerprint: "e".repeat(64),
    };
    let observation = ObservationSnapshot {
        schema_version: 1,
        id: "f".repeat(32),
        attempt_id: "b".repeat(32),
        effect_key: format!("callback-lookup-{}", "d".repeat(32)),
        revision: 2,
        recorded_at: 1502,
        intent: ObservationIntent {
            payment,
            credential_fingerprint: "e".repeat(64),
        },
    };
    let resolution = identities::Resolution {
        id: "1".repeat(32),
        attempt_id: "b".repeat(32),
        effect_key: format!("callback-inbox-{}", "d".repeat(32)),
        revision: 1,
        through_observation: 2,
        recorded_at: 1503,
        summary: IdentitySummary {
            provider_ids: vec!["tr_PROOF".into()],
            paid_provider_ids: vec![],
        },
    };
    let row = ProcessingRow {
        event_id: Some("2".repeat(32)),
        processed_at: 1504,
        outcome: "observed".into(),
        observation_id: Some(observation.id.clone()),
        resolution_id: Some(resolution.id.clone()),
    };
    (row, receipt, observation, resolution)
}
#[test]
fn financial_proof_matching_rejects_each_identity_money_time_prefix_and_outcome_drift() {
    for field in [
        "effect",
        "payment",
        "fingerprint",
        "observation_time",
        "resolution_time",
        "resolution_identity",
        "prefix",
        "processing_time",
        "classification",
    ] {
        let (mut row, receipt, mut observation, mut resolution) = proofs();
        let op = receipt.operation.id.clone();
        assert!(
            row.validate_financial(
                &op,
                &receipt,
                &observation,
                &resolution,
                Disposition::Observed
            )
            .is_ok()
        );
        match field {
            "effect" => observation.effect_key = "another-operation".into(),
            "payment" => observation.intent.payment.amount_minor = 38244,
            "fingerprint" => observation.intent.credential_fingerprint = "a".repeat(64),
            "observation_time" => observation.recorded_at = 1500,
            "resolution_time" => resolution.recorded_at = 1500,
            "resolution_identity" => row.resolution_id = Some("a".repeat(32)),
            "prefix" => resolution.through_observation = 1,
            "processing_time" => row.processed_at = 1502,
            "classification" => resolution.summary.paid_provider_ids = vec!["tr_PROOF".into()],
            _ => unreachable!("literal fixture cases"),
        }
        assert!(
            matches!(
                row.validate_financial(
                    &op,
                    &receipt,
                    &observation,
                    &resolution,
                    Disposition::Observed
                ),
                Err(PaymentStoreError::InvalidStoredData)
            ),
            "{field}"
        );
        let (healthy, receipt, observation, resolution) = proofs();
        assert!(
            healthy
                .validate_financial(
                    &op,
                    &receipt,
                    &observation,
                    &resolution,
                    Disposition::Observed
                )
                .is_ok(),
            "healthy exact restoration {field}"
        );
    }
}
#[test]
fn processing_outcome_decoder_is_closed_and_classification_retains_ambiguity() {
    for raw in [
        "observed",
        "paid_observed",
        "ambiguous",
        "unmatched",
        "custody_invalid",
    ] {
        assert!(Disposition::parse(raw).is_ok());
    }
    for raw in ["", "paid", "unknown", "Observed"] {
        assert!(matches!(
            Disposition::parse(raw),
            Err(PaymentStoreError::InvalidStoredData)
        ));
    }
    let (_, _, _, mut resolution) = proofs();
    resolution.summary.provider_ids.push("tr_OTHER".into());
    resolution.summary.paid_provider_ids.push("tr_PROOF".into());
    assert!(classify(&resolution) == Disposition::Ambiguous);
}

// Test-only access to the real private write boundary. The supplied operation is
// backed by an actual authenticated loopback receipt in the caller's fixture.
// This never restores a payment capability and cannot record financial refs.
pub(crate) async fn persist_clock_probe(
    pool: &PgPool,
    clock: &dyn Clock,
    operation: &str,
    recorded_at: Option<i64>,
) -> Result<Disposition, PaymentStoreError> {
    persist(
        pool,
        clock,
        operation,
        Disposition::CustodyInvalid,
        None,
        None,
        recorded_at,
    )
    .await
}
