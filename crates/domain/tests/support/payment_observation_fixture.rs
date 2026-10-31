//! Literal synthetic observation facts. No actual order, key or provider acceptance.
use commerce_domain::{
    Currency,
    payment_attempts::{observations::*, *},
};
pub fn attempt() -> AttemptSnapshot {
    AttemptSnapshot {
        schema_version: 1,
        request_sha256: "b".repeat(64),
        binding: AttemptBinding {
            id: "1".repeat(32),
            order_id: "2".repeat(32),
            basket_id: "3".repeat(32),
            stock_group_id: "4".repeat(32),
            profile: ProfileBinding {
                id: "pfl_SYNTHETIC_NotARealProfile".into(),
                credential_fingerprint: "a".repeat(64),
                mode: PaymentMode::Test,
            },
            configuration_sha256: "d".repeat(64),
            currency: Currency::EUR,
            amount_minor: 38245,
            created_at: 1100,
            dispatch_before: 2000,
        },
    }
}
pub fn payment() -> ProviderPayment {
    ProviderPayment {
        resource: "payment".into(),
        id: "tr_SYNTHETIC_NotARealPayment".into(),
        profile_id: "pfl_SYNTHETIC_NotARealProfile".into(),
        mode: PaymentMode::Test,
        currency: Currency::EUR,
        amount_minor: 38245,
        metadata_schema_version: 1,
        order_id: "2".repeat(32),
        attempt_id: "1".repeat(32),
        status: PaymentStatus::Paid,
    }
}
pub fn observation() -> ObservationSnapshot {
    ObservationSnapshot {
        schema_version: 1,
        id: "9".repeat(32),
        attempt_id: "1".repeat(32),
        effect_key: "fetch-a".into(),
        revision: 1,
        recorded_at: 5000,
        intent: ObservationIntent {
            payment: payment(),
            credential_fingerprint: "c".repeat(64),
        },
    }
}
