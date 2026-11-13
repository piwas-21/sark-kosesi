//! One accepted test POST, a lost reply, and late stock recovery on PostgreSQL.
use super::*;
use crate::{
    holds::{ExpiryBatch, expire_due_holds},
    late_stock::{read_recorded, recover},
    order_stock::read_settlement,
    payment_attempts::{
        recognition::{current, recognize},
        reconciliation::{ReviewReason, read_job, run_batch},
    },
};
use commerce_domain::{late_stock::Outcome, payment_attempts::recognition::State};

async fn original_hold(pool: &PgPool, group: &str) -> (String, String, i64) {
    let rows = sqlx::query_as(
        "SELECT id,state,expires_at FROM stock_holds WHERE basket_group_id=$1 ORDER BY id",
    )
    .bind(group)
    .fetch_all(pool)
    .await
    .expect("exact original group members");
    let [hold]: [_; 1] = rows.try_into().expect("one original hold");
    hold
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn lost_create_reply_recovers_exact_late_payment_and_allocates_stock_once(pool: PgPool) {
    let (store, time, session, csrf, order) = crate::payment_test_fixture::accepted(&pool).await;
    let owner = GuestCredential::from_session(&session).expect("guest owner");
    let configuration = config(&credential());
    let frozen = prepare_attempt(
        &store,
        &owner,
        &csrf,
        "journey-attempt",
        &crate::payment_test_fixture::intent(&order),
        &configuration,
    )
    .await
    .expect("exact order and reservation admitted")
    .frozen;
    let attempt = &frozen.snapshot().binding.id;
    let provider_payment = payment(frozen.snapshot());
    let server = Server::start(vec![
        profile(),
        Reply::Close,
        Reply::Json(
            200,
            json!({"count":1,"_embedded":{"payments":[provider_payment.clone()]},"_links":{"next":null}}),
        ),
        Reply::Json(200, provider_payment),
    ]);
    let client = local(&server).verify_profile().await.expect("test profile");
    let ids = observation_cases::ids();
    let DispatchOutcome::Start(permit) =
        start_dispatch(&pool, time.as_ref(), &ids, attempt, client.configuration())
            .await
            .expect("one durable dispatch")
    else {
        panic!("new attempt must receive its one permit");
    };
    assert!(matches!(
        client.create(permit, time.as_ref()).await,
        Err(MollieError::CreateUncertain)
    ));
    assert_eq!(
        server.count(),
        2,
        "loopback received the POST before closing"
    );
    assert!(
        current(&pool, attempt)
            .await
            .expect("no financial proof")
            .is_none()
    );
    assert!(
        read_recorded(&pool, &order.identity.id)
            .await
            .expect("no allocation")
            .is_none()
    );

    let group = &order.identity.stock_group_id;
    let (original_id, state, expires_at) = original_hold(&pool, group).await;
    assert_eq!(state, "active");
    assert_eq!(expires_at, order.identity.hold_expires_at);
    assert!((1501..5000).contains(&expires_at));
    time.0.store(5000, Ordering::SeqCst);
    assert_eq!(
        expire_due_holds(&pool, 5000, 25)
            .await
            .expect("real expiry pass"),
        ExpiryBatch {
            expired: 1,
            failed: 0
        }
    );
    assert_eq!(
        original_hold(&pool, group).await,
        (original_id, "expired".into(), expires_at)
    );
    assert!(matches!(
        start_dispatch(&pool, time.as_ref(), &ids, attempt, client.configuration()).await,
        Ok(DispatchOutcome::Reconcile { .. })
    ));
    assert_eq!(
        server.count(),
        2,
        "the existing start cannot produce a second POST"
    );
    let client = client.into_reconciliation();
    let batch = run_batch(&pool, time.as_ref(), &ids, &client, 25)
        .await
        .expect("bounded GET-only recovery");
    assert_eq!(
        (batch.processed, batch.needs_review, batch.retry_pending),
        (3, 1, 0)
    );
    assert!(matches!(
        read_job(&pool, attempt)
            .await
            .expect("checked job")
            .expect("stored job")
            .review_reason,
        Some(ReviewReason::PaidObserved)
    ));
    assert!(
        read_recorded(&pool, &order.identity.id)
            .await
            .expect("supervisor is evidence only")
            .is_none()
    );

    let recognized = recognize(&pool, time.as_ref(), &ids, attempt)
        .await
        .expect("explicit recognition")
        .recognition;
    assert_eq!(
        (
            recognized.state,
            recognized.amount_minor,
            recognized.through_observation
        ),
        (State::Received, 38245, 1)
    );
    let allocation = recover(&pool, time.as_ref(), &ids, &order.identity.id)
        .await
        .expect("explicit late allocation");
    assert!(!allocation.replayed);
    let Outcome::Late(late) = &allocation.outcome else {
        panic!("available expired stock must be reallocated");
    };
    assert_eq!(late.attempt_id, *attempt);
    assert_eq!(late.recognition_id, recognized.id);
    assert_eq!(late.allocated_at, 5000);
    assert_ne!(late.fresh_group_id, order.identity.stock_group_id);
    assert!(
        read_settlement(&pool, &order.identity.id)
            .await
            .expect("original settlement reader")
            .is_none()
    );
    let replay = recover(
        &pool,
        &observation_cases::BadPorts,
        &observation_cases::BadPorts,
        &order.identity.id,
    )
    .await
    .expect("historical allocation needs neither clock nor entropy");
    assert!(replay.replayed && replay.outcome == allocation.outcome);
    let counts: (i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM payment_dispatch_starts),
                (SELECT count(*) FROM payment_observations),
                (SELECT count(*) FROM order_stock_allocations),
                (SELECT count(*) FROM commerce_outbox WHERE kind='order.stock_late_allocated.v1'),
                (SELECT count(*) FROM stock_movements WHERE action='consume-hold')",
    )
    .fetch_one(&pool)
    .await
    .expect("literal custody and effect counts");
    assert_eq!(counts, (1, 1, 1, 1, 2));
    assert_eq!(
        order_stock_cases::fixture::balances(&pool).await,
        vec![
            ("SEAT-COVER".into(), 0, 0, 0),
            ("SEAT-FOAM".into(), 0, 0, 0)
        ]
    );
    server.inspect(|requests| {
        assert_eq!(
            requests
                .iter()
                .map(|r| (r.method.as_str(), r.path.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("GET", "/v2/profiles/me"),
                ("POST", "/v2/payments"),
                ("GET", "/v2/payments?limit=10&sort=desc"),
                ("GET", "/v2/payments/tr_SYNTHETIC_NotARealPayment"),
            ]
        );
        assert_eq!(requests[1].body, frozen.body().as_bytes());
        let body: Value = serde_json::from_slice(&requests[1].body).expect("observed POST JSON");
        assert_eq!(body["amount"], json!({"currency":"EUR","value":"382.45"}));
        assert_eq!(body["sequenceType"], "oneoff");
        assert_eq!(body["method"], "ideal");
        assert_eq!(
            body["metadata"],
            json!({"schema_version":1,"order_id":order.identity.id,"attempt_id":attempt})
        );
        for private_field in [
            "profileId",
            "testmode",
            "email",
            "billingAddress",
            "shippingAddress",
            "credential",
        ] {
            assert!(body.get(private_field).is_none());
        }
        assert_eq!(
            requests[1].header("Idempotency-Key"),
            Some(format!("piwas-test-payment-{attempt}").as_str())
        );
    });
}
