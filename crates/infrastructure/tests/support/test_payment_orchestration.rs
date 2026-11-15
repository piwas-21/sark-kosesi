//! Isolated PostgreSQL and loopback tests for the private composed test dispatch.
use super::*;
use crate::{
    MIGRATOR,
    guest_baskets::GuestCredential,
    payment_attempts::{
        InitialDispatchError, InitialDispatchOutcome, dispatch_and_observe_test_payment,
        observations::read_recorded, prepare_attempt,
    },
};
use sqlx::{Executor, PgPool};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

async fn prepared(pool: &PgPool) -> (Arc<crate::payment_test_fixture::quote::Time>, FrozenAttempt) {
    let (store, time, session, csrf, order) = crate::payment_test_fixture::accepted(pool).await;
    let owner = GuestCredential::from_session(&session).expect("owner");
    let frozen = prepare_attempt(
        &store,
        &owner,
        &csrf,
        "pay",
        &crate::payment_test_fixture::intent(&order),
        &config(&credential()),
    )
    .await
    .expect("prepared")
    .frozen;
    (time, frozen)
}
async fn counts(pool: &PgPool) -> (i64, i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM payment_dispatch_starts), (SELECT count(*) FROM commerce_outbox WHERE kind='payment.reconciliation_required.v1'), (SELECT count(*) FROM payment_observations), (SELECT count(*) FROM commerce_outbox WHERE kind='payment.observation_recorded.v1')").fetch_one(pool).await.expect("counts")
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn competing_calls_post_once_then_store_verified_observation(pool: PgPool) {
    let (time, frozen) = prepared(&pool).await;
    let id = &frozen.snapshot().binding.id;
    let pay = payment(frozen.snapshot());
    let server = Server::start(vec![
        profile(),
        Reply::Json(201, pay.clone()),
        Reply::Json(200, pay),
    ]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("verified profile");
    let ids = crate::payment_test_fixture::Ids(AtomicU64::new(10000));
    let (a, b) = tokio::join!(
        dispatch_and_observe_test_payment(&pool, time.as_ref(), &ids, &client, id),
        dispatch_and_observe_test_payment(&pool, time.as_ref(), &ids, &client, id)
    );
    assert!(matches!(
        (&a, &b),
        (
            Ok(InitialDispatchOutcome::Observed),
            Ok(InitialDispatchOutcome::Reconcile)
        ) | (
            Ok(InitialDispatchOutcome::Reconcile),
            Ok(InitialDispatchOutcome::Observed)
        )
    ));
    assert_eq!(counts(&pool).await, (1, 1, 1, 1));
    let observation = read_recorded(&pool, id, &format!("initial-fetch-{id}"))
        .await
        .expect("stored read")
        .expect("observation");
    assert_eq!(observation.intent.payment.amount_minor, 38245);
    assert_eq!(
        observation.intent.payment.id,
        "tr_SYNTHETIC_NotARealPayment"
    );
    server.inspect(|r| {
        assert_eq!(r.len(), 3);
        assert_eq!(r[1].method, "POST");
        assert_eq!(r[1].body, frozen.body().as_bytes());
        assert_eq!(r[2].method, "GET");
        assert_eq!(r[2].path, "/v2/payments/tr_SYNTHETIC_NotARealPayment");
    });
}

async fn failed(pool: PgPool, fault: u8) {
    let (time, frozen) = prepared(&pool).await;
    let id = &frozen.snapshot().binding.id;
    let pay = payment(frozen.snapshot());
    let mut bad = pay.clone();
    bad["amount"]["value"] = "382.44".into();
    if fault == 2 {
        pool.execute("CREATE FUNCTION fixture_reject_observation() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'synthetic'; END $$").await.expect("fault function");
        pool.execute("CREATE TRIGGER fixture_reject_observation BEFORE INSERT ON payment_observations FOR EACH ROW EXECUTE FUNCTION fixture_reject_observation()").await.expect("fault trigger");
    }
    let replies = match fault {
        0 => vec![
            profile(),
            Reply::Close,
            Reply::Json(
                200,
                json!({"count":1,"_embedded":{"payments":[pay.clone()]},"_links":{"next":null}}),
            ),
            Reply::Json(200, pay.clone()),
        ],
        1 => vec![profile(), Reply::Json(201, pay), Reply::Json(200, bad)],
        _ => vec![
            profile(),
            Reply::Json(201, pay.clone()),
            Reply::Json(200, pay.clone()),
            Reply::Json(
                200,
                json!({"count":1,"_embedded":{"payments":[pay.clone()]},"_links":{"next":null}}),
            ),
            Reply::Json(200, pay),
        ],
    };
    let server = Server::start(replies);
    let client = local(&server).verify_profile().await.expect("verified");
    let ids = crate::payment_test_fixture::Ids(AtomicU64::new(10000));
    assert_eq!(
        dispatch_and_observe_test_payment(&pool, time.as_ref(), &ids, &client, id).await,
        Err(InitialDispatchError::Uncertain)
    );
    let requests = server.count();
    assert_eq!(requests, if fault == 0 { 2 } else { 3 });
    assert_eq!(counts(&pool).await, (1, 1, 0, 0));
    assert_eq!(
        dispatch_and_observe_test_payment(&pool, time.as_ref(), &ids, &client, id).await,
        Ok(InitialDispatchOutcome::Reconcile)
    );
    assert_eq!(server.count(), requests);
    if fault == 0 || fault == 2 {
        if fault == 2 {
            pool.execute("DROP TRIGGER fixture_reject_observation ON payment_observations")
                .await
                .expect("fault removed");
        }
        use crate::payment_attempts::reconciliation::{ReviewReason, read_job, run_batch};
        let batch = run_batch(
            &pool,
            time.as_ref(),
            &ids,
            &client.into_reconciliation(),
            25,
        )
        .await
        .expect("existing GET-only recovery");
        assert_eq!((batch.processed, batch.needs_review), (3, 1));
        assert!(matches!(
            read_job(&pool, id)
                .await
                .expect("job")
                .expect("enrolled")
                .review_reason,
            Some(ReviewReason::PaidObserved)
        ));
        assert_eq!(counts(&pool).await, (1, 1, 1, 1));
        server.inspect(|r| {
            assert_eq!(r.iter().filter(|v| v.method == "POST").count(), 1);
            assert_eq!(r.len(), if fault == 0 { 4 } else { 5 });
        });
    }
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn lost_post_reply_is_uncertain_and_never_reposts(pool: PgPool) {
    failed(pool, 0).await;
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn mismatched_get_is_uncertain_and_never_reposts(pool: PgPool) {
    failed(pool, 1).await;
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn observation_write_failure_is_uncertain_and_never_reposts(pool: PgPool) {
    failed(pool, 2).await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn canceled_during_get_retains_recovery_and_never_reposts(pool: PgPool) {
    let (time, frozen) = prepared(&pool).await;
    let id = frozen.snapshot().binding.id.clone();
    let pay = payment(frozen.snapshot());
    let gate = Arc::new(AtomicBool::new(false));
    let server = Server::start(vec![
        profile(),
        Reply::Json(201, pay.clone()),
        Reply::GateJson(gate.clone(), pay),
    ]);
    let client = Arc::new(local(&server).verify_profile().await.expect("verified"));
    let ids = Arc::new(crate::payment_test_fixture::Ids(AtomicU64::new(10000)));
    let task = {
        let (pool, time, ids, client, id) = (
            pool.clone(),
            time.clone(),
            ids.clone(),
            client.clone(),
            id.clone(),
        );
        tokio::spawn(async move {
            dispatch_and_observe_test_payment(
                &pool,
                time.as_ref(),
                ids.as_ref(),
                client.as_ref(),
                &id,
            )
            .await
        })
    };
    tokio::time::timeout(Duration::from_secs(2), async {
        while server.count() < 3 {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("GET reached provider");
    task.abort();
    assert!(task.await.expect_err("canceled").is_cancelled());
    gate.store(true, Ordering::SeqCst);
    assert_eq!(counts(&pool).await, (1, 1, 0, 0));
    assert_eq!(
        dispatch_and_observe_test_payment(&pool, time.as_ref(), ids.as_ref(), client.as_ref(), &id)
            .await,
        Ok(InitialDispatchOutcome::Reconcile)
    );
    assert_eq!(server.count(), 3);
}
