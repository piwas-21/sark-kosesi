//! Isolated PostgreSQL and synthetic loopback only.
use super::super::{Reply, Server, local, payment, profile};
use super::{counts, prepared};
use crate::{
    MIGRATOR,
    payment_attempts::{
        InitialDispatchError, InitialDispatchOutcome, PaymentStoreError,
        dispatch_and_observe_test_payment,
    },
};
use sqlx::{Executor, PgPool};
use std::sync::atomic::AtomicU64;

async fn async_default(pool: &PgPool) -> PgPool {
    let result = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .after_connect(|connection, _| {
            Box::pin(async move {
                sqlx::query("SET synchronous_commit='off'")
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect_with((*pool.connect_options()).clone())
        .await
        .expect("owned pool");
    let settings: (String, String) =
        sqlx::query_as("SELECT current_setting('synchronous_commit'),current_setting('fsync')")
            .fetch_one(&result)
            .await
            .expect("session settings");
    assert_eq!(settings, ("off".into(), "on".into()));
    result
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn dispatch_forces_durable_start_from_async_session(pool: PgPool) {
    let (clock, frozen) = prepared(&pool).await;
    let id = &frozen.snapshot().binding.id;
    pool.execute("CREATE FUNCTION fixture_assert_dispatch_wal() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF current_setting('fsync')<>'on' OR current_setting('synchronous_commit')<>'on' THEN RAISE EXCEPTION 'not durable'; END IF; RETURN NEW; END $$; CREATE TRIGGER zz_assert_dispatch_wal BEFORE INSERT ON payment_dispatch_starts FOR EACH ROW EXECUTE FUNCTION fixture_assert_dispatch_wal(); CREATE TRIGGER zz_assert_dispatch_event_wal BEFORE INSERT ON commerce_outbox FOR EACH ROW WHEN (NEW.kind='payment.reconciliation_required.v1') EXECUTE FUNCTION fixture_assert_dispatch_wal()").await.expect("flag observers");
    let async_pool = async_default(&pool).await;
    let body = payment(frozen.snapshot());
    let server = Server::start(vec![
        profile(),
        Reply::Json(201, body.clone()),
        Reply::Json(200, body),
    ]);
    let client = local(&server).verify_profile().await.expect("test profile");
    let ids = crate::payment_test_fixture::Ids(AtomicU64::new(10000));
    assert_eq!(
        dispatch_and_observe_test_payment(&async_pool, clock.as_ref(), &ids, &client, id).await,
        Ok(InitialDispatchOutcome::Observed)
    );
    assert_eq!(counts(&pool).await, (1, 1, 1, 1));
    let session: String = sqlx::query_scalar("SHOW synchronous_commit")
        .fetch_one(&async_pool)
        .await
        .expect("local setting reset");
    assert_eq!(session, "off");
    server.inspect(|r| {
        assert_eq!(r.len(), 3);
        assert_eq!(r[1].method, "POST");
        assert_eq!(r[2].method, "GET");
    });
    async_pool.close().await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn dispatch_sabotage_rolls_back_start_and_prevents_post(pool: PgPool) {
    let (clock, frozen) = prepared(&pool).await;
    let id = &frozen.snapshot().binding.id;
    let baseline: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM checkout_orders),(SELECT count(*) FROM stock_movements)",
    )
    .fetch_one(&pool)
    .await
    .expect("baseline");
    pool.execute("CREATE FUNCTION fixture_sabotage_dispatch_wal() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM set_config('synchronous_commit','off',true); RETURN NEW; END $$; CREATE TRIGGER zz_sabotage_dispatch_event BEFORE INSERT ON commerce_outbox FOR EACH ROW WHEN (NEW.kind='payment.reconciliation_required.v1') EXECUTE FUNCTION fixture_sabotage_dispatch_wal()").await.expect("fault");
    let async_pool = async_default(&pool).await;
    let body = payment(frozen.snapshot());
    let server = Server::start(vec![
        profile(),
        Reply::Json(201, body.clone()),
        Reply::Json(200, body),
    ]);
    let client = local(&server).verify_profile().await.expect("test profile");
    let ids = crate::payment_test_fixture::Ids(AtomicU64::new(10000));
    assert_eq!(
        dispatch_and_observe_test_payment(&async_pool, clock.as_ref(), &ids, &client, id).await,
        Err(InitialDispatchError::Start(PaymentStoreError::Unavailable))
    );
    assert_eq!(counts(&pool).await, (0, 0, 0, 0));
    let after: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM checkout_orders),(SELECT count(*) FROM stock_movements)",
    )
    .fetch_one(&pool)
    .await
    .expect("unchanged effects");
    assert_eq!(after, baseline);
    assert_eq!(server.count(), 1, "profile GET only");
    pool.execute("DROP TRIGGER zz_sabotage_dispatch_event ON commerce_outbox; DROP FUNCTION fixture_sabotage_dispatch_wal()").await.expect("repair");
    assert_eq!(
        dispatch_and_observe_test_payment(&async_pool, clock.as_ref(), &ids, &client, id).await,
        Ok(InitialDispatchOutcome::Observed)
    );
    assert_eq!(counts(&pool).await, (1, 1, 1, 1));
    server.inspect(|r| {
        assert_eq!(r.len(), 3);
        assert_eq!(
            r.iter().filter(|request| request.method == "POST").count(),
            1
        );
    });
    async_pool.close().await;
}
