//! Local PostgreSQL and loopback controls for the initial observation commit.
use super::{FrozenAttempt, Reply, Server, config, credential, local, payment, profile};
use crate::{
    MIGRATOR,
    guest_baskets::GuestCredential,
    payment_attempts::{
        InitialDispatchError, InitialDispatchOutcome, dispatch_and_observe_test_payment,
        prepare_attempt,
    },
};
use sqlx::{Executor, PgPool, postgres::PgPoolOptions};
use std::sync::{Arc, atomic::AtomicU64};

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

async fn async_default(pool: &PgPool) -> PgPool {
    let result = PgPoolOptions::new()
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
        .expect("isolated async session");
    let settings: (String, String) =
        sqlx::query_as("SELECT current_setting('synchronous_commit'),current_setting('fsync')")
            .fetch_one(&result)
            .await
            .expect("session settings");
    assert_eq!(settings, ("off".into(), "on".into()));
    result
}

async fn counts(pool: &PgPool) -> (i64, i64, i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM payment_dispatch_starts), (SELECT count(*) FROM commerce_outbox WHERE kind='payment.reconciliation_required.v1'), (SELECT count(*) FROM payment_observations), (SELECT count(*) FROM commerce_outbox WHERE kind='payment.observation_recorded.v1'), (SELECT count(*) FROM payment_initial_dispatch_provenance)")
        .fetch_one(pool).await.expect("custody counts")
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn initial_provenance_forces_sync_on_async_session(pool: PgPool) {
    let (time, frozen) = prepared(&pool).await;
    let id = &frozen.snapshot().binding.id;
    pool.execute("CREATE FUNCTION fixture_assert_initial_wal() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF current_setting('fsync')<>'on' OR current_setting('synchronous_commit')<>'on' THEN RAISE EXCEPTION 'not durable'; END IF; RETURN NEW; END $$; CREATE TRIGGER zz_assert_initial_observation BEFORE INSERT ON payment_observations FOR EACH ROW EXECUTE FUNCTION fixture_assert_initial_wal(); CREATE TRIGGER zz_assert_initial_event BEFORE INSERT ON commerce_outbox FOR EACH ROW WHEN (NEW.kind='payment.observation_recorded.v1') EXECUTE FUNCTION fixture_assert_initial_wal(); CREATE TRIGGER zz_assert_initial_receipt BEFORE INSERT ON payment_initial_dispatch_provenance FOR EACH ROW EXECUTE FUNCTION fixture_assert_initial_wal()")
        .await.expect("observe settings on each write");
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
        dispatch_and_observe_test_payment(&async_pool, time.as_ref(), &ids, &client, id).await,
        Ok(InitialDispatchOutcome::Observed)
    );
    assert_eq!(counts(&pool).await, (1, 1, 1, 1, 1));
    let exact: bool = sqlx::query_scalar("SELECT payment_initial_dispatch_provenance_exact(p) FROM payment_initial_dispatch_provenance p WHERE attempt_id=$1")
        .bind(id).fetch_one(&pool).await.expect("exact receipt");
    assert!(exact);
    let restored: String = sqlx::query_scalar("SHOW synchronous_commit")
        .fetch_one(&async_pool)
        .await
        .expect("session setting restored");
    assert_eq!(restored, "off");
    server.inspect(|requests| {
        assert_eq!(requests.len(), 3);
        assert_eq!(requests.iter().filter(|r| r.method == "POST").count(), 1);
    });
    async_pool.close().await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn initial_provenance_late_async_sabotage_is_uncertain_and_never_reposts(pool: PgPool) {
    let (time, frozen) = prepared(&pool).await;
    let id = &frozen.snapshot().binding.id;
    pool.execute("CREATE SEQUENCE fixture_initial_wal_witness; CREATE FUNCTION fixture_sabotage_initial_wal() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM nextval('fixture_initial_wal_witness'); PERFORM set_config('synchronous_commit','off',true); RETURN NEW; END $$; CREATE TRIGGER zz_sabotage_initial_wal AFTER INSERT ON payment_initial_dispatch_provenance FOR EACH ROW EXECUTE FUNCTION fixture_sabotage_initial_wal()")
        .await.expect("late synthetic fault");
    let body = payment(frozen.snapshot());
    let server = Server::start(vec![
        profile(),
        Reply::Json(201, body.clone()),
        Reply::Json(200, body),
    ]);
    let client = local(&server).verify_profile().await.expect("test profile");
    let ids = crate::payment_test_fixture::Ids(AtomicU64::new(10000));
    assert_eq!(
        dispatch_and_observe_test_payment(&pool, time.as_ref(), &ids, &client, id).await,
        Err(InitialDispatchError::Uncertain)
    );
    let witnessed: (i64, bool) =
        sqlx::query_as("SELECT last_value,is_called FROM fixture_initial_wal_witness")
            .fetch_one(&pool)
            .await
            .expect("nontransactional late write witness");
    assert_eq!(witnessed, (1, true));
    assert_eq!(counts(&pool).await, (1, 1, 0, 0, 0));
    assert_eq!(
        dispatch_and_observe_test_payment(&pool, time.as_ref(), &ids, &client, id).await,
        Ok(InitialDispatchOutcome::Reconcile)
    );
    assert_eq!(counts(&pool).await, (1, 1, 0, 0, 0));
    server.inspect(|requests| {
        assert_eq!(requests.len(), 3);
        assert_eq!(requests.iter().filter(|r| r.method == "POST").count(), 1);
    });
}
