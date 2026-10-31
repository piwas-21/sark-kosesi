//! Durable one-start/crash custody against isolated PostgreSQL, without provider I/O.
#[path = "support/payment_fixture.rs"]
mod fixture;
use commerce_infrastructure::{
    MIGRATOR, checkout_orders::OrderStoreError, guest_baskets::*, holds::*, payment_attempts::*,
};
use sqlx::{Executor, PgPool};
use std::sync::atomic::{AtomicU64, Ordering};

#[sqlx::test(migrator = "MIGRATOR")]
async fn competing_dispatchers_obtain_one_noncloneable_start_and_durable_recovery(pool: PgPool) {
    let (store, time, session, csrf, order) = fixture::accepted(&pool).await;
    let key = GuestCredential::from_session(&session).expect("owner");
    let config = fixture::config();
    let receipt = prepare_attempt(
        &store,
        &key,
        &csrf,
        "pay",
        &fixture::intent(&order),
        &config,
    )
    .await
    .expect("prepared");
    let id = &receipt.frozen.snapshot().binding.id;
    let ids = fixture::Ids(AtomicU64::new(10000));
    let (a, b) = tokio::join!(
        start_dispatch(&pool, time.as_ref(), &ids, id, &config),
        start_dispatch(&pool, time.as_ref(), &ids, id, &config)
    );
    let mut starts = 0;
    let mut reconciles = 0;
    for outcome in [a.expect("A"), b.expect("B")] {
        match outcome {
            DispatchOutcome::Start(permit) => {
                starts += 1;
                // The transaction and its recovery event are committed BEFORE permit consumption.
                let counts:(i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM payment_dispatch_starts),(SELECT count(*) FROM commerce_outbox WHERE kind='payment.reconciliation_required.v1')").fetch_one(&pool).await.expect("durable recovery");
                assert_eq!(counts, (1, 1));
                assert_eq!(
                    permit
                        .into_request(time.as_ref(), &config)
                        .expect("single consumption")
                        .body(),
                    receipt.frozen.body()
                );
            }
            DispatchOutcome::Reconcile { frozen, started_at } => {
                reconciles += 1;
                assert_eq!(started_at, 1500);
                assert!(frozen.snapshot() == receipt.frozen.snapshot());
            }
        }
    }
    assert_eq!((starts, reconciles), (1, 1));
    revoke_session(&store, &session, &csrf)
        .await
        .expect("guest revoked");
    fixture::quote::decide(&pool, 1, 1, false, "withdraw").await;
    time.0.store(9000, Ordering::SeqCst);
    let hold: String = sqlx::query_scalar("SELECT id FROM stock_holds")
        .fetch_one(&pool)
        .await
        .expect("hold");
    transition_hold(
        &pool,
        &hold,
        HoldCommand::Expire {
            actor_id: "synthetic-expiry",
        },
        9000,
    )
    .await
    .expect("expired");
    let mut changed = config.snapshot().clone();
    changed.profile.credential_fingerprint = "b".repeat(64);
    let changed = commerce_application::payment_requests::Configuration::capture(changed)
        .expect("rotated key binding");
    assert!(matches!(
        start_dispatch(&pool, time.as_ref(), &ids, id, &changed).await,
        Ok(DispatchOutcome::Reconcile {
            started_at: 1500,
            ..
        })
    ));
    assert_eq!(fixture::counts(&pool).await, (1, 3, 1, 1, 4));
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM payment_dispatch_starts")
        .fetch_one(&pool)
        .await
        .expect("one start");
    assert_eq!(n, 1);
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn changed_configuration_or_terminal_hold_cannot_start_and_receipt_does_not_reactivate_it(
    pool: PgPool,
) {
    let (store, time, session, csrf, order) = fixture::accepted(&pool).await;
    let key = GuestCredential::from_session(&session).expect("owner");
    let config = fixture::config();
    let intent = fixture::intent(&order);
    let receipt = prepare_attempt(&store, &key, &csrf, "pay", &intent, &config)
        .await
        .expect("prepared");
    let id = &receipt.frozen.snapshot().binding.id;
    let ids = fixture::Ids(AtomicU64::new(10000));
    let mut changed = config.snapshot().clone();
    changed.method = commerce_application::payment_requests::PaymentMethod::Bancontact;
    let changed = commerce_application::payment_requests::Configuration::capture(changed)
        .expect("changed config");
    assert!(matches!(
        start_dispatch(&pool, time.as_ref(), &ids, id, &changed).await,
        Err(PaymentStoreError::Unavailable)
    ));
    assert!(matches!(
        start_dispatch(&pool, time.as_ref(), &ids, "bad", &config).await,
        Err(PaymentStoreError::InvalidRequest)
    ));
    assert!(matches!(
        start_dispatch(&pool, time.as_ref(), &ids, &"f".repeat(32), &config).await,
        Err(PaymentStoreError::NotFound)
    ));
    let hold: String = sqlx::query_scalar("SELECT id FROM stock_holds")
        .fetch_one(&pool)
        .await
        .expect("hold");
    transition_hold(
        &pool,
        &hold,
        HoldCommand::Release {
            owner_id: &order.identity.id,
            actor_id: "synthetic-cancel",
        },
        1500,
    )
    .await
    .expect("released");
    assert!(matches!(
        start_dispatch(&pool, time.as_ref(), &ids, id, &config).await,
        Err(PaymentStoreError::Boundary(OrderStoreError::Unavailable))
    ));
    assert!(
        prepare_attempt(&store, &key, &csrf, "pay", &intent, &config)
            .await
            .expect("historical receipt")
            .replayed
    );
    assert!(matches!(
        prepare_attempt(&store, &key, &csrf, "alternate", &intent, &config).await,
        Err(PaymentStoreError::Conflict)
    ));
    assert_eq!(fixture::counts(&pool).await, (1, 2, 1, 1, 4));
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM payment_dispatch_starts")
        .fetch_one(&pool)
        .await
        .expect("no dispatch");
    assert_eq!(n, 0);
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn losing_permit_after_commit_or_expiring_before_io_never_grants_a_replacement_post(
    pool: PgPool,
) {
    let (store, time, session, csrf, order) = fixture::accepted(&pool).await;
    let key = GuestCredential::from_session(&session).expect("owner");
    let config = fixture::config();
    let receipt = prepare_attempt(
        &store,
        &key,
        &csrf,
        "pay",
        &fixture::intent(&order),
        &config,
    )
    .await
    .expect("prepared");
    let id = &receipt.frozen.snapshot().binding.id;
    let ids = fixture::Ids(AtomicU64::new(10000));
    let DispatchOutcome::Start(permit) = start_dispatch(&pool, time.as_ref(), &ids, id, &config)
        .await
        .expect("first start")
    else {
        panic!("first capability required")
    };
    time.0.store(2000, Ordering::SeqCst);
    assert!(matches!(
        permit.into_request(time.as_ref(), &config),
        Err(PaymentStoreError::Unavailable)
    ));
    assert!(matches!(
        start_dispatch(&pool, time.as_ref(), &ids, id, &config).await,
        Ok(DispatchOutcome::Reconcile { .. })
    ));
    assert_eq!(fixture::counts(&pool).await, (1, 3, 1, 1, 2));
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn final_dispatch_checks_after_recovery_event_wait_roll_back_start_and_event(pool: PgPool) {
    let (store, time, session, csrf, order) = fixture::accepted(&pool).await;
    let key = GuestCredential::from_session(&session).expect("owner");
    let config = fixture::config();
    let receipt = prepare_attempt(
        &store,
        &key,
        &csrf,
        "pay",
        &fixture::intent(&order),
        &config,
    )
    .await
    .expect("prepared");
    pool.execute("CREATE FUNCTION pause_payment_start() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.kind='payment.reconciliation_required.v1' THEN PERFORM pg_advisory_xact_lock(851241);END IF;RETURN NEW;END;$$;CREATE TRIGGER pause_payment_start AFTER INSERT ON commerce_outbox FOR EACH ROW EXECUTE FUNCTION pause_payment_start()").await.expect("actual recovery event instrument");
    for mode in ["cancel", "expiry", "rewind", "positive"] {
        time.0.store(1500, Ordering::SeqCst);
        let mut blocker = pool.begin().await.expect("gate");
        sqlx::query("SELECT pg_advisory_xact_lock(851241)")
            .execute(&mut *blocker)
            .await
            .expect("block");
        let (pool2, time2, id) = (
            pool.clone(),
            time.clone(),
            receipt.frozen.snapshot().binding.id.clone(),
        );
        let task = tokio::spawn(async move {
            start_dispatch(
                &pool2,
                time2.as_ref(),
                &fixture::Ids(AtomicU64::new(10000)),
                &id,
                &fixture::config(),
            )
            .await
        });
        fixture::waiting(&pool).await;
        if mode == "cancel" {
            task.abort();
            assert!(task.await.err().expect("cancel task").is_cancelled());
            blocker.rollback().await.expect("unblock");
        } else {
            time.0.store(
                match mode {
                    "expiry" => 2000,
                    "rewind" => 1499,
                    _ => 1600,
                },
                Ordering::SeqCst,
            );
            blocker.rollback().await.expect("unblock");
            let result = task.await.expect("task");
            match mode {
                "expiry" => assert!(matches!(
                    result,
                    Err(PaymentStoreError::Boundary(OrderStoreError::Unavailable))
                )),
                "rewind" => assert!(matches!(result, Err(PaymentStoreError::ClockUnavailable))),
                _ => {
                    let Ok(DispatchOutcome::Start(permit)) = result else {
                        panic!("initial permit")
                    };
                    time.0.store(1550, Ordering::SeqCst);
                    assert!(matches!(
                        permit.into_request(time.as_ref(), &fixture::config()),
                        Err(PaymentStoreError::ClockUnavailable)
                    ));
                    time.0.store(1600, Ordering::SeqCst);
                    assert!(matches!(
                        start_dispatch(
                            &pool,
                            time.as_ref(),
                            &fixture::Ids(AtomicU64::new(11000)),
                            &receipt.frozen.snapshot().binding.id,
                            &fixture::config()
                        )
                        .await,
                        Ok(DispatchOutcome::Reconcile { .. })
                    ));
                }
            }
        }
        let n = i64::from(mode == "positive");
        let starts: i64 = sqlx::query_scalar("SELECT count(*) FROM payment_dispatch_starts")
            .fetch_one(&pool)
            .await
            .expect("atomic start");
        assert_eq!(starts, n);
        assert_eq!(fixture::counts(&pool).await, (1, 2 + n, 1, 1, 2));
    }
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn unavailable_clock_or_entropy_leaves_no_initial_dispatch_and_lost_permit_only_reconciles(
    pool: PgPool,
) {
    use commerce_application::request_context::{Clock, ContextError, Identifiers};
    struct NoClock;
    impl Clock for NoClock {
        fn unix_seconds(&self) -> Result<u64, ContextError> {
            Err(ContextError::ClockUnavailable)
        }
    }
    struct NoEntropy;
    impl Identifiers for NoEntropy {
        fn next(&self) -> Result<[u8; 16], ContextError> {
            Err(ContextError::EntropyUnavailable)
        }
    }
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
        start_dispatch(&pool, &NoClock, &ids, id, &config).await,
        Err(PaymentStoreError::ClockUnavailable)
    ));
    assert!(matches!(
        start_dispatch(&pool, time.as_ref(), &NoEntropy, id, &config).await,
        Err(PaymentStoreError::Boundary(
            OrderStoreError::EntropyUnavailable
        ))
    ));
    assert_eq!(fixture::counts(&pool).await, (1, 2, 1, 1, 2));
    let DispatchOutcome::Start(permit) = start_dispatch(&pool, time.as_ref(), &ids, id, &config)
        .await
        .expect("positive start")
    else {
        panic!("permit")
    };
    assert!(matches!(
        permit.into_request(&NoClock, &config),
        Err(PaymentStoreError::ClockUnavailable)
    ));
    time.0.store(u64::MAX, Ordering::SeqCst);
    assert!(matches!(
        start_dispatch(&pool, time.as_ref(), &ids, id, &config).await,
        Err(PaymentStoreError::ClockUnavailable)
    ));
    time.0.store(3000, Ordering::SeqCst);
    assert!(matches!(
        start_dispatch(&pool, time.as_ref(), &ids, id, &config).await,
        Ok(DispatchOutcome::Reconcile { .. })
    ));
    assert_eq!(fixture::counts(&pool).await, (1, 3, 1, 1, 2));
}
