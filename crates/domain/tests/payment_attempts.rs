//! Invented source oracles; no provider, account, charge or merchant acceptance.
use commerce_domain::{Currency, payment_attempts::*};
fn attempt() -> AttemptSnapshot {
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
fn configuration(fingerprint: &str) -> CurrentConfiguration {
    CurrentConfiguration {
        profile: ProfileBinding {
            id: "pfl_SYNTHETIC_NotARealProfile".into(),
            credential_fingerprint: fingerprint.into(),
            mode: PaymentMode::Test,
        },
        sha256: "d".repeat(64),
    }
}
fn provider() -> ProviderPayment {
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
#[test]
fn new_attempt_can_plan_one_durable_start_with_a_literal_stable_key() {
    let a = attempt();
    assert_eq!(a.validate(), Ok(()));
    let state = DispatchState::Prepared;
    let plan = plan_dispatch(&a, &state, 1200, &configuration(&"a".repeat(64)))
        .expect("valid unsent attempt");
    let DispatchPlan::Start {
        next,
        idempotency_key,
    } = plan
    else {
        panic!("new attempt must request a persisted start");
    };
    assert_eq!(
        idempotency_key,
        "piwas-test-payment-11111111111111111111111111111111"
    );
    assert!(next == DispatchState::Started { started_at: 1200 });
    for at in [1200, 1999, 2000, 4800, 10000] {
        for credential in ["a".repeat(64), "c".repeat(64)] {
            assert!(matches!(
                plan_dispatch(&a, &next, at, &configuration(&credential)),
                Ok(DispatchPlan::Reconcile { provider_id: None })
            ));
        }
    }
    // Planning is pure: an infrastructure CAS must persist the returned start before I/O.
    assert!(state == DispatchState::Prepared);
}
#[test]
fn dispatch_deadlines_clock_and_configuration_never_authorize_a_late_or_changed_post() {
    let a = attempt();
    for at in [1100, 1999] {
        assert!(matches!(
            plan_dispatch(
                &a,
                &DispatchState::Prepared,
                at,
                &configuration(&"a".repeat(64))
            ),
            Ok(DispatchPlan::Start { .. })
        ));
    }
    for (at, want) in [
        (1099, PaymentError::InvalidClock),
        (2000, PaymentError::Expired),
        (9_007_199_254_740_992, PaymentError::InvalidClock),
    ] {
        assert!(
            matches!(plan_dispatch(&a,&DispatchState::Prepared,at,&configuration(&"a".repeat(64))),Err(got) if got==want)
        );
    }
    let mut changed = configuration(&"a".repeat(64));
    changed.sha256 = "e".repeat(64);
    assert!(matches!(
        plan_dispatch(&a, &DispatchState::Prepared, 1200, &changed),
        Err(PaymentError::ConfigurationChanged)
    ));
    assert!(matches!(
        plan_dispatch(
            &a,
            &DispatchState::Started { started_at: 1200 },
            4800,
            &changed
        ),
        Ok(DispatchPlan::Reconcile { .. })
    ));
    assert!(matches!(
        plan_dispatch(
            &a,
            &DispatchState::Prepared,
            1200,
            &configuration(&"c".repeat(64))
        ),
        Err(PaymentError::ConfigurationChanged)
    ));
    assert!(matches!(
        plan_dispatch(
            &a,
            &DispatchState::Started { started_at: 1200 },
            1199,
            &configuration(&"a".repeat(64))
        ),
        Err(PaymentError::InvalidClock)
    ));
    for started_at in [1099, 2000] {
        assert_eq!(
            DispatchState::Started { started_at }.validate(&a),
            Err(PaymentError::InvalidSnapshot)
        );
    }
    let mut a = attempt();
    a.binding.amount_minor = 0;
    assert!(matches!(
        plan_dispatch(
            &a,
            &DispatchState::Prepared,
            1200,
            &configuration(&"a".repeat(64))
        ),
        Err(PaymentError::InvalidSnapshot)
    ));
}
#[test]
fn known_provider_identity_is_immutable_and_never_authorizes_another_post() {
    let a = attempt();
    let id = "tr_SYNTHETIC_NotARealPayment";
    assert!(matches!(
        bind_provider(&a, &DispatchState::Prepared, id),
        Err(PaymentError::NotDispatched)
    ));
    let started = DispatchState::Started { started_at: 1200 };
    let known = bind_provider(&a, &started, id).expect("captured identity");
    assert!(
        known
            == DispatchState::Known {
                started_at: 1200,
                provider_id: id.into()
            }
    );
    assert!(bind_provider(&a, &known, id).expect("exact repeat") == known);
    assert!(matches!(
        bind_provider(&a, &known, "tr_DIFFERENT"),
        Err(PaymentError::IdentityConflict)
    ));
    assert!(matches!(
        bind_provider(&a, &started, "not-a-payment"),
        Err(PaymentError::ProviderMismatch)
    ));
    assert!(
        matches!(plan_dispatch(&a,&known,10000,&configuration(&"c".repeat(64))),Ok(DispatchPlan::Reconcile{provider_id:Some(value)}) if value==id)
    );
    assert!(matches!(
        plan_dispatch(&a, &known, 1199, &configuration(&"a".repeat(64))),
        Err(PaymentError::InvalidClock)
    ));
    assert_eq!(
        DispatchState::Known {
            started_at: 1200,
            provider_id: "tr_".into()
        }
        .validate(&a),
        Err(PaymentError::InvalidSnapshot)
    );
}
#[test]
fn stable_snapshot_requires_exact_bounded_identity_positive_eur_and_source_digest() {
    let changes: [fn(&mut AttemptSnapshot); 18] = [
        |a| a.schema_version = 2,
        |a| a.binding.id = "bad".into(),
        |a| a.binding.order_id = "g".repeat(32),
        |a| a.binding.basket_id = "".into(),
        |a| a.binding.stock_group_id = "5".repeat(33),
        |a| a.binding.profile.id = "pfl_".into(),
        |a| a.binding.profile.id = " pfl_x".into(),
        |a| a.binding.profile.id = "pfl_bad\nvalue".into(),
        |a| a.binding.profile.id = format!("pfl_{}", "x".repeat(253)),
        |a| a.binding.configuration_sha256 = "invalid".into(),
        |a| a.binding.profile.id = "pfl_SYNTHETIC ".into(),
        |a| a.binding.profile.credential_fingerprint = "G".repeat(64),
        |a| a.request_sha256 = "".into(),
        |a| a.binding.amount_minor = 0,
        |a| a.binding.amount_minor = 9_007_199_254_740_992,
        |a| a.binding.created_at = -1,
        |a| a.binding.dispatch_before = 1100,
        |a| a.binding.dispatch_before = 2901,
    ];
    for change in changes {
        let mut a = attempt();
        change(&mut a);
        assert_eq!(a.validate(), Err(PaymentError::InvalidSnapshot));
    }
    let mut a = attempt();
    a.binding.created_at = 9_007_199_254_740_990;
    a.binding.dispatch_before = 9_007_199_254_740_992;
    assert_eq!(a.validate(), Err(PaymentError::InvalidSnapshot));
    let mut a = attempt();
    a.binding.amount_minor = 9_007_199_254_740_991;
    a.binding.profile.id = format!("pfl_{}", "x".repeat(252));
    assert_eq!(a.validate(), Ok(()));
}
#[test]
fn matching_requires_exact_fetched_amount_profile_and_metadata_even_for_late_paid() {
    let a = attempt();
    let p = provider();
    assert_eq!(match_provider(&a, &p), Ok(()));
    assert_eq!(p.amount_minor, 38245);
    for status in [
        PaymentStatus::Open,
        PaymentStatus::Pending,
        PaymentStatus::Authorized,
        PaymentStatus::Paid,
        PaymentStatus::Canceled,
        PaymentStatus::Expired,
        PaymentStatus::Failed,
    ] {
        let mut p = provider();
        p.status = status;
        assert_eq!(match_provider(&a, &p), Ok(()));
    }
    let changes: [fn(&mut ProviderPayment); 9] = [
        |p| p.resource = "order".into(),
        |p| p.id = "tr_".into(),
        |p| p.id = format!("tr_{}", "x".repeat(254)),
        |p| p.id = "tr_bad\r\nid".into(),
        |p| p.profile_id = "pfl_OTHER".into(),
        |p| p.amount_minor = 38244,
        |p| p.metadata_schema_version = 2,
        |p| p.order_id = "9".repeat(32),
        |p| p.attempt_id = "8".repeat(32),
    ];
    for change in changes {
        let mut p = provider();
        change(&mut p);
        assert_eq!(match_provider(&a, &p), Err(PaymentError::ProviderMismatch));
    }
    let mut a = attempt();
    a.request_sha256 = "bad".into();
    assert_eq!(match_provider(&a, &p), Err(PaymentError::InvalidSnapshot));
    let mut p = provider();
    p.id = format!("tr_{}", "x".repeat(253));
    assert_eq!(match_provider(&attempt(), &p), Ok(()));
    // No current deadline is accepted by matching: late-paid evidence remains available
    // for explicit stock/finance recovery; matching does not fulfil or consume a hold.
    assert_eq!(match_provider(&attempt(), &provider()), Ok(()));
}
#[test]
fn stored_contract_has_no_missing_mode_currency_or_unknown_status_fallback() {
    let a = attempt();
    let raw = serde_json::to_value(&a).expect("snapshot");
    assert!(serde_json::from_value::<AttemptSnapshot>(raw.clone()).expect("round trip") == a);
    for (pointer, value) in [
        ("/binding/profile/mode", "live"),
        ("/binding/currency", "USD"),
    ] {
        let mut raw = raw.clone();
        *raw.pointer_mut(pointer).expect("required contract field") = value.into();
        assert!(serde_json::from_value::<AttemptSnapshot>(raw).is_err());
    }
    let mut unknown = raw.clone();
    unknown["role"] = "owner".into();
    assert!(serde_json::from_value::<AttemptSnapshot>(unknown).is_err());
    for (parent, field) in [
        ("/binding/profile", "mode"),
        ("/binding/profile", "credential_fingerprint"),
        ("/binding", "currency"),
        ("/binding", "configuration_sha256"),
        ("", "request_sha256"),
    ] {
        let mut raw = raw.clone();
        assert!(
            raw.pointer_mut(parent)
                .expect("parent")
                .as_object_mut()
                .expect("object")
                .remove(field)
                .is_some()
        );
        assert!(serde_json::from_value::<AttemptSnapshot>(raw).is_err());
    }
    let raw = serde_json::to_value(provider()).expect("provider");
    for (field, value) in [
        ("mode", "live"),
        ("currency", "USD"),
        ("status", "settled"),
        ("role", "owner"),
    ] {
        let mut raw = raw.clone();
        raw[field] = value.into();
        assert!(serde_json::from_value::<ProviderPayment>(raw).is_err());
    }
    let raw = serde_json::to_value(DispatchState::Started { started_at: 1200 }).expect("state");
    let round: DispatchState = serde_json::from_value(raw).expect("state");
    assert!(round == DispatchState::Started { started_at: 1200 });
}

#[test]
fn payment_preparation_intent_is_explicit_closed_and_contains_no_customer_total() {
    let intent = PreparationIntent {
        order_id: "2".repeat(32),
        confirmed: true,
    };
    assert_eq!(intent.validate(), Ok(()));
    for v in [
        PreparationIntent {
            order_id: "bad".into(),
            confirmed: true,
        },
        PreparationIntent {
            order_id: "2".repeat(32),
            confirmed: false,
        },
    ] {
        assert_eq!(v.validate(), Err(PaymentError::InvalidRequest));
    }
    let mut value = serde_json::to_value(&intent).expect("intent");
    value["amount_minor"] = serde_json::json!(1);
    assert!(serde_json::from_value::<PreparationIntent>(value).is_err());
    let mut value = serde_json::to_value(&intent).expect("intent");
    value.as_object_mut().expect("object").remove("confirmed");
    assert!(serde_json::from_value::<PreparationIntent>(value).is_err());
}
