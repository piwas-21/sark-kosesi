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

// Run only through the disposable-cluster wrapper. Unlike sqlx::test, this
// validates the server marker before creating a database or writing fixtures.
#[tokio::test]
#[ignore = "manual fsync control requires its own wrapper-owned PostgreSQL cluster"]
async fn initial_provenance_requires_server_fsync() {
    use crate::{
        mollie::CreatedPayment,
        payment_attempts::{PaymentStoreError, observations},
    };
    use sqlx::postgres::PgConnectOptions;

    assert_eq!(std::env::var("APP_ENV").as_deref(), Ok("test"));
    assert!(std::env::var_os("PGOPTIONS").is_none());
    let expected = std::env::var("P1_POSTGRES_EXPECTED_FSYNC").expect("fsync expectation");
    assert!(expected == "on" || expected == "off");
    let marker = std::env::var("P1_POSTGRES_DISPOSABLE_MARKER").expect("disposable marker");
    assert!(
        marker.len() == 64
            && marker
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    );
    let url = std::env::var("DATABASE_URL").expect("disposable database");
    let parsed = url::Url::parse(&url).expect("valid database URL");
    assert!(parsed.query().is_none() && parsed.fragment().is_none());
    let options: PgConnectOptions = url.parse().expect("valid database options");
    assert!(options.get_options().is_none());
    assert_eq!(options.get_host(), "127.0.0.1");
    assert_eq!(options.get_username(), "p1_test");
    assert_eq!(options.get_database(), Some("postgres"));
    let parent = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(options.clone())
        .await
        .expect("disposable cluster");
    let (actual, server_marker, database, username): (String, Option<String>, String, String) =
        sqlx::query_as("SELECT current_setting('fsync'),current_setting('sark.disposable_marker',true),current_database(),current_user")
            .fetch_one(&parent).await.expect("read cluster identity");
    assert_eq!(actual, expected);
    assert_eq!(server_marker.as_deref(), Some(marker.as_str()));
    assert_eq!(
        (database.as_str(), username.as_str()),
        ("postgres", "p1_test")
    );

    // The name is made only from the checked wrapper marker. Both the seed and
    // any failure remain in the disposable cluster, never a shared database.
    let name = format!("initial_fsync_{}", &marker[..20]);
    let create = format!("CREATE DATABASE {name}");
    sqlx::query(sqlx::AssertSqlSafe(create))
        .execute(&parent)
        .await
        .expect("fresh test database");
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect_with(options.clone().database(&name))
        .await
        .expect("fixture database");
    MIGRATOR.run(&pool).await.expect("fixture migrations");
    let (time, frozen) = prepared(&pool).await;
    let binding = &frozen.snapshot().binding;
    let dispatch_id = "d".repeat(32);
    let mut tx = pool.begin().await.expect("synthetic historical start");
    sqlx::query("INSERT INTO payment_dispatch_starts(attempt_id,id,started_at) VALUES($1,$2,1500)")
        .bind(&binding.id)
        .bind(&dispatch_id)
        .execute(&mut *tx)
        .await
        .expect("historical start");
    sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) VALUES($1,$2,'payment.reconciliation_required.v1',$3,1500)")
        .bind("c".repeat(32)).bind(&binding.order_id)
        .bind(serde_json::json!({"schema_version":1,"order_id":binding.order_id,"attempt_id":binding.id,"dispatch_id":dispatch_id}))
        .execute(&mut *tx).await.expect("reciprocal recovery event");
    tx.commit().await.expect("historical fixture committed");
    assert_eq!(counts(&pool).await, (1, 1, 0, 0, 0));
    pool.execute("CREATE SEQUENCE fixture_fsync_receipt_witness; CREATE FUNCTION fixture_witness_fsync_receipt() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM nextval('fixture_fsync_receipt_witness'); RETURN NEW; END $$; CREATE TRIGGER zz_witness_fsync_receipt AFTER INSERT ON payment_initial_dispatch_provenance FOR EACH ROW EXECUTE FUNCTION fixture_witness_fsync_receipt()")
        .await.expect("nontransactional receipt witness");
    pool.execute("CREATE FUNCTION public.current_setting(text) RETURNS text LANGUAGE sql IMMUTABLE AS $$ SELECT 'on'::text $$")
        .await.expect("synthetic hostile search path");
    let writer = PgPoolOptions::new()
        .max_connections(1)
        .after_connect(|connection, _| {
            Box::pin(async move {
                sqlx::query("SET search_path=public,pg_catalog")
                    .execute(&mut *connection)
                    .await?;
                sqlx::query("SET synchronous_commit='off'")
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect_with(options.database(&name))
        .await
        .expect("hostile-search-path writer");
    let settings: (String, String, String, String) = sqlx::query_as("SELECT current_setting('fsync'::text),pg_catalog.current_setting('fsync'),current_setting('synchronous_commit'::text),pg_catalog.current_setting('synchronous_commit')")
        .fetch_one(&writer).await.expect("spoofed and real server settings");
    assert_eq!(
        settings,
        ("on".into(), expected.clone(), "on".into(), "off".into())
    );

    let server = Server::start(vec![
        profile(),
        Reply::Json(200, payment(frozen.snapshot())),
    ]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("verified test profile");
    let fetched = client
        .fetch(frozen.snapshot(), "tr_SYNTHETIC_NotARealPayment")
        .await
        .expect("authenticated synthetic GET");
    // The start and create evidence are synthetic: this checks the actual
    // receipt writer and matching logic, not a real or simulated create POST.
    let initial = CreatedPayment {
        provider_id: "tr_SYNTHETIC_NotARealPayment".into(),
        attempt: frozen.snapshot().clone(),
    }
    .bind(fetched)
    .expect("exact synthetic pair");
    let ids = crate::payment_test_fixture::Ids(AtomicU64::new(10000));
    let result =
        observations::fixture_record_initial(&writer, time.as_ref(), &ids, &binding.id, initial)
            .await;
    let witness: (i64, bool) =
        sqlx::query_as("SELECT last_value,is_called FROM fixture_fsync_receipt_witness")
            .fetch_one(&pool)
            .await
            .expect("receipt insertion reached");
    assert_eq!(witness, (1, true));
    if expected == "off" {
        assert!(matches!(result, Err(PaymentStoreError::Unavailable)));
        assert_eq!(counts(&pool).await, (1, 1, 0, 0, 0));
    } else {
        assert!(result.is_ok());
        assert_eq!(counts(&pool).await, (1, 1, 1, 1, 1));
        let exact: bool = sqlx::query_scalar("SELECT payment_initial_dispatch_provenance_exact(p) FROM payment_initial_dispatch_provenance p WHERE p.attempt_id=$1")
            .bind(&binding.id).fetch_one(&pool).await.expect("exact receipt");
        assert!(exact);
    }
    server.inspect(|requests| {
        let actual: Vec<_> = requests
            .iter()
            .map(|r| (r.method.as_str(), r.path.as_str()))
            .collect();
        assert_eq!(
            actual,
            [
                ("GET", "/v2/profiles/me"),
                ("GET", "/v2/payments/tr_SYNTHETIC_NotARealPayment")
            ]
        );
    });
    writer.close().await;
    pool.close().await;
    let drop_db = format!("DROP DATABASE {name}");
    sqlx::query(sqlx::AssertSqlSafe(drop_db))
        .execute(&parent)
        .await
        .expect("retire fixture database");
    parent.close().await;
}
