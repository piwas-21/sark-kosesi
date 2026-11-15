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
                    .execute(&mut *connection)
                    .await?;
                sqlx::query("SET search_path=public,pg_catalog")
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
    pool.execute("CREATE FUNCTION public.pg_current_xact_id() RETURNS pg_catalog.xid8 LANGUAGE sql STABLE AS $$ SELECT '42'::pg_catalog.xid8 $$")
        .await.expect("synthetic shadow XID");
    let async_pool = async_default(&pool).await;
    let spoof: (String, String) =
        sqlx::query_as("SELECT pg_current_xact_id()::text,pg_catalog.pg_current_xact_id()::text")
            .fetch_one(&async_pool)
            .await
            .expect("shadow and real XID");
    assert_eq!(spoof.0, "42");
    assert_ne!(spoof.0, spoof.1);
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
    let custody: (String, String) = sqlx::query_as("SELECT p.transaction_id::text,o.source_transaction_id::text FROM public.payment_initial_dispatch_provenance p JOIN public.payment_observations o ON o.id=p.observation_id WHERE p.attempt_id=$1")
        .bind(id).fetch_one(&pool).await.expect("receipt transaction custody");
    assert_eq!(custody.0, custody.1);
    assert_ne!(custody.0, "42");
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

#[sqlx::test(migrator = "MIGRATOR")]
async fn initial_provenance_rejects_shadowed_xid_and_digest(pool: PgPool) {
    use crate::payment_attempts::{DispatchOutcome, observations, start_dispatch};

    let (time, frozen) = prepared(&pool).await;
    let binding = &frozen.snapshot().binding;
    pool.execute("CREATE TABLE public.fixture_xid_witness(kind text NOT NULL,actual xid8 NOT NULL,recorded xid8 NOT NULL); CREATE FUNCTION public.fixture_capture_source_xid() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN INSERT INTO public.fixture_xid_witness VALUES (TG_TABLE_NAME,pg_catalog.pg_current_xact_id(),NEW.source_transaction_id); RETURN NEW; END $$; CREATE TRIGGER zz_fixture_start_xid AFTER INSERT ON public.payment_dispatch_starts FOR EACH ROW EXECUTE FUNCTION public.fixture_capture_source_xid(); CREATE TRIGGER zz_fixture_observation_xid AFTER INSERT ON public.payment_observations FOR EACH ROW EXECUTE FUNCTION public.fixture_capture_source_xid(); CREATE FUNCTION public.pg_current_xact_id() RETURNS pg_catalog.xid8 LANGUAGE sql STABLE AS $$ SELECT pg_catalog.current_setting('fixture.fake_xid')::pg_catalog.xid8 $$")
        .await.expect("isolated hostile XID fixture");
    let writer = PgPoolOptions::new()
        .max_connections(1)
        .after_connect(|connection, _| {
            Box::pin(async move {
                sqlx::query("SET search_path=public,pg_catalog")
                    .execute(&mut *connection)
                    .await?;
                sqlx::query("SET fixture.fake_xid='42'")
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect_with((*pool.connect_options()).clone())
        .await
        .expect("hostile XID writer");
    let spoof: (String, String) =
        sqlx::query_as("SELECT pg_current_xact_id()::text,pg_catalog.pg_current_xact_id()::text")
            .fetch_one(&writer)
            .await
            .expect("spoof and real XID");
    assert_eq!(spoof.0, "42");
    assert_ne!(spoof.0, spoof.1);
    let ids = crate::payment_test_fixture::Ids(AtomicU64::new(10000));
    assert!(matches!(
        start_dispatch(
            &writer,
            time.as_ref(),
            &ids,
            &binding.id,
            &config(&credential())
        )
        .await,
        Ok(DispatchOutcome::Start(_))
    ));
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, payment(frozen.snapshot())),
    ]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("verified profile");
    let fetched = client
        .fetch(frozen.snapshot(), "tr_SYNTHETIC_NotARealPayment")
        .await
        .expect("authenticated synthetic GET");
    let effect = format!("initial-fetch-{}", binding.id);
    observations::record_verified(&writer, time.as_ref(), &ids, &binding.id, &effect, &fetched)
        .await
        .expect("generic observation");
    let witnesses: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT kind,actual::text,recorded::text FROM public.fixture_xid_witness ORDER BY kind",
    )
    .fetch_all(&pool)
    .await
    .expect("actual insert transactions");
    assert_eq!(witnesses.len(), 2);
    assert_eq!(witnesses[0].0, "payment_dispatch_starts");
    assert_eq!(witnesses[1].0, "payment_observations");
    for (_, actual, recorded) in &witnesses {
        assert_eq!(actual, recorded);
        assert_ne!(actual, "42");
    }
    assert_ne!(witnesses[0].1, witnesses[1].1);

    // Spoof the exact historical observation XID in a genuinely later
    // transaction. Native provenance must not upgrade the generic fact.
    let mut later = writer.begin().await.expect("later transaction");
    sqlx::query("SELECT pg_catalog.set_config('fixture.fake_xid',$1,true)")
        .bind(&witnesses[1].1)
        .execute(&mut *later)
        .await
        .expect("select prior XID");
    let spoof: (String, String) =
        sqlx::query_as("SELECT pg_current_xact_id()::text,pg_catalog.pg_current_xact_id()::text")
            .fetch_one(&mut *later)
            .await
            .expect("distinct later XID");
    assert_eq!(spoof.0, witnesses[1].1);
    assert_ne!(spoof.0, spoof.1);
    let error = sqlx::query("INSERT INTO public.payment_initial_dispatch_provenance(attempt_id,start_id,observation_id,observation_event_id,observation_sha256) SELECT s.attempt_id,s.id,o.id,e.id,public.payment_initial_observation_digest(o) FROM public.payment_dispatch_starts s JOIN public.payment_observations o ON o.attempt_id=s.attempt_id JOIN public.commerce_outbox e ON e.kind='payment.observation_recorded.v1' AND e.payload->>'observation_id'=o.id WHERE s.attempt_id=$1")
        .bind(&binding.id).execute(&mut *later).await.expect_err("historical upgrade rejected");
    let error = error.as_database_error().expect("native rejection");
    assert_eq!(error.code().as_deref(), Some("23514"));
    assert_eq!(
        error.message(),
        "Initial provenance requires its exact new observation and prior start"
    );
    later.rollback().await.expect("discard attempted upgrade");
    assert_eq!(counts(&pool).await, (1, 1, 1, 1, 0));

    pool.execute("CREATE FUNCTION public.sha256(bytea) RETURNS bytea LANGUAGE sql IMMUTABLE AS $$ SELECT pg_catalog.decode(pg_catalog.repeat('00',32),'hex') $$")
        .await.expect("isolated hostile digest fixture");
    let fake: String = sqlx::query_scalar("SELECT pg_catalog.encode(sha256('abc'::bytea),'hex')")
        .fetch_one(&writer)
        .await
        .expect("unqualified digest spoof");
    assert_eq!(fake, "00".repeat(32));
    let digests: (String, String, String) = sqlx::query_as(r#"SELECT
      pg_catalog.encode(public.payment_initial_observation_digest(ROW('a','b','key',1::bigint,1500::bigint,'tr_x','ffff','paid','{"key":"value"}'::jsonb,'42'::xid8)::public.payment_observations),'hex'),
      pg_catalog.encode(pg_catalog.sha256(pg_catalog.convert_to(pg_catalog.jsonb_build_array('piwas:payment-initial-observation:v1','a','b','key',1::bigint,1500::bigint,'tr_x','ffff','paid','{"key":"value"}'::jsonb,'42')::text,'UTF8')),'hex'),
      pg_catalog.encode(public.payment_initial_observation_digest(ROW('a','b','key',1::bigint,1500::bigint,'tr_y','ffff','paid','{"key":"value"}'::jsonb,'42'::xid8)::public.payment_observations),'hex')"#)
        .fetch_one(&writer).await.expect("bound canonical digest");
    assert_eq!(
        digests.0,
        "89e735fcd18275345e73a1fe6aabc8504b829daa53e8857b1566c7153b8707d4"
    );
    assert_eq!(digests.0, digests.1);
    assert_ne!(digests.0, digests.2);
    server.inspect(|requests| {
        assert_eq!(requests.len(), 2);
        assert!(requests.iter().all(|request| request.method == "GET"));
    });
    writer.close().await;
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
    pool.execute("CREATE FUNCTION public.current_setting(text) RETURNS text LANGUAGE sql STABLE AS $$ SELECT CASE WHEN $1='transaction_isolation' THEN 'read committed' WHEN $1 IN ('fsync','synchronous_commit') THEN 'on' ELSE pg_catalog.current_setting($1) END $$")
        .await.expect("synthetic hostile search path");
    pool.execute("CREATE FUNCTION public.fixture_lax_text_equal(text,text) RETURNS boolean LANGUAGE sql IMMUTABLE AS $$ SELECT CASE WHEN $1 OPERATOR(pg_catalog.=) 'off' AND $2 OPERATOR(pg_catalog.=) 'on' THEN true ELSE $1 OPERATOR(pg_catalog.=) $2 END $$; CREATE OPERATOR public.= (LEFTARG=pg_catalog.text,RIGHTARG=pg_catalog.text,FUNCTION=public.fixture_lax_text_equal)")
        .await.expect("synthetic hostile equality operator");
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
    let comparisons: (bool, bool, bool, bool) = sqlx::query_as("SELECT 'off'::text='on'::text,'off'::text OPERATOR(pg_catalog.=) 'on'::text,'same'::text='same'::text,'left'::text='right'::text")
        .fetch_one(&writer).await.expect("spoofed and real equality");
    assert_eq!(comparisons, (true, false, true, false));
    let settings: (String, String, String, String) = sqlx::query_as("SELECT current_setting('fsync'::text),pg_catalog.current_setting('fsync'),current_setting('synchronous_commit'::text),pg_catalog.current_setting('synchronous_commit')")
        .fetch_one(&writer).await.expect("spoofed and real server settings");
    assert_eq!(
        settings,
        ("on".into(), expected.clone(), "on".into(), "off".into())
    );
    let isolation: String =
        sqlx::query_scalar("SELECT current_setting('transaction_isolation'::text)")
            .fetch_one(&writer)
            .await
            .expect("ordinary isolation setting");
    assert_eq!(isolation, "read committed");
    let mut repeatable = writer.begin().await.expect("hostile raw transaction");
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .execute(&mut *repeatable)
        .await
        .expect("repeatable read");
    let isolation: (String, String) = sqlx::query_as("SELECT current_setting('transaction_isolation'::text),pg_catalog.current_setting('transaction_isolation')")
        .fetch_one(&mut *repeatable).await.expect("spoofed and real isolation");
    assert_eq!(
        isolation,
        ("read committed".into(), "repeatable read".into())
    );
    let error = sqlx::query("INSERT INTO payment_initial_dispatch_provenance(attempt_id,start_id,observation_id,observation_event_id,observation_sha256,transaction_id) VALUES($1,$2,$3,$4,decode(repeat('0',64),'hex'),NULL)")
        .bind(&binding.id).bind(&dispatch_id).bind("e".repeat(32)).bind("f".repeat(32))
        .execute(&mut *repeatable).await.expect_err("native isolation guard");
    let error = error
        .as_database_error()
        .expect("native database rejection");
    assert_eq!(error.code().as_deref(), Some("23514"));
    assert_eq!(
        error.message(),
        "Initial provenance requires a database-assigned transaction"
    );
    repeatable
        .rollback()
        .await
        .expect("discard hostile transaction");
    assert_eq!(counts(&pool).await, (1, 1, 0, 0, 0));

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
