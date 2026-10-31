//! Synthetic isolated credential/wire/HTTP oracles; no actual merchant/provider evidence.
use super::*;
use commerce_application::payment_requests::FrozenAttempt;
use commerce_domain::{
    Currency,
    payment_attempts::{AttemptBinding, PaymentStatus},
};
use serde_json::{Value, json};
use std::time::Duration;
#[path = "mollie_server.rs"]
mod server;
use server::{Reply, Server};
fn raw_key() -> String {
    format!("test_{}", "synthetic".repeat(4))
}
fn credential() -> TestCredential {
    TestCredential::capture(&raw_key(), "pfl_SYNTHETIC_NotARealProfile")
        .expect("synthetic credential")
}
fn config(key: &TestCredential) -> Configuration {
    let mut snapshot = crate::payment_test_fixture::config().snapshot().clone();
    snapshot.profile = key.binding().clone();
    Configuration::capture(snapshot).expect("synthetic server policy")
}
fn captured(amount_minor: i64) -> (Configuration, FrozenAttempt) {
    let key = credential();
    let c = config(&key);
    let current = c.identity();
    let f = FrozenAttempt::capture(
        AttemptBinding {
            id: "1".repeat(32),
            order_id: "2".repeat(32),
            basket_id: "3".repeat(32),
            stock_group_id: "4".repeat(32),
            profile: current.profile,
            configuration_sha256: current.sha256,
            currency: Currency::EUR,
            amount_minor,
            created_at: 1500,
            dispatch_before: 2000,
        },
        &c,
    )
    .expect("captured synthetic payment");
    (c, f)
}
fn payment(attempt: &AttemptSnapshot) -> Value {
    json!({"resource":"payment","id":"tr_SYNTHETIC_NotARealPayment","profileId":"pfl_SYNTHETIC_NotARealProfile","mode":"test","amount":{"currency":"EUR","value":"382.45"},"status":"paid","sequenceType":"oneoff","metadata":{"schema_version":1,"order_id":attempt.binding.order_id,"attempt_id":attempt.binding.id},"_links":{"checkout":{"href":"https://www.mollie.com/payscreen/select-method/synthetic"}}})
}
fn profile() -> Reply {
    Reply::Json(
        200,
        json!({"resource":"profile","id":"pfl_SYNTHETIC_NotARealProfile"}),
    )
}
fn local(server: &Server) -> TestClient {
    let key = credential();
    let c = config(&key);
    TestClient::local(key, c, server.base.clone(), Duration::from_secs(2))
        .expect("loopback only client")
}
#[test]
fn test_credentials_have_explicit_profile_fingerprint_and_sensitive_header_without_debug_data() {
    let key = credential();
    assert!(key.authorization.is_sensitive());
    assert!(!format!("{:?}", key.authorization).contains(&raw_key()));
    assert!(format!("Bearer {}", raw_key()).contains(&raw_key()));
    assert_eq!(
        key.binding().credential_fingerprint,
        "78a0e854453c8e82b9c97187636308791541cdbbcf28965b47a6ee3e33af97ef"
    );
    assert_eq!(key.binding().id, "pfl_SYNTHETIC_NotARealProfile");
    let client =
        TestClient::new(key, config(&credential())).expect("fixed HTTPS constructor does no I/O");
    assert_eq!(
        client.configuration().snapshot().merchant_origin,
        "https://shop.example.test"
    );
    for bad in [
        "".into(),
        "live_abcdefghijklmnopqrstuv".into(),
        "test_short".into(),
        format!("test_{}", "x".repeat(129)),
        format!("test_{}\n", "x".repeat(30)),
        format!("test_{}-", "x".repeat(30)),
    ] {
        assert!(matches!(
            TestCredential::capture(&bad, "pfl_SYNTHETIC"),
            Err(MollieError::InvalidCredential)
        ));
    }
    for id in ["", "pfl_", "pfl_\n", &format!("pfl_{}", "x".repeat(253))] {
        assert!(TestCredential::capture(&raw_key(), id).is_err());
    }
}
#[test]
fn payment_projection_uses_literal_decimal_amounts_and_preserves_late_paid_without_optional_links()
{
    for (minor, value) in [
        (1, "0.01"),
        (99, "0.99"),
        (100, "1.00"),
        (38245, "382.45"),
        (9_007_199_254_740_991, "90071992547409.91"),
    ] {
        let (_, f) = captured(minor);
        let mut v = payment(f.snapshot());
        v["amount"]["value"] = value.into();
        let (p, link) = wire::payment(
            &serde_json::to_vec(&v).expect("JSON"),
            f.snapshot(),
            Some("tr_SYNTHETIC_NotARealPayment"),
        )
        .expect("exact server projection");
        assert_eq!(p.amount_minor, minor);
        assert!(matches!(p.status, PaymentStatus::Paid));
        assert!(link.is_some());
    }
    let (_, f) = captured(38245);
    for links in [
        Value::Null,
        json!([]),
        json!({}),
        json!({"checkout":[]}),
        json!({"checkout":{"href":3}}),
        json!({"checkout":{"href":"https://attacker.example.test"}}),
    ] {
        let mut v = payment(f.snapshot());
        v["_links"] = links;
        let (p, link) = wire::payment(&serde_json::to_vec(&v).expect("JSON"), f.snapshot(), None)
            .expect("irrelevant malformed link cannot discard funds");
        assert!(matches!(p.status, PaymentStatus::Paid));
        assert!(link.is_none());
    }
}
#[test]
fn required_payment_objects_money_mode_profile_metadata_and_oneoff_sequence_fail_closed() {
    let (_, f) = captured(38245);
    let original = payment(f.snapshot());
    for v in [json!([]), json!({}), json!(null)] {
        assert!(wire::payment(&serde_json::to_vec(&v).expect("JSON"), f.snapshot(), None).is_err());
    }
    for pointer in ["/amount", "/metadata"] {
        let mut v = original.clone();
        *v.pointer_mut(pointer).expect("critical object") = json!([]);
        assert!(wire::payment(&serde_json::to_vec(&v).expect("JSON"), f.snapshot(), None).is_err());
    }
    for (pointer, value) in [
        ("/resource", json!("refund")),
        ("/id", json!("invalid")),
        ("/profileId", json!("pfl_OTHER")),
        ("/mode", json!("live")),
        ("/amount/currency", json!("USD")),
        ("/status", json!("unknown")),
        ("/metadata/order_id", json!("f".repeat(32))),
        ("/metadata/attempt_id", json!("f".repeat(32))),
        ("/metadata/schema_version", json!(2)),
        ("/sequenceType", json!("recurring")),
        ("/amount/value", json!(382.45)),
    ] {
        let mut v = original.clone();
        *v.pointer_mut(pointer).expect("field") = value;
        assert!(wire::payment(&serde_json::to_vec(&v).expect("JSON"), f.snapshot(), None).is_err());
    }
    let mut extra = original.clone();
    extra["metadata"]["role"] = "owner".into();
    assert!(
        wire::payment(
            &serde_json::to_vec(&extra).expect("JSON"),
            f.snapshot(),
            None
        )
        .is_err()
    );
    for invalid in [
        "",
        "382",
        ".45",
        "00.01",
        "-1.00",
        "1.0",
        "1.000",
        "1.a0",
        "a.00",
        "1..00",
        "90071992547409.92",
        "9223372036854775807.00",
        "92233720368547758.99",
        "9999999999999999999999999999999999.00",
    ] {
        let mut v = original.clone();
        v["amount"]["value"] = invalid.into();
        assert!(
            wire::payment(&serde_json::to_vec(&v).expect("JSON"), f.snapshot(), None).is_err(),
            "reject {invalid}"
        );
    }
    assert!(wire::payment(b"not JSON", f.snapshot(), None).is_err());
    assert!(wire::payment(&vec![b'x'; 65537], f.snapshot(), None).is_err());
    assert!(
        wire::payment(
            &serde_json::to_vec(&original).expect("JSON"),
            f.snapshot(),
            Some("tr_OTHER")
        )
        .is_err()
    );
}
#[tokio::test]
async fn expected_profile_is_verified_before_any_payment_operation_and_redirects_are_not_followed()
{
    let server = Server::start(vec![profile()]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("verified test profile");
    assert_eq!(
        client.configuration().snapshot().profile.id,
        "pfl_SYNTHETIC_NotARealProfile"
    );
    assert_eq!(server.count(), 1);
    server.inspect(|requests| {
        assert_eq!(requests[0].method, "GET");
        assert_eq!(requests[0].path, "/v2/profiles/me");
        assert_eq!(
            requests[0].header("Authorization"),
            Some(format!("Bearer {}", raw_key()).as_str())
        );
    });
    for body in [
        json!({"resource":"profile","id":"pfl_OTHER"}),
        json!({"resource":"payment","id":"pfl_SYNTHETIC_NotARealProfile"}),
        json!([]),
    ] {
        let server = Server::start(vec![Reply::Json(200, body)]);
        assert!(local(&server).verify_profile().await.is_err());
        assert_eq!(server.count(), 1);
    }
    let target = Server::start(vec![profile()]);
    let server = Server::start(vec![Reply::Redirect(
        target
            .base
            .join("profiles/me")
            .expect("loopback target")
            .to_string(),
    )]);
    assert!(local(&server).verify_profile().await.is_err());
    assert_eq!(server.count(), 1);
    assert_eq!(target.count(), 0);
}
#[tokio::test]
async fn provider_http_status_framing_timeout_and_body_budgets_are_bounded_and_redacted() {
    let marker = "synthetic-private-provider-detail";
    for reply in [
        Reply::Json(401, json!({"detail":marker})),
        Reply::Raw {
            status: 200,
            body: "not JSON".into(),
            mime: "application/json",
            chunked: false,
        },
        Reply::Raw {
            status: 200,
            body: "{}".into(),
            mime: "text/html",
            chunked: false,
        },
        Reply::Close,
    ] {
        let server = Server::start(vec![reply]);
        let result = local(&server).verify_profile().await;
        let Err(error) = result else {
            panic!("invalid provider response")
        };
        assert!(!format!("{error:?}").contains(marker));
        assert_eq!(server.count(), 1);
    }
    for chunked in [false, true] {
        let server = Server::start(vec![Reply::Raw {
            status: 200,
            body: "x".repeat(65537),
            mime: "application/json",
            chunked,
        }]);
        assert!(matches!(
            local(&server).verify_profile().await,
            Err(MollieError::ResponseTooLarge)
        ));
        assert_eq!(server.count(), 1);
    }
    let mut v = json!({"resource":"profile","id":"pfl_SYNTHETIC_NotARealProfile","padding":""});
    let empty = v.to_string().len();
    v["padding"] = "x".repeat(65536 - empty).into();
    assert_eq!(v.to_string().len(), 65536);
    let server = Server::start(vec![Reply::Raw {
        status: 200,
        body: v.to_string(),
        mime: "application/json; charset=utf-8",
        chunked: true,
    }]);
    assert!(local(&server).verify_profile().await.is_ok());
    assert_eq!(server.count(), 1);
    let server = Server::start(vec![Reply::Delay(Duration::from_millis(200))]);
    let key = credential();
    let c = config(&key);
    let client = TestClient::local(key, c, server.base.clone(), Duration::from_millis(40))
        .expect("test timeout");
    let started = std::time::Instant::now();
    assert!(matches!(
        client.verify_profile().await,
        Err(MollieError::Unavailable)
    ));
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(server.count(), 1);
}
#[tokio::test]
async fn authenticated_fetch_preserves_late_paid_under_a_verified_same_profile_rotated_test_key() {
    let (_, f) = captured(38245);
    let mut response = payment(f.snapshot());
    response["details"] = json!({"private_bank_data":"synthetic-private-bank-details"});
    let server = Server::start(vec![profile(), Reply::Json(200, response.clone())]);
    let rotated = TestCredential::capture(
        &format!("test_{}", "rotated".repeat(4)),
        "pfl_SYNTHETIC_NotARealProfile",
    )
    .expect("rotated TEST key");
    let mut policy = config(&rotated).snapshot().clone();
    policy.webhook_url = "https://shop.example.test/changed-hook".into();
    let client = TestClient::local(
        rotated,
        Configuration::capture(policy).expect("new callback policy"),
        server.base.clone(),
        Duration::from_secs(2),
    )
    .expect("local")
    .verify_profile()
    .await
    .expect("same authenticated profile");
    let observation = client
        .fetch(f.snapshot(), "tr_SYNTHETIC_NotARealPayment")
        .await
        .expect("GET ignores old hold and POST credential/cache policy");
    assert!(matches!(observation.payment().status, PaymentStatus::Paid));
    assert_eq!(observation.payment().amount_minor, 38245);
    assert_ne!(
        observation.credential_fingerprint(),
        f.snapshot().binding.profile.credential_fingerprint
    );
    assert_eq!(
        observation.credential_fingerprint(),
        client
            .configuration()
            .snapshot()
            .profile
            .credential_fingerprint
    );
    assert_eq!(
        observation.checkout_url(),
        Some("https://www.mollie.com/payscreen/select-method/synthetic")
    );
    assert!(
        response
            .to_string()
            .contains("synthetic-private-bank-details")
    );
    assert!(
        !serde_json::to_string(observation.payment())
            .expect("minimal projection")
            .contains("synthetic-private-bank-details")
    );
    server.inspect(|r| {
        assert_eq!(r.len(), 2);
        assert_eq!(r[1].method, "GET");
        assert_eq!(r[1].path, "/v2/payments/tr_SYNTHETIC_NotARealPayment");
        assert!(r[1].body.is_empty());
    });
}
#[tokio::test]
async fn fetch_never_uses_a_different_profile_or_accepts_changed_provider_identity() {
    let (_, f) = captured(38245);
    let server = Server::start(vec![profile()]);
    let client = local(&server).verify_profile().await.expect("verified");
    for id in [
        "",
        "tr_",
        "tr_bad\n",
        "tr_bad\nmiddle",
        &format!("tr_{}", "x".repeat(254)),
    ] {
        assert!(matches!(
            client.fetch(f.snapshot(), id).await,
            Err(MollieError::PaymentMismatch)
        ));
    }
    let mut wrong = f.snapshot().clone();
    wrong.binding.profile.id = "pfl_OTHER".into();
    assert!(matches!(
        client.fetch(&wrong, "tr_SYNTHETIC").await,
        Err(MollieError::PaymentMismatch)
    ));
    wrong.schema_version = 2;
    assert!(matches!(
        client.fetch(&wrong, "tr_SYNTHETIC").await,
        Err(MollieError::PaymentMismatch)
    ));
    assert_eq!(server.count(), 1);
    let server = Server::start(vec![profile(), Reply::Json(200, payment(f.snapshot()))]);
    let client = local(&server).verify_profile().await.expect("verified");
    assert!(matches!(
        client.fetch(f.snapshot(), "tr_DIFFERENT").await,
        Err(MollieError::PaymentMismatch)
    ));
    assert_eq!(server.count(), 2);
    // Opaque IDs remain one encoded path segment and cannot choose a host/query/path.
    let mut reply = payment(f.snapshot());
    reply["id"] = "tr_opaque/with?query#fragment".into();
    let server = Server::start(vec![profile(), Reply::Json(200, reply)]);
    let client = local(&server).verify_profile().await.expect("verified");
    assert!(
        client
            .fetch(f.snapshot(), "tr_opaque/with?query#fragment")
            .await
            .is_ok()
    );
    server.inspect(|r| {
        assert_eq!(
            r[1].path,
            "/v2/payments/tr_opaque%2Fwith%3Fquery%23fragment"
        )
    });
}
#[test]
fn checkout_links_require_canonical_known_https_hosts_without_discarding_the_payment() {
    let (_, f) = captured(38245);
    for href in [
        "http://www.mollie.com/checkout",
        "https://www.mollie.com.evil.test/checkout",
        "https://127.0.0.1/checkout",
        "https://name@www.mollie.com/checkout",
        "https://:password@www.mollie.com/checkout",
        "https://www.mollie.com:444/checkout",
        "https://www.mollie.com/checkout#fragment",
        "https://www.mollie.com/a/../checkout",
        ":",
        &"x".repeat(2049),
    ] {
        let mut v = payment(f.snapshot());
        v["_links"]["checkout"]["href"] = href.into();
        let (p, link) = wire::payment(&serde_json::to_vec(&v).expect("JSON"), f.snapshot(), None)
            .expect("matched financial evidence");
        assert!(matches!(p.status, PaymentStatus::Paid));
        assert!(link.is_none());
    }
    for host in ["mollie.com", "www.mollie.com", "checkout.mollie.com"] {
        let mut v = payment(f.snapshot());
        let href = format!("https://{host}/checkout?token=synthetic");
        v["_links"]["checkout"]["href"] = href.clone().into();
        assert_eq!(
            wire::payment(&serde_json::to_vec(&v).expect("JSON"), f.snapshot(), None)
                .expect("matched")
                .1
                .as_deref(),
            Some(href.as_str())
        );
    }
}
#[test]
fn configuration_and_loopback_overrides_never_grant_a_production_endpoint_or_wrong_key() {
    let server = Server::start(vec![]);
    let mut policy = config(&credential()).snapshot().clone();
    policy.profile.credential_fingerprint = "b".repeat(64);
    assert!(matches!(
        TestClient::new(
            credential(),
            Configuration::capture(policy).expect("valid differing key")
        ),
        Err(MollieError::InvalidConfiguration)
    ));
    for base in [
        "https://api.mollie.com/v2/",
        "http://localhost/v2/",
        "http://127.0.0.1/other/",
        "http://name@127.0.0.1/v2/",
        "http://:password@127.0.0.1/v2/",
        "http://127.0.0.1/v2/?query",
        "http://127.0.0.1/v2/#fragment",
    ] {
        assert!(matches!(
            TestClient::local(
                credential(),
                config(&credential()),
                url::Url::parse(base).expect("test URI"),
                Duration::from_secs(1)
            ),
            Err(MollieError::InvalidConfiguration)
        ));
    }
    assert_eq!(server.count(), 0);
}
use crate::{
    MIGRATOR,
    guest_baskets::GuestCredential,
    payment_attempts::{DispatchOutcome, prepare_attempt, start_dispatch},
};
use sqlx::{Executor, PgPool};
use std::sync::atomic::{AtomicU64, Ordering};
#[sqlx::test(migrator = "MIGRATOR")]
async fn durable_permit_sends_exact_oneoff_bytes_and_key_then_requires_separate_authenticated_fetch(
    pool: PgPool,
) {
    let (store, time, session, csrf, order) = crate::payment_test_fixture::accepted(&pool).await;
    let guest = GuestCredential::from_session(&session).expect("owner");
    let c = config(&credential());
    let frozen = prepare_attempt(
        &store,
        &guest,
        &csrf,
        "pay",
        &crate::payment_test_fixture::intent(&order),
        &c,
    )
    .await
    .expect("prepared")
    .frozen;
    let response = payment(frozen.snapshot());
    assert!(
        wire::payment(
            &serde_json::to_vec(&response).expect("JSON"),
            frozen.snapshot(),
            None
        )
        .is_ok(),
        "independently matched provider fixture"
    );
    let server = Server::start(vec![
        profile(),
        Reply::Json(201, response.clone()),
        Reply::Json(200, response),
    ]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("profile first");
    let ids = crate::payment_test_fixture::Ids(AtomicU64::new(10000));
    let DispatchOutcome::Start(permit) = start_dispatch(
        &pool,
        time.as_ref(),
        &ids,
        &frozen.snapshot().binding.id,
        client.configuration(),
    )
    .await
    .expect("durable initial start") else {
        panic!("single permit")
    };
    let result = client.create(permit, time.as_ref()).await;
    if result.is_err() {
        server.inspect(|r| {
            eprintln!(
                "Local provider received method/path/body lengths: {:?}",
                r.iter()
                    .map(|v| (&v.method, &v.path, v.body.len()))
                    .collect::<Vec<_>>()
            )
        });
    }
    let hint = result.expect("one POST identity hint");
    assert_eq!(hint.provider_id(), "tr_SYNTHETIC_NotARealPayment");
    assert_eq!(server.count(), 2);
    assert_eq!(
        crate::payment_test_fixture::counts(&pool).await,
        (1, 3, 1, 1, 2)
    );
    let verified = client
        .fetch(frozen.snapshot(), hint.provider_id())
        .await
        .expect("separate authenticated GET verification");
    assert_eq!(verified.payment().amount_minor, 38245);
    assert!(matches!(verified.payment().status, PaymentStatus::Paid));
    assert!(matches!(
        start_dispatch(
            &pool,
            time.as_ref(),
            &ids,
            &frozen.snapshot().binding.id,
            client.configuration()
        )
        .await,
        Ok(DispatchOutcome::Reconcile { .. })
    ));
    assert_eq!(server.count(), 3);
    assert_eq!(
        crate::payment_test_fixture::counts(&pool).await,
        (1, 3, 1, 1, 2)
    );
    server.inspect(|requests| {
        assert_eq!(requests[0].path, "/v2/profiles/me");
        assert_eq!(requests[1].method, "POST");
        assert_eq!(requests[1].path, "/v2/payments");
        assert_eq!(
            requests[1].header("Idempotency-Key"),
            Some(format!("piwas-test-payment-{}", frozen.snapshot().binding.id).as_str())
        );
        assert_eq!(requests[1].header("Content-Type"), Some("application/json"));
        assert_eq!(requests[1].body, frozen.body().as_bytes());
        let body: Value = serde_json::from_slice(&requests[1].body).expect("wire JSON");
        assert_eq!(body["amount"], json!({"currency":"EUR","value":"382.45"}));
        assert_eq!(body["sequenceType"], "oneoff");
        for forbidden in [
            "profileId",
            "testmode",
            "email",
            "billingAddress",
            "shippingAddress",
            "credential",
        ] {
            assert!(body.get(forbidden).is_none());
        }
        assert_eq!(requests[2].method, "GET");
        assert!(!requests[2].path.contains("testmode"));
        assert!(!requests[2].path.contains("profileId"));
    });
}
async fn failed_create(pool: PgPool, kind: u8) {
    let (store, time, session, csrf, order) = crate::payment_test_fixture::accepted(&pool).await;
    let guest = GuestCredential::from_session(&session).expect("owner");
    let c = config(&credential());
    let f = prepare_attempt(
        &store,
        &guest,
        &csrf,
        "pay",
        &crate::payment_test_fixture::intent(&order),
        &c,
    )
    .await
    .expect("prepared")
    .frozen;
    let good = payment(f.snapshot());
    let mut wrong = good.clone();
    wrong["amount"]["value"] = "382.44".into();
    let bad = match kind {
        0 => Reply::Close,
        1 => Reply::Raw {
            status: 201,
            body: "{invalid".into(),
            mime: "application/json",
            chunked: false,
        },
        2 => Reply::Json(422, json!({"detail":"synthetic-private-provider-error"})),
        _ => Reply::Json(201, wrong),
    };
    // Keep a second create response available: an automatic retry would be observable.
    let server = Server::start(vec![profile(), bad, Reply::Json(201, good)]);
    let client = local(&server).verify_profile().await.expect("verified");
    let ids = crate::payment_test_fixture::Ids(AtomicU64::new(10000));
    let DispatchOutcome::Start(permit) = start_dispatch(
        &pool,
        time.as_ref(),
        &ids,
        &f.snapshot().binding.id,
        client.configuration(),
    )
    .await
    .expect("start") else {
        panic!("permit")
    };
    assert!(matches!(
        client.create(permit, time.as_ref()).await,
        Err(MollieError::CreateUncertain)
    ));
    assert_eq!(server.count(), 2);
    assert!(matches!(
        start_dispatch(
            &pool,
            time.as_ref(),
            &ids,
            &f.snapshot().binding.id,
            client.configuration()
        )
        .await,
        Ok(DispatchOutcome::Reconcile { .. })
    ));
    assert_eq!(
        crate::payment_test_fixture::counts(&pool).await,
        (1, 3, 1, 1, 2)
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn lost_create_reply_never_retries_even_when_another_success_response_is_available(
    pool: PgPool,
) {
    failed_create(pool, 0).await;
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn malformed_create_reply_never_replaces_original_started_attempt(pool: PgPool) {
    failed_create(pool, 1).await;
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn rejected_create_reply_is_redacted_and_still_requires_reconciliation(pool: PgPool) {
    failed_create(pool, 2).await;
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn mismatched_create_projection_never_becomes_a_payment_identity_hint(pool: PgPool) {
    failed_create(pool, 3).await;
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn retained_seller_deadline_blocks_post_even_with_a_verified_profile_and_live_stock(
    pool: PgPool,
) {
    let (store, time, session, csrf, order) =
        crate::payment_test_fixture::accepted_with_windows(&pool, 1100, 1600, None).await;
    let guest = GuestCredential::from_session(&session).expect("owner");
    let c = config(&credential());
    let f = prepare_attempt(
        &store,
        &guest,
        &csrf,
        "pay",
        &crate::payment_test_fixture::intent(&order),
        &c,
    )
    .await
    .expect("prepared")
    .frozen;
    let server = Server::start(vec![profile()]);
    let client = local(&server).verify_profile().await.expect("verified");
    let ids = crate::payment_test_fixture::Ids(AtomicU64::new(10000));
    let DispatchOutcome::Start(permit) = start_dispatch(
        &pool,
        time.as_ref(),
        &ids,
        &f.snapshot().binding.id,
        client.configuration(),
    )
    .await
    .expect("start") else {
        panic!("permit")
    };
    time.0.store(1600, Ordering::SeqCst);
    assert!(matches!(
        client.create(permit, time.as_ref()).await,
        Err(MollieError::Custody(
            crate::payment_attempts::PaymentStoreError::Unavailable
        ))
    ));
    assert_eq!(server.count(), 1);
    assert!(matches!(
        start_dispatch(
            &pool,
            time.as_ref(),
            &ids,
            &f.snapshot().binding.id,
            client.configuration()
        )
        .await,
        Ok(DispatchOutcome::Reconcile { .. })
    ));
    assert_eq!(
        crate::payment_test_fixture::counts(&pool).await,
        (1, 3, 1, 1, 2)
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn cancellation_during_provider_io_holds_no_database_claim_and_preserves_recovery(
    pool: PgPool,
) {
    use std::sync::{Arc, atomic::AtomicBool};
    let (store, time, session, csrf, order) = crate::payment_test_fixture::accepted(&pool).await;
    let guest = GuestCredential::from_session(&session).expect("owner");
    let c = config(&credential());
    let f = prepare_attempt(
        &store,
        &guest,
        &csrf,
        "pay",
        &crate::payment_test_fixture::intent(&order),
        &c,
    )
    .await
    .expect("prepared")
    .frozen;
    let gate = Arc::new(AtomicBool::new(false));
    let server = Server::start(vec![profile(), Reply::Gate(gate.clone())]);
    let client = local(&server).verify_profile().await.expect("verified");
    let ids = crate::payment_test_fixture::Ids(AtomicU64::new(10000));
    let DispatchOutcome::Start(permit) = start_dispatch(
        &pool,
        time.as_ref(),
        &ids,
        &f.snapshot().binding.id,
        client.configuration(),
    )
    .await
    .expect("start") else {
        panic!("permit")
    };
    let clock = time.clone();
    let task = tokio::spawn(async move { client.create(permit, clock.as_ref()).await });
    tokio::time::timeout(Duration::from_secs(1), async {
        while server.count() < 2 {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("provider has received POST and is gated");
    let mut tx = pool.begin().await.expect("independent observer");
    tx.execute("SET LOCAL lock_timeout='200ms'")
        .await
        .expect("bounded proof");
    tx.execute("SELECT id FROM payment_attempts FOR UPDATE")
        .await
        .expect("attempt claim already released");
    tx.execute("SELECT component_code FROM stock_balances ORDER BY component_code FOR UPDATE")
        .await
        .expect("physical stock claims already released");
    tx.rollback().await.expect("observer cleanup");
    task.abort();
    assert!(task.await.err().expect("cancelled").is_cancelled());
    gate.store(true, Ordering::SeqCst);
    assert!(matches!(
        start_dispatch(
            &pool,
            time.as_ref(),
            &ids,
            &f.snapshot().binding.id,
            &config(&credential())
        )
        .await,
        Ok(DispatchOutcome::Reconcile { .. })
    ));
    assert_eq!(server.count(), 2);
    assert_eq!(
        crate::payment_test_fixture::counts(&pool).await,
        (1, 3, 1, 1, 2)
    );
}

#[tokio::test]
async fn unsupported_content_encoding_cannot_bypass_byte_limits_or_decode_as_a_profile() {
    for encoding in ["gzip", "br", "deflate", "zstd"] {
        let server = Server::start(vec![Reply::Encoded(
            encoding,
            json!({"resource":"profile","id":"pfl_SYNTHETIC_NotARealProfile"}),
        )]);
        assert!(matches!(
            local(&server).verify_profile().await,
            Err(MollieError::InvalidResponse)
        ));
    }
    let server = Server::start(vec![Reply::Encoded(
        "identity",
        json!({"resource":"profile","id":"pfl_SYNTHETIC_NotARealProfile"}),
    )]);
    assert!(local(&server).verify_profile().await.is_ok());
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn rotated_same_profile_key_can_never_send_a_post_under_the_original_permit(pool: PgPool) {
    let (store, time, session, csrf, order) = crate::payment_test_fixture::accepted(&pool).await;
    let guest = GuestCredential::from_session(&session).expect("owner");
    let c = config(&credential());
    let f = prepare_attempt(
        &store,
        &guest,
        &csrf,
        "pay",
        &crate::payment_test_fixture::intent(&order),
        &c,
    )
    .await
    .expect("prepared")
    .frozen;
    let ids = crate::payment_test_fixture::Ids(AtomicU64::new(10000));
    let DispatchOutcome::Start(permit) =
        start_dispatch(&pool, time.as_ref(), &ids, &f.snapshot().binding.id, &c)
            .await
            .expect("start")
    else {
        panic!("permit")
    };
    let server = Server::start(vec![profile()]);
    let key = TestCredential::capture(
        &format!("test_{}", "rotated".repeat(4)),
        "pfl_SYNTHETIC_NotARealProfile",
    )
    .expect("rotated key");
    let policy = config(&key);
    let client = TestClient::local(key, policy, server.base.clone(), Duration::from_secs(2))
        .expect("loopback")
        .verify_profile()
        .await
        .expect("same profile");
    assert!(matches!(
        client.create(permit, time.as_ref()).await,
        Err(MollieError::Custody(
            crate::payment_attempts::PaymentStoreError::Unavailable
        ))
    ));
    assert_eq!(server.count(), 1);
    assert!(matches!(
        start_dispatch(
            &pool,
            time.as_ref(),
            &ids,
            &f.snapshot().binding.id,
            client.configuration()
        )
        .await,
        Ok(DispatchOutcome::Reconcile { .. })
    ));
    assert_eq!(
        crate::payment_test_fixture::counts(&pool).await,
        (1, 3, 1, 1, 2)
    );
}

#[test]
fn duplicate_critical_provider_fields_are_not_resolved_by_a_lossy_json_projection() {
    let (_, f) = captured(38245);
    let source = payment(f.snapshot()).to_string();
    for (old, new) in [
        (
            "\"status\":\"paid\"",
            "\"status\":\"pending\",\"status\":\"paid\"",
        ),
        ("\"mode\":\"test\"", "\"mode\":\"live\",\"mode\":\"test\""),
        (
            "\"value\":\"382.45\"",
            "\"value\":\"1.00\",\"value\":\"382.45\"",
        ),
        (
            "\"schema_version\":1",
            "\"schema_version\":2,\"schema_version\":1",
        ),
    ] {
        assert!(source.contains(old));
        let ambiguous = source.replace(old, new);
        assert!(matches!(
            wire::payment(ambiguous.as_bytes(), f.snapshot(), None),
            Err(MollieError::InvalidResponse)
        ));
    }
    assert!(matches!(
        wire::profile(
            br#"{"resource":"profile","id":"pfl_OTHER","id":"pfl_SYNTHETIC_NotARealProfile"}"#,
            "pfl_SYNTHETIC_NotARealProfile"
        ),
        Err(MollieError::InvalidResponse)
    ));
    assert!(wire::profile(br#"{"resource":"profile","id":"pfl_SYNTHETIC_NotARealProfile","name":"ignored merchant detail"}"#,"pfl_SYNTHETIC_NotARealProfile").is_ok());
    assert!(wire::payment(source.as_bytes(), f.snapshot(), None).is_ok());
}

#[tokio::test]
async fn streaming_limit_rejects_before_an_unfinished_provider_body_can_timeout_or_grow() {
    let server = Server::start(vec![Reply::OversizedPrefixThenStall]);
    let started = std::time::Instant::now();
    assert!(matches!(
        local(&server).verify_profile().await,
        Err(MollieError::ResponseTooLarge)
    ));
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(server.count(), 1);
}

#[path = "mollie_search.rs"]
mod search_cases;

#[path = "mollie_observations.rs"]
mod observation_cases;

#[path = "mollie_search_custody.rs"]
mod search_custody;

#[path = "payment_identities.rs"]
mod identity_cases;

#[path = "payment_identity_ownership.rs"]
mod identity_ownership;
