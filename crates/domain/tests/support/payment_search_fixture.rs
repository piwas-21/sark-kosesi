//! Literal synthetic discovery history; no provider authority.
use commerce_domain::{
    Currency,
    payment_attempts::{searches::*, *},
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
pub fn first() -> SearchSnapshot {
    SearchSnapshot {
        schema_version: 1,
        id: "9".repeat(32),
        attempt_id: "1".repeat(32),
        effect_key: "list-a".into(),
        revision: 1,
        page_number: 1,
        recorded_at: 5000,
        intent: SearchIntent {
            previous_page_id: None,
            from: None,
            next: Some("tr_A".into()),
            candidates: vec!["tr_ONE".into(), "tr_TWO".into()],
            credential_fingerprint: "c".repeat(64),
        },
    }
}
pub fn second() -> SearchSnapshot {
    SearchSnapshot {
        schema_version: 1,
        id: "8".repeat(32),
        attempt_id: "1".repeat(32),
        effect_key: "list-b".into(),
        revision: 2,
        page_number: 2,
        recorded_at: 5100,
        intent: SearchIntent {
            previous_page_id: Some("9".repeat(32)),
            from: Some("tr_A".into()),
            next: None,
            candidates: vec![],
            credential_fingerprint: "d".repeat(64),
        },
    }
}
