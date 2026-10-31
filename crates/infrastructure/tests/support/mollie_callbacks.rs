//! Original-attempt callback custody over disposable PostgreSQL and authenticated GETs.
use super::observation_cases::{BadPorts, ids, prepared};
use super::*;
use crate::payment_attempts::{callbacks::record_fetched, identities, observations};
use sqlx::{Executor, PgPool};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
fn reply(a: &AttemptSnapshot, id: &str, status: &str) -> Value {
    let mut p = payment(a);
    p["id"] = id.into();
    p["status"] = status.into();
    p
}
async fn read_client(server: &Server) -> VerifiedReconciliationClient {
    local(server)
        .verify_profile()
        .await
        .expect("authenticated profile")
        .into_reconciliation()
}
fn get_only(server: &Server) {
    server.inspect(|rs| {
        assert!(rs.iter().all(|r| r.method == "GET"
            && r.body.is_empty()
            && r.header("Idempotency-Key").is_none()))
    });
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn callback_id_fetch_matches_original_and_retains_late_paid_and_reordered_facts(
    pool: PgPool,
) {
    let (time, f) = prepared(&pool, true).await;
    let a = &f.snapshot().binding.id;
    time.0.store(5000, Ordering::SeqCst);
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, reply(f.snapshot(), "tr_CALLBACK", "paid")),
        Reply::Json(200, reply(f.snapshot(), "tr_CALLBACK", "open")),
    ]);
    let client = read_client(&server).await;
    let fetched = client
        .fetch_callback("tr_CALLBACK")
        .await
        .expect("only provider ID supplied");
    assert_eq!(fetched.payment_id(), "tr_CALLBACK");
    assert_eq!(fetched.attempt_id_hint(), a);
    let checked = fetched.bind(f.snapshot()).expect("entire original match");
    assert!(
        checked.checkout_url().is_none(),
        "callback custody never retains optional checkout capability"
    );
    let i = ids();
    let first = record_fetched(&pool, time.as_ref(), &i, "callback-1", &fetched)
        .await
        .expect("late paid original custody");
    assert!(!first.replayed);
    assert_eq!(first.snapshot.revision, 1);
    assert_eq!(first.snapshot.recorded_at, 5000);
    assert_eq!(first.snapshot.intent.payment.amount_minor, 38245);
    assert_eq!(
        first.snapshot.intent.credential_fingerprint,
        credential().binding().credential_fingerprint
    );
    let replay = record_fetched(&pool, &BadPorts, &BadPorts, "callback-1", &fetched)
        .await
        .expect("original receipt without new time or entropy");
    assert!(replay.replayed && replay.snapshot == first.snapshot);
    let reordered = client
        .fetch_callback("tr_CALLBACK")
        .await
        .expect("new authenticated fact");
    assert_eq!(
        record_fetched(&pool, time.as_ref(), &i, "callback-1", &reordered)
            .await
            .err(),
        Some(PaymentStoreError::Conflict)
    );
    let second = record_fetched(&pool, time.as_ref(), &i, "callback-2", &reordered)
        .await
        .expect("distinct operation preserves reordered fact");
    assert_eq!(second.snapshot.revision, 2);
    let resolved = identities::resolve(&pool, time.as_ref(), &i, a, "resolve-callbacks")
        .await
        .expect("separate original-identity projection")
        .resolution;
    assert_eq!(resolved.summary.provider_ids, vec!["tr_CALLBACK"]);
    assert_eq!(resolved.summary.paid_provider_ids, vec!["tr_CALLBACK"]);
    assert_eq!(
        crate::payment_test_fixture::counts(&pool).await,
        (1, 6, 1, 1, 2)
    );
    assert_eq!(server.count(), 3);
    get_only(&server);
}
#[tokio::test]
async fn callback_lookup_rejects_wrong_scope_mode_currency_shapes_and_duplicate_critical_fields() {
    let (_, f) = captured(38245);
    let good = reply(f.snapshot(), "tr_CALLBACK", "paid");
    let cases = [
        ("resource", json!("refund"), MollieError::PaymentMismatch),
        (
            "profileId",
            json!("pfl_OTHER"),
            MollieError::PaymentMismatch,
        ),
        ("id", json!("tr_OTHER"), MollieError::PaymentMismatch),
        ("mode", json!("live"), MollieError::InvalidResponse),
        ("currency", json!("USD"), MollieError::InvalidResponse),
        ("schema_version", json!(2), MollieError::PaymentMismatch),
        ("attempt_id", json!("bad"), MollieError::PaymentMismatch),
        ("order_id", json!("bad"), MollieError::PaymentMismatch),
        ("amount", json!([]), MollieError::InvalidResponse),
        ("metadata", json!([]), MollieError::InvalidResponse),
        (
            "sequenceType",
            json!("recurring"),
            MollieError::PaymentMismatch,
        ),
    ];
    for (field, value, error) in cases {
        let mut p = good.clone();
        match field {
            "currency" => p["amount"][field] = value,
            "schema_version" | "attempt_id" | "order_id" => p["metadata"][field] = value,
            _ => p[field] = value,
        }
        let server = Server::start(vec![profile(), Reply::Json(200, p)]);
        let client = read_client(&server).await;
        assert_eq!(
            client.fetch_callback("tr_CALLBACK").await.err(),
            Some(error),
            "{field}"
        );
        assert_eq!(server.count(), 2);
        get_only(&server);
    }
    let raw = serde_json::to_string(&good).expect("valid control");
    for (old, new) in [
        (
            "\"id\":\"tr_CALLBACK\"",
            "\"id\":\"tr_CALLBACK\",\"id\":\"tr_OTHER\"",
        ),
        (
            "\"value\":\"382.45\"",
            "\"value\":\"382.45\",\"value\":\"0.01\"",
        ),
        (
            "\"schema_version\":1",
            "\"schema_version\":1,\"schema_version\":2",
        ),
    ] {
        assert!(raw.contains(old));
        let server = Server::start(vec![
            profile(),
            Reply::Raw {
                status: 200,
                body: raw.replace(old, new),
                mime: "application/json",
                chunked: false,
            },
        ]);
        let client = read_client(&server).await;
        assert_eq!(
            client.fetch_callback("tr_CALLBACK").await.err(),
            Some(MollieError::InvalidResponse)
        );
    }
    let server = Server::start(vec![profile(), Reply::Json(200, good)]);
    let client = read_client(&server).await;
    assert!(
        client.fetch_callback("tr_CALLBACK").await.is_ok(),
        "paired healthy projection"
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn authenticated_metadata_is_only_a_hint_and_cannot_choose_other_order_amount_or_attempt(
    pool: PgPool,
) {
    let (time, f) = prepared(&pool, true).await;
    let i = ids();
    for field in ["amount", "order", "unknown-attempt"] {
        let mut p = reply(f.snapshot(), "tr_MISMATCH", "paid");
        match field {
            "amount" => p["amount"]["value"] = "382.44".into(),
            "order" => p["metadata"]["order_id"] = "f".repeat(32).into(),
            _ => p["metadata"]["attempt_id"] = "f".repeat(32).into(),
        }
        let server = Server::start(vec![profile(), Reply::Json(200, p)]);
        let client = read_client(&server).await;
        let fetched = client
            .fetch_callback("tr_MISMATCH")
            .await
            .expect("authenticated scope is not original matching");
        assert_eq!(
            fetched.bind(f.snapshot()).err(),
            Some(MollieError::PaymentMismatch)
        );
        let expected = if field == "unknown-attempt" {
            PaymentStoreError::NotFound
        } else {
            PaymentStoreError::Conflict
        };
        assert_eq!(
            record_fetched(&pool, time.as_ref(), &i, "bad-original", &fetched)
                .await
                .err(),
            Some(expected)
        );
        assert_eq!(
            crate::payment_test_fixture::counts(&pool).await,
            (1, 3, 1, 1, 2)
        );
        get_only(&server);
    }
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn missing_dispatch_invalid_operation_clock_entropy_and_storage_cannot_create_callback_facts(
    pool: PgPool,
) {
    let (time, f) = prepared(&pool, false).await;
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, reply(f.snapshot(), "tr_UNSTARTED", "paid")),
    ]);
    let client = read_client(&server).await;
    let fetched = client
        .fetch_callback("tr_UNSTARTED")
        .await
        .expect("read cannot mint dispatch custody");
    assert_eq!(
        record_fetched(&pool, time.as_ref(), &ids(), "fetch", &fetched)
            .await
            .err(),
        Some(PaymentStoreError::Unavailable)
    );
    assert_eq!(
        record_fetched(&pool, time.as_ref(), &ids(), "bad!", &fetched)
            .await
            .err(),
        Some(PaymentStoreError::InvalidRequest)
    );
    pool.close().await;
    assert_eq!(
        record_fetched(&pool, time.as_ref(), &ids(), "fetch", &fetched)
            .await
            .err(),
        Some(PaymentStoreError::Database)
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn rotated_same_profile_fetches_are_audited_and_concurrent_callbacks_commit_once(
    pool: PgPool,
) {
    let (time, f) = prepared(&pool, true).await;
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, reply(f.snapshot(), "tr_ROTATED_CALLBACK", "pending")),
    ]);
    let rotated = TestCredential::capture(
        &format!("test_{}", "rotated".repeat(5)),
        "pfl_SYNTHETIC_NotARealProfile",
    )
    .expect("rotated synthetic key");
    let expected = "cb8e8c6ddc442fdd402ba0381304ecc726c62489357de643615702cb393f2a9f";
    let client = ReconciliationClient {
        http: http::Http::build(
            server.base.clone(),
            rotated.authorization,
            Duration::from_secs(2),
        )
        .expect("bounded loopback"),
        profile: rotated.binding,
    }
    .verify_profile()
    .await
    .expect("rotated authenticated profile");
    let fetched = Arc::new(
        client
            .fetch_callback("tr_ROTATED_CALLBACK")
            .await
            .expect("same-profile read rotation"),
    );
    assert_eq!(
        record_fetched(&pool, &BadPorts, &ids(), "clock", &fetched)
            .await
            .err(),
        Some(PaymentStoreError::ClockUnavailable)
    );
    assert!(
        record_fetched(&pool, time.as_ref(), &BadPorts, "entropy", &fetched)
            .await
            .is_err()
    );
    let i = Arc::new(ids());
    let mut tasks = Vec::new();
    for _ in 0..2 {
        let (p, t, i, v) = (pool.clone(), time.clone(), i.clone(), fetched.clone());
        tasks.push(tokio::spawn(async move {
            record_fetched(&p, t.as_ref(), i.as_ref(), "same-callback", v.as_ref()).await
        }));
    }
    let first = tasks.remove(0).await.expect("task").expect("exact receipt");
    let other = tasks
        .remove(0)
        .await
        .expect("task")
        .expect("serialized receipt");
    assert_ne!(first.replayed, other.replayed);
    assert!(first.snapshot == other.snapshot);
    assert_eq!(first.snapshot.intent.credential_fingerprint, expected);
    assert_eq!(
        crate::payment_test_fixture::counts(&pool).await,
        (1, 4, 1, 1, 2)
    );
    assert_eq!(
        observations::read_recorded(&pool, &f.snapshot().binding.id, "same-callback")
            .await
            .expect("durable")
            .expect("fact")
            .intent
            .payment
            .id,
        "tr_ROTATED_CALLBACK"
    );
    get_only(&server);
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn provider_get_cancellation_and_failures_never_hold_database_claims_or_store_facts(
    pool: PgPool,
) {
    let (_time, f) = prepared(&pool, true).await;
    let gate = Arc::new(AtomicBool::new(false));
    let server = Server::start(vec![profile(), Reply::Gate(gate)]);
    let client = Arc::new(read_client(&server).await);
    let c = client.clone();
    let task = tokio::spawn(async move { c.fetch_callback("tr_CALLBACK").await });
    tokio::time::timeout(Duration::from_secs(1), async {
        while server.count() < 2 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("GET in flight");
    let mut tx = pool.begin().await.expect("independent DB claim");
    tx.execute("SET LOCAL lock_timeout='100ms'")
        .await
        .expect("bound");
    sqlx::query("SELECT id FROM payment_attempts WHERE id=$1 FOR UPDATE NOWAIT")
        .bind(&f.snapshot().binding.id)
        .fetch_one(&mut *tx)
        .await
        .expect("no DB lock across GET");
    tx.rollback().await.expect("release");
    task.abort();
    assert!(task.await.err().expect("canceled fetch").is_cancelled());
    assert_eq!(
        crate::payment_test_fixture::counts(&pool).await,
        (1, 3, 1, 1, 2)
    );
    let server = Server::start(vec![
        profile(),
        Reply::Json(404, json!({"detail":"synthetic private response"})),
    ]);
    let client = read_client(&server).await;
    assert_eq!(
        client.fetch_callback("tr_UNKNOWN").await.err(),
        Some(MollieError::Unavailable)
    );
    for id in ["", "tr_", "https://attacker.example", "tr_\n"] {
        assert_eq!(
            client.fetch_callback(id).await.err(),
            Some(MollieError::PaymentMismatch)
        );
    }
    assert_eq!(server.count(), 2);
}

#[tokio::test]
async fn callback_identifier_cannot_select_an_origin_path_query_or_automatic_redirect() {
    let (_, f) = captured(38245);
    let id = "tr_safe/../other?x=1";
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, reply(f.snapshot(), id, "paid")),
    ]);
    let client = read_client(&server).await;
    assert_eq!(
        client
            .fetch_callback(id)
            .await
            .expect("opaque encoded segment")
            .payment_id(),
        id
    );
    server.inspect(|rs| assert_eq!(rs[1].path, "/v2/payments/tr_safe%2F..%2Fother%3Fx=1"));
    get_only(&server);
    let server = Server::start(vec![
        profile(),
        Reply::Redirect("https://attacker.example/callback".into()),
    ]);
    let client = read_client(&server).await;
    assert_eq!(
        client.fetch_callback("tr_CALLBACK").await.err(),
        Some(MollieError::Unavailable)
    );
    assert_eq!(server.count(), 2, "no redirect or replay");
}
