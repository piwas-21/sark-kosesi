//! Bounded closed private receipt decoding and accepted-order binding under damaged imports.
#[allow(dead_code)]
#[path = "support/payment_fixture.rs"]
mod fixture;
use commerce_application::payment_requests::{Configuration, FrozenAttempt};
use commerce_infrastructure::{MIGRATOR, guest_baskets::*, payment_attempts::*};
use sqlx::{Executor, PgPool};
#[sqlx::test(migrator = "MIGRATOR")]
async fn damaged_payment_imports_fail_closed_and_exact_original_receipt_restores(pool: PgPool) {
    let (store, _, session, csrf, order) = fixture::accepted(&pool).await;
    let key = GuestCredential::from_session(&session).expect("owner");
    let config = fixture::config();
    let intent = fixture::intent(&order);
    let original = prepare_attempt(&store, &key, &csrf, "pay", &intent, &config)
        .await
        .expect("prepare")
        .frozen;
    let id = &original.snapshot().binding.id;
    pool.execute("ALTER TABLE payment_attempts DISABLE TRIGGER guard_payment_attempt;ALTER TABLE payment_attempts DROP CONSTRAINT payment_attempts_snapshot_check,DROP CONSTRAINT payment_attempts_configuration_check,DROP CONSTRAINT payment_attempts_intent_check,DROP CONSTRAINT payment_attempts_request_body_check,DROP CONSTRAINT payment_attempts_configuration_material_check,DROP CONSTRAINT payment_attempts_effect_key_check,DROP CONSTRAINT payment_attempts_key_binding").await.expect("owned isolated corruption instrument");
    for (column, baseline) in [
        (
            "snapshot",
            serde_json::to_value(original.snapshot()).expect("snapshot"),
        ),
        (
            "configuration",
            serde_json::to_value(config.snapshot()).expect("config"),
        ),
        ("intent", serde_json::to_value(&intent).expect("intent")),
    ] {
        let mut changes = vec![
            serde_json::json!([]),
            serde_json::json!({}),
            serde_json::json!({"oversized":"x".repeat(16385)}),
        ];
        let pointers: &[&str] = match column {
            "snapshot" => &["/binding", "/binding/profile"],
            "configuration" => &["/profile"],
            _ => &[],
        };
        for pointer in pointers {
            let mut v = baseline.clone();
            *v.pointer_mut(pointer).expect("object") = serde_json::json!([]);
            changes.push(v);
        }
        let mut unknown = baseline.clone();
        unknown["role"] = "admin".into();
        changes.push(unknown);
        match column {
            "snapshot" => {
                for (pointer, value) in [
                    ("/schema_version", serde_json::json!(2)),
                    ("/request_sha256", serde_json::json!("bad")),
                    ("/binding/id", serde_json::json!("f".repeat(32))),
                    ("/binding/order_id", serde_json::json!("f".repeat(32))),
                    ("/binding/basket_id", serde_json::json!("f".repeat(32))),
                    ("/binding/created_at", serde_json::json!(1501)),
                    ("/binding/dispatch_before", serde_json::json!(1999)),
                    ("/binding/profile/mode", serde_json::json!("live")),
                    ("/binding/currency", serde_json::json!("USD")),
                    ("/binding/amount_minor", serde_json::json!(0)),
                    ("/binding/amount_minor", serde_json::json!(38244)),
                ] {
                    let mut v = baseline.clone();
                    *v.pointer_mut(pointer).expect("field") = value;
                    changes.push(v);
                }
            }
            "configuration" => {
                for (pointer, value) in [
                    ("/schema_version", serde_json::json!(2)),
                    ("/return_url", serde_json::json!("http://bad.example.test")),
                    ("/profile/mode", serde_json::json!("live")),
                    ("/method", serde_json::json!("unknown")),
                ] {
                    let mut v = baseline.clone();
                    *v.pointer_mut(pointer).expect("field") = value;
                    changes.push(v);
                }
            }
            _ => {
                for (pointer, value) in [
                    ("/order_id", serde_json::json!("f".repeat(32))),
                    ("/confirmed", serde_json::json!(false)),
                ] {
                    let mut v = baseline.clone();
                    *v.pointer_mut(pointer).expect("field") = value;
                    changes.push(v);
                }
            }
        }
        for value in changes {
            sqlx::query(update(column))
                .bind(value)
                .execute(&pool)
                .await
                .expect("damage synthetic row");
            assert!(
                matches!(
                    owned_attempt(&store, &key, id).await,
                    Err(PaymentStoreError::InvalidStoredData)
                ),
                "closed {column}"
            );
        }
        sqlx::query(update(column))
            .bind(baseline)
            .execute(&pool)
            .await
            .expect("restore synthetic row");
        assert!(
            owned_attempt(&store, &key, id)
                .await
                .expect("positive restore")
                .snapshot()
                == original.snapshot()
        );
    }
    for (column, baseline, changes) in [
        (
            "request_body",
            original.body().to_owned(),
            vec![
                "[]".into(),
                format!("{} ", original.body()),
                original.body().replace("382.45", "382.44"),
                "x".repeat(8193),
            ],
        ),
        (
            "configuration_material",
            config.fingerprint_material().to_owned(),
            vec!["[]".into(), "x".repeat(8193)],
        ),
        (
            "effect_key",
            "pay".into(),
            vec!["bad key".into(), "x".repeat(129)],
        ),
        (
            "idempotency_key",
            original.snapshot().idempotency_key(),
            vec!["changed".into(), "x".repeat(100)],
        ),
    ] {
        for value in changes {
            sqlx::query(update(column))
                .bind(value)
                .execute(&pool)
                .await
                .expect("damaged bytes/header");
            assert!(
                matches!(
                    owned_attempt(&store, &key, id).await,
                    Err(PaymentStoreError::InvalidStoredData)
                ),
                "closed {column}"
            );
        }
        sqlx::query(update(column))
            .bind(baseline)
            .execute(&pool)
            .await
            .expect("restore bytes/header");
    }
    // A coherent replacement body+digest must still equal the actual accepted order.
    for change in 0..4 {
        let mut b = original.snapshot().binding.clone();
        match change {
            0 => b.amount_minor = 38244,
            1 => b.stock_group_id = "f".repeat(32),
            2 => b.created_at = 1000,
            _ => b.dispatch_before = 1999,
        };
        let counterfeit =
            FrozenAttempt::capture(b, &config).expect("individually valid synthetic record");
        sqlx::query("UPDATE payment_attempts SET snapshot=$1,request_body=$2,created_at=$3,dispatch_before=$4").bind(serde_json::to_value(counterfeit.snapshot()).expect("snapshot")).bind(counterfeit.body()).bind(counterfeit.snapshot().binding.created_at).bind(counterfeit.snapshot().binding.dispatch_before).execute(&pool).await.expect("coherent damaged import");
        assert!(matches!(
            owned_attempt(&store, &key, id).await,
            Err(PaymentStoreError::InvalidStoredData)
        ));
    }
    sqlx::query("UPDATE payment_attempts SET snapshot=$1,request_body=$2,created_at=1500,dispatch_before=2000").bind(serde_json::to_value(original.snapshot()).expect("snapshot")).bind(original.body()).execute(&pool).await.expect("restore original");
    let restored = owned_attempt(&store, &key, id)
        .await
        .expect("positive exact receipt");
    assert!(restored.snapshot() == original.snapshot());
    assert_eq!(restored.body(), original.body());
    assert!(
        prepare_attempt(
            &store,
            &key,
            &csrf,
            "pay",
            &intent,
            &Configuration::capture(config.snapshot().clone()).expect("config")
        )
        .await
        .expect("original receipt")
        .replayed
    );
}

fn update(column: &str) -> &'static str {
    match column {
        "snapshot" => "UPDATE payment_attempts SET snapshot=$1",
        "configuration" => "UPDATE payment_attempts SET configuration=$1",
        "intent" => "UPDATE payment_attempts SET intent=$1",
        "request_body" => "UPDATE payment_attempts SET request_body=$1",
        "configuration_material" => "UPDATE payment_attempts SET configuration_material=$1",
        "effect_key" => "UPDATE payment_attempts SET effect_key=$1",
        "idempotency_key" => "UPDATE payment_attempts SET idempotency_key=$1",
        _ => panic!("unknown isolated fixture column"),
    }
}

#[test]
fn payment_failure_diagnostics_discard_private_nested_dependency_details() {
    use commerce_application::payment_requests::RequestError;
    use commerce_infrastructure::{
        checkout_orders::OrderStoreError, holds::HoldWriteError, market_policies::MarketError,
        quotes::QuoteFailure,
    };
    let marker = "synthetic-private-contact-and-provider-body";
    let raw = sqlx::Error::Protocol(marker.into());
    assert!(format!("{raw:?}").contains(marker));
    let errors = [
        PaymentStoreError::from(sqlx::Error::Protocol(marker.into())),
        PaymentStoreError::from(QuoteFailure::Database(sqlx::Error::Protocol(marker.into()))),
        PaymentStoreError::from(MarketError::Database(sqlx::Error::Protocol(marker.into()))),
        PaymentStoreError::from(HoldWriteError::Database(sqlx::Error::Protocol(
            marker.into(),
        ))),
        PaymentStoreError::from(OrderStoreError::InvalidStoredData),
        PaymentStoreError::from(RequestError::InvalidConfiguration),
        PaymentStoreError::from(RequestError::InvalidBinding),
        PaymentStoreError::from(RequestError::InvalidStoredRequest),
    ];
    for e in errors {
        assert!(!format!("{e:?}").contains(marker));
    }
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn malformed_durable_start_or_impossible_reconciliation_clock_never_grants_a_post(
    pool: PgPool,
) {
    use std::sync::atomic::{AtomicU64, Ordering};
    let (store, time, session, csrf, order) = fixture::accepted(&pool).await;
    let key = GuestCredential::from_session(&session).expect("owner");
    let config = fixture::config();
    let f = prepare_attempt(
        &store,
        &key,
        &csrf,
        "pay",
        &fixture::intent(&order),
        &config,
    )
    .await
    .expect("prepared")
    .frozen;
    let id = &f.snapshot().binding.id;
    let ids = fixture::Ids(AtomicU64::new(10000));
    assert!(matches!(
        start_dispatch(&pool, time.as_ref(), &ids, id, &config).await,
        Ok(DispatchOutcome::Start(_))
    ));
    pool.execute("ALTER TABLE payment_dispatch_starts DISABLE TRIGGER USER;ALTER TABLE payment_dispatch_starts DROP CONSTRAINT payment_dispatch_starts_id_check").await.expect("isolated malformed import");
    pool.execute("UPDATE payment_dispatch_starts SET id='bad'")
        .await
        .expect("bad identity");
    assert!(matches!(
        start_dispatch(&pool, time.as_ref(), &ids, id, &config).await,
        Err(PaymentStoreError::InvalidStoredData)
    ));
    pool.execute(
        "UPDATE payment_dispatch_starts SET id='00000000000000000000000000002711',started_at=1499",
    )
    .await
    .expect("invalid start window");
    assert!(matches!(
        start_dispatch(&pool, time.as_ref(), &ids, id, &config).await,
        Err(PaymentStoreError::InvalidStoredData)
    ));
    pool.execute("UPDATE payment_dispatch_starts SET started_at=1600")
        .await
        .expect("restored valid start");
    assert!(matches!(
        start_dispatch(&pool, time.as_ref(), &ids, id, &config).await,
        Err(PaymentStoreError::ClockUnavailable)
    ));
    time.0.store(9_007_199_254_740_992, Ordering::SeqCst);
    assert!(matches!(
        start_dispatch(&pool, time.as_ref(), &ids, id, &config).await,
        Err(PaymentStoreError::ClockUnavailable)
    ));
    time.0.store(3000, Ordering::SeqCst);
    assert!(matches!(
        start_dispatch(&pool, time.as_ref(), &ids, id, &config).await,
        Ok(DispatchOutcome::Reconcile {
            started_at: 1600,
            ..
        })
    ));
    assert_eq!(fixture::counts(&pool).await, (1, 3, 1, 1, 2));
}
