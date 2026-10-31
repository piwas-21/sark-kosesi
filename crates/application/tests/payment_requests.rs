//! Synthetic server configuration; no actual merchant/profile/method/provider acceptance.
use commerce_application::payment_requests::*;
use commerce_domain::{Currency, payment_attempts::*};
fn policy() -> ConfigurationSnapshot {
    ConfigurationSnapshot {
        schema_version: 1,
        profile: ProfileBinding {
            id: "pfl_SYNTHETIC_NotARealProfile".into(),
            credential_fingerprint: "a".repeat(64),
            mode: PaymentMode::Test,
        },
        method: PaymentMethod::Ideal,
        merchant_origin: "https://shop.example.test".into(),
        return_url: "https://shop.example.test/payment-return".into(),
        webhook_url: "https://shop.example.test/api/payment-webhook".into(),
    }
}
fn binding(config: &Configuration) -> AttemptBinding {
    let current = config.identity();
    AttemptBinding {
        id: "1".repeat(32),
        order_id: "2".repeat(32),
        basket_id: "3".repeat(32),
        stock_group_id: "4".repeat(32),
        profile: current.profile,
        configuration_sha256: current.sha256,
        currency: Currency::EUR,
        amount_minor: 38245,
        created_at: 1100,
        dispatch_before: 2000,
    }
}
#[test]
fn frozen_request_has_literal_decimal_metadata_routes_and_no_profile_key_or_contact_data() {
    let config = Configuration::capture(policy()).expect("synthetic trusted policy");
    let frozen = FrozenAttempt::capture(binding(&config), &config).expect("freeze");
    let expected = include_str!("../../../tests/fixtures/payment_request.json").trim_end();
    assert_eq!(frozen.body(), expected);
    assert_eq!(
        frozen.snapshot().request_sha256,
        "64db7b14d1b2ef2a3fe5f7c5a1183f5b7f9da5419b10ae653c441e702ab8792d"
    );
    assert_eq!(frozen.snapshot().validate(), Ok(()));
    assert!(frozen.snapshot().binding.profile == policy().profile);
    let recovered = FrozenAttempt::restore(frozen.snapshot(), expected, &config)
        .expect("exact immutable restore");
    assert_eq!(recovered.body(), expected);
    assert!(recovered.snapshot() == frozen.snapshot());
    for forbidden in [
        "profileId",
        "testmode",
        "credential",
        "basket_id",
        "stock_group_id",
        "email",
        "billingAddress",
    ] {
        assert!(!frozen.body().contains(forbidden));
    }
    assert_eq!(
        config.identity().sha256,
        "af25a72d9b84e860ab9b8c237bcb509876391bdc9f3534cf682184af80f25701"
    );
    assert_eq!(
        config.fingerprint_material(),
        r#"["piwas-mollie-test-configuration-v1","pfl_SYNTHETIC_NotARealProfile","aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","test","ideal","https://shop.example.test","https://shop.example.test/payment-return","https://shop.example.test/api/payment-webhook"]"#
    );
    let config2 = Configuration::capture(policy()).expect("same exact policy");
    assert_eq!(config.identity().sha256, config2.identity().sha256);
    assert!(config.snapshot() == &policy());
}
#[test]
fn all_minor_units_are_exact_decimal_strings_without_float_rounding_or_recurring_effects() {
    let config = Configuration::capture(policy()).expect("policy");
    for (minor, expected) in [
        (1, "0.01"),
        (99, "0.99"),
        (100, "1.00"),
        (38245, "382.45"),
        (9_007_199_254_740_991, "90071992547409.91"),
    ] {
        let mut b = binding(&config);
        b.amount_minor = minor;
        let f = FrozenAttempt::capture(b, &config).expect("freeze");
        let value: serde_json::Value = serde_json::from_str(f.body()).expect("canonical JSON");
        assert_eq!(value["amount"]["value"], expected);
        assert_eq!(value["sequenceType"], "oneoff");
    }
    for method in [
        PaymentMethod::Ideal,
        PaymentMethod::Bancontact,
        PaymentMethod::Creditcard,
        PaymentMethod::Paypal,
    ] {
        let mut p = policy();
        p.method = method;
        let config = Configuration::capture(p).expect("supported syntax only");
        let f = FrozenAttempt::capture(binding(&config), &config).expect("freeze");
        let value: serde_json::Value = serde_json::from_str(f.body()).expect("JSON");
        let expected = match method {
            PaymentMethod::Ideal => "ideal",
            PaymentMethod::Bancontact => "bancontact",
            PaymentMethod::Creditcard => "creditcard",
            PaymentMethod::Paypal => "paypal",
        };
        assert_eq!(value["method"], expected);
    }
}
#[test]
fn configured_https_origin_and_distinct_absolute_routes_reject_ambiguous_or_external_targets() {
    let changes: [fn(&mut ConfigurationSnapshot); 21] = [
        |p| p.schema_version = 2,
        |p| p.profile.id = "pfl_".into(),
        |p| p.profile.credential_fingerprint = "bad".into(),
        |p| p.merchant_origin = "x".repeat(2049),
        |p| p.return_url = "x".repeat(2049),
        |p| p.webhook_url = "x".repeat(2049),
        |p| p.return_url = p.webhook_url.clone(),
        |p| p.merchant_origin = ":".into(),
        |p| p.merchant_origin = "http://shop.example.test".into(),
        |p| p.merchant_origin = "https://127.0.0.1".into(),
        |p| p.merchant_origin = "https://localhost".into(),
        |p| p.merchant_origin = "https://shop.example.test/".into(),
        |p| p.merchant_origin = "https://shop.example.test.".into(),
        |p| p.return_url = ":".into(),
        |p| p.return_url = "https://other.example.test/return".into(),
        |p| p.return_url = "https://name@shop.example.test/return".into(),
        |p| p.return_url = "https://:password@shop.example.test/return".into(),
        |p| p.return_url = "https://shop.example.test/return?token=private".into(),
        |p| p.return_url = "https://shop.example.test/return#fragment".into(),
        |p| p.return_url = "https://shop.example.test/a/../return".into(),
        |p| p.webhook_url = "https://other.example.test/webhook".into(),
    ];
    for change in changes {
        let mut p = policy();
        change(&mut p);
        assert!(matches!(
            Configuration::capture(p),
            Err(RequestError::InvalidConfiguration)
        ));
    }
    for origin in [
        "https://name@shop.example.test",
        "https://shop.example.test?",
        "https://shop.example.test#",
        "https://SHOP.example.test",
        "https://shop.example.test:443",
    ] {
        let mut p = policy();
        p.merchant_origin = origin.into();
        assert!(matches!(
            Configuration::capture(p),
            Err(RequestError::InvalidConfiguration)
        ));
    }
}
#[test]
fn changed_configuration_cannot_silently_change_a_prepared_attempt_or_frozen_body() {
    let config = Configuration::capture(policy()).expect("policy");
    let b = binding(&config);
    let frozen = FrozenAttempt::capture(b.clone(), &config).expect("freeze");
    for change in 0..5 {
        let mut p = policy();
        match change {
            0 => p.profile.credential_fingerprint = "b".repeat(64),
            1 => p.profile.id = "pfl_OTHER".into(),
            2 => p.return_url = "https://shop.example.test/different-return".into(),
            3 => p.webhook_url = "https://shop.example.test/different-hook".into(),
            _ => p.method = PaymentMethod::Bancontact,
        };
        let changed = Configuration::capture(p).expect("valid different policy");
        assert_ne!(config.identity().sha256, changed.identity().sha256);
        assert!(matches!(
            FrozenAttempt::capture(b.clone(), &changed),
            Err(RequestError::InvalidConfiguration)
        ));
        assert!(matches!(
            plan_dispatch(
                frozen.snapshot(),
                &DispatchState::Prepared,
                1200,
                &changed.identity()
            ),
            Err(PaymentError::ConfigurationChanged)
        ));
        assert!(matches!(
            plan_dispatch(
                frozen.snapshot(),
                &DispatchState::Started { started_at: 1200 },
                4800,
                &changed.identity()
            ),
            Ok(DispatchPlan::Reconcile { .. })
        ));
    }
    let mut invalid = b.clone();
    invalid.amount_minor = 0;
    assert!(matches!(
        FrozenAttempt::capture(invalid, &config),
        Err(RequestError::InvalidBinding)
    ));
}
#[test]
fn corrupt_or_changed_stored_bytes_fail_closed_and_positive_restore_keeps_original_request() {
    let config = Configuration::capture(policy()).expect("policy");
    let f = FrozenAttempt::capture(binding(&config), &config).expect("freeze");
    for body in [
        "[]".into(),
        "{}".into(),
        format!("{} ", f.body()),
        f.body().replace("382.45", "382.44"),
        "x".repeat(8193),
    ] {
        assert!(matches!(
            FrozenAttempt::restore(f.snapshot(), &body, &config),
            Err(RequestError::InvalidStoredRequest)
        ));
    }
    for change in 0..3 {
        let mut snapshot = f.snapshot().clone();
        match change {
            0 => snapshot.schema_version = 2,
            1 => snapshot.request_sha256 = "bad".into(),
            _ => snapshot.request_sha256 = "c".repeat(64),
        };
        assert!(matches!(
            FrozenAttempt::restore(&snapshot, f.body(), &config),
            Err(RequestError::InvalidStoredRequest)
        ));
    }
    assert_eq!(
        FrozenAttempt::restore(f.snapshot(), f.body(), &config)
            .expect("positive exact restore")
            .body(),
        f.body()
    );
}
#[test]
fn policy_serialization_requires_explicit_test_profile_method_and_endpoints() {
    let raw = serde_json::to_value(policy()).expect("policy");
    for pointer in ["/profile/mode", "/method"] {
        let mut v = raw.clone();
        *v.pointer_mut(pointer).expect("field") = if pointer.ends_with("mode") {
            "live".into()
        } else {
            "klarna".into()
        };
        assert!(serde_json::from_value::<ConfigurationSnapshot>(v).is_err());
    }
    for field in [
        "profile",
        "method",
        "merchant_origin",
        "return_url",
        "webhook_url",
    ] {
        let mut v = raw.clone();
        assert!(v.as_object_mut().expect("object").remove(field).is_some());
        assert!(serde_json::from_value::<ConfigurationSnapshot>(v).is_err());
    }
    let mut v = raw;
    v["role"] = "owner".into();
    assert!(serde_json::from_value::<ConfigurationSnapshot>(v).is_err());
}
