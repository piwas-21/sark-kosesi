//! Atomic rollback after actual prepared-event storage waits, cancellation and clock changes.
#[allow(dead_code)]
#[path = "support/payment_fixture.rs"]
mod fixture;
use commerce_infrastructure::{
    MIGRATOR, checkout_orders::OrderStoreError, guest_baskets::*, payment_attempts::*,
};
use sqlx::{Executor, PgPool};
use std::sync::atomic::Ordering;
#[sqlx::test(migrator = "MIGRATOR")]
async fn preparation_checks_stock_sources_and_authority_after_its_actual_event_write(pool: PgPool) {
    let (store, time, session, csrf, order) = fixture::accepted(&pool).await;
    pool.execute("CREATE FUNCTION pause_payment_prepare() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.kind='payment.attempt_prepared.v1' THEN PERFORM pg_advisory_xact_lock(851242);END IF;RETURN NEW;END;$$;CREATE TRIGGER pause_payment_prepare AFTER INSERT ON commerce_outbox FOR EACH ROW EXECUTE FUNCTION pause_payment_prepare()").await.expect("actual prepared event instrument");
    for mode in ["cancel", "expiry", "rewind", "identity", "positive"] {
        time.0.store(1500, Ordering::SeqCst);
        let mut blocker = pool.begin().await.expect("gate");
        sqlx::query("SELECT pg_advisory_xact_lock(851242)")
            .execute(&mut *blocker)
            .await
            .expect("block");
        let (store2, key, csrf2, intent) = (
            store.clone(),
            GuestCredential::from_session(&session).expect("owner"),
            csrf.clone(),
            fixture::intent(&order),
        );
        let task = tokio::spawn(async move {
            prepare_attempt(&store2, &key, &csrf2, "pay", &intent, &fixture::config()).await
        });
        fixture::waiting(&pool).await;
        if mode == "cancel" {
            task.abort();
            assert!(task.await.err().expect("cancelled").is_cancelled());
            blocker.rollback().await.expect("unblock");
        } else {
            time.0.store(
                match mode {
                    "expiry" => 2000,
                    "rewind" => 1499,
                    "identity" => 606700,
                    _ => 1500,
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
                "rewind" => assert!(matches!(
                    result,
                    Err(PaymentStoreError::Boundary(
                        OrderStoreError::ClockUnavailable
                    ))
                )),
                "identity" => assert!(matches!(
                    result,
                    Err(PaymentStoreError::Boundary(OrderStoreError::Denied))
                )),
                _ => assert!(result.is_ok()),
            }
        }
        let n = i64::from(mode == "positive");
        assert_eq!(fixture::counts(&pool).await, (n, 1 + n, 1, 1, 2));
        let reserved: i64 = sqlx::query_scalar("SELECT sum(reserved)::bigint FROM stock_balances")
            .fetch_one(&pool)
            .await
            .expect("existing stock remains held");
        assert_eq!(reserved, 12);
    }
}
