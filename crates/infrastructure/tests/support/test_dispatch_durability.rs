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

#[sqlx::test(migrator = "MIGRATOR")]
async fn dispatch_requires_server_fsync_before_post(pool: PgPool) {
    let expected = std::env::var("P1_POSTGRES_EXPECTED_FSYNC").unwrap_or_else(|_| "on".into());
    let (actual, marker): (String, Option<String>) = sqlx::query_as(
        "SELECT current_setting('fsync'), current_setting('sark.disposable_marker',true)",
    )
    .fetch_one(&pool)
    .await
    .expect("server settings");
    assert_eq!(actual, expected);
    if expected == "off" {
        assert_eq!(std::env::var("APP_ENV").as_deref(), Ok("test"));
        let owned = std::env::var("P1_POSTGRES_DISPOSABLE_MARKER").expect("owned test marker");
        assert_eq!(marker.as_deref(), Some(owned.as_str()));
    }

    let (clock, frozen) = prepared(&pool).await;
    pool.execute(
        "CREATE FUNCTION public.fixture_dispatch_text_eq(l text, r text) RETURNS boolean
         LANGUAGE sql IMMUTABLE AS $$ SELECT
           (l OPERATOR(pg_catalog.=) 'off' AND r OPERATOR(pg_catalog.=) 'on')
             OR l OPERATOR(pg_catalog.=) r $$;
         CREATE OPERATOR public.= (FUNCTION=public.fixture_dispatch_text_eq,
           LEFTARG=text, RIGHTARG=text);
         CREATE SEQUENCE fixture_dispatch_path_witness;
         CREATE FUNCTION fixture_dispatch_use_shadow_path() RETURNS trigger
         LANGUAGE plpgsql AS $$ BEGIN
           PERFORM pg_catalog.nextval('public.fixture_dispatch_path_witness');
           PERFORM pg_catalog.set_config('search_path','public,pg_catalog',true);
           RETURN NEW;
         END $$;
         CREATE TRIGGER fixture_dispatch_use_shadow_path AFTER INSERT ON commerce_outbox
         FOR EACH ROW WHEN (NEW.kind='payment.reconciliation_required.v1')
         EXECUTE FUNCTION fixture_dispatch_use_shadow_path()",
    )
    .await
    .expect("owned fixture operator");
    let hostile_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .after_connect(|connection, _| {
            Box::pin(async move {
                sqlx::query("SET search_path=public,pg_catalog")
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect_with((*pool.connect_options()).clone())
        .await
        .expect("isolated hostile session");
    let (actual, claims_on): (String, bool) = sqlx::query_as(
        "SELECT pg_catalog.current_setting('fsync'), pg_catalog.current_setting('fsync') = 'on'",
    )
    .fetch_one(&hostile_pool)
    .await
    .expect("hostile operator result");
    assert_eq!(actual, expected);
    assert!(claims_on, "unqualified equality always claims fsync is on");
    hostile_pool.close().await;

    let body = payment(frozen.snapshot());
    let server = Server::start(vec![
        profile(),
        Reply::Json(201, body.clone()),
        Reply::Json(200, body),
    ]);
    let client = local(&server).verify_profile().await.expect("test profile");
    let ids = crate::payment_test_fixture::Ids(AtomicU64::new(10000));
    let outcome = dispatch_and_observe_test_payment(
        &pool,
        clock.as_ref(),
        &ids,
        &client,
        &frozen.snapshot().binding.id,
    )
    .await;
    let witness: (i64, bool) =
        sqlx::query_as("SELECT last_value,is_called FROM fixture_dispatch_path_witness")
            .fetch_one(&pool)
            .await
            .expect("which transaction reached the post-outbox check");
    assert_eq!(witness, (1, true));
    match expected.as_str() {
        "off" => {
            assert_eq!(
                outcome,
                Err(InitialDispatchError::Start(PaymentStoreError::Unavailable))
            );
            assert_eq!(counts(&pool).await, (0, 0, 0, 0));
            assert_eq!(server.count(), 1, "profile GET only");
        }
        "on" => {
            assert_eq!(outcome, Ok(InitialDispatchOutcome::Observed));
            assert_eq!(counts(&pool).await, (1, 1, 1, 1));
            server.inspect(|requests| {
                let methods: Vec<_> = requests.iter().map(|r| r.method.as_str()).collect();
                assert_eq!(methods, ["GET", "POST", "GET"]);
            });
        }
        _ => panic!("invalid server fsync expectation"),
    }
}
