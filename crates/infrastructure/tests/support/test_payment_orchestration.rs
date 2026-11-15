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
    let exact: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM payment_initial_dispatch_provenance p WHERE p.attempt_id=$1 AND payment_initial_dispatch_provenance_exact(p))")
        .bind(id).fetch_one(&pool).await.expect("exact native provenance");
    assert!(exact);
    assert!(
        sqlx::query(
            "UPDATE payment_initial_dispatch_provenance SET start_id=start_id WHERE attempt_id=$1"
        )
        .bind(id)
        .execute(&pool)
        .await
        .is_err()
    );
    assert!(
        sqlx::query("DELETE FROM payment_initial_dispatch_provenance WHERE attempt_id=$1")
            .bind(id)
            .execute(&pool)
            .await
            .is_err()
    );
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

#[sqlx::test(migrator = "MIGRATOR")]
async fn invalid_or_absent_start_never_posts_or_creates_recovery_event(pool: PgPool) {
    use crate::payment_attempts::PaymentStoreError;
    let (time, frozen) = prepared(&pool).await;
    let pay = payment(frozen.snapshot());
    let server = Server::start(vec![
        profile(),
        Reply::Json(201, pay.clone()),
        Reply::Json(200, pay),
    ]);
    let client = local(&server).verify_profile().await.expect("verified");
    let ids = crate::payment_test_fixture::Ids(AtomicU64::new(10000));
    for (id, error) in [
        ("invalid", PaymentStoreError::InvalidRequest),
        (&"f".repeat(32), PaymentStoreError::NotFound),
    ] {
        assert_eq!(
            dispatch_and_observe_test_payment(&pool, time.as_ref(), &ids, &client, id).await,
            Err(InitialDispatchError::Start(error))
        );
        assert_eq!(counts(&pool).await, (0, 0, 0, 0));
        assert_eq!(server.count(), 1, "profile preflight only");
    }
    assert_eq!(
        dispatch_and_observe_test_payment(
            &pool,
            time.as_ref(),
            &ids,
            &client,
            &frozen.snapshot().binding.id
        )
        .await,
        Ok(InitialDispatchOutcome::Observed)
    );
    assert_eq!(counts(&pool).await, (1, 1, 1, 1));
    assert_eq!(server.count(), 3);
}

// Both dispatch identifiers are allocated immediately before the final in-TX
// clock read. Fail the next read, which is permit consumption after commit.
struct PostCommitFault {
    ids: AtomicU64,
    final_reads: AtomicU64,
}
impl commerce_application::request_context::Identifiers for PostCommitFault {
    fn next(&self) -> Result<[u8; 16], commerce_application::request_context::ContextError> {
        let id = self.ids.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(u128::from(10000 + id).to_be_bytes())
    }
}
impl commerce_application::request_context::Clock for PostCommitFault {
    fn unix_seconds(&self) -> Result<u64, commerce_application::request_context::ContextError> {
        if self.ids.load(Ordering::SeqCst) >= 2
            && self.final_reads.fetch_add(1, Ordering::SeqCst) >= 1
        {
            Err(commerce_application::request_context::ContextError::ClockUnavailable)
        } else {
            Ok(1500)
        }
    }
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn clock_failure_after_committed_start_is_uncertain_without_a_post(pool: PgPool) {
    let (time, frozen) = prepared(&pool).await;
    let id = &frozen.snapshot().binding.id;
    let server = Server::start(vec![profile()]);
    let client = local(&server).verify_profile().await.expect("verified");
    let fault = PostCommitFault {
        ids: AtomicU64::new(0),
        final_reads: AtomicU64::new(0),
    };
    assert_eq!(
        dispatch_and_observe_test_payment(&pool, &fault, &fault, &client, id).await,
        Err(InitialDispatchError::Uncertain)
    );
    assert_eq!(
        counts(&pool).await,
        (1, 1, 0, 0),
        "start and recovery committed before fault"
    );
    assert_eq!(server.count(), 1, "no POST was sent");
    let ids = crate::payment_test_fixture::Ids(AtomicU64::new(20000));
    assert_eq!(
        dispatch_and_observe_test_payment(&pool, time.as_ref(), &ids, &client, id).await,
        Ok(InitialDispatchOutcome::Reconcile)
    );
    assert_eq!(server.count(), 1);
}

#[path = "test_dispatch_durability.rs"]
mod durability;

async fn failed_initial_provenance_insert(pool: PgPool, suppress: bool) {
    let (time, frozen) = prepared(&pool).await;
    let id = &frozen.snapshot().binding.id;
    let fault = if suppress {
        "CREATE SEQUENCE fixture_initial_suppression_witness; CREATE FUNCTION fixture_reject_initial_provenance() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM nextval('fixture_initial_suppression_witness'); RETURN NULL; END $$; CREATE TRIGGER zz_fixture_reject_initial_provenance BEFORE INSERT ON payment_initial_dispatch_provenance FOR EACH ROW EXECUTE FUNCTION fixture_reject_initial_provenance()"
    } else {
        "CREATE FUNCTION fixture_reject_initial_provenance() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'synthetic'; END $$; CREATE TRIGGER fixture_reject_initial_provenance BEFORE INSERT ON payment_initial_dispatch_provenance FOR EACH ROW EXECUTE FUNCTION fixture_reject_initial_provenance()"
    };
    pool.execute(fault).await.expect("fault setup");
    let pay = payment(frozen.snapshot());
    let server = Server::start(vec![
        profile(),
        Reply::Json(201, pay.clone()),
        Reply::Json(200, pay),
    ]);
    let client = local(&server).verify_profile().await.expect("verified");
    let ids = crate::payment_test_fixture::Ids(AtomicU64::new(10000));
    assert_eq!(
        dispatch_and_observe_test_payment(&pool, time.as_ref(), &ids, &client, id).await,
        Err(InitialDispatchError::Uncertain)
    );
    assert_eq!(counts(&pool).await, (1, 1, 0, 0));
    if suppress {
        let witness: (i64, bool) =
            sqlx::query_as("SELECT last_value,is_called FROM fixture_initial_suppression_witness")
                .fetch_one(&pool)
                .await
                .expect("suppressed receipt was attempted");
        assert_eq!(witness, (1, true));
    }
    let receipts: i64 =
        sqlx::query_scalar("SELECT count(*) FROM payment_initial_dispatch_provenance")
            .fetch_one(&pool)
            .await
            .expect("no receipt");
    assert_eq!(receipts, 0);
    assert_eq!(
        dispatch_and_observe_test_payment(&pool, time.as_ref(), &ids, &client, id).await,
        Ok(InitialDispatchOutcome::Reconcile)
    );
    assert_eq!(server.count(), 3);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn initial_provenance_insert_failure_rolls_back_observation_and_event(pool: PgPool) {
    failed_initial_provenance_insert(pool, false).await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn suppressed_initial_provenance_rolls_back_and_never_reposts(pool: PgPool) {
    failed_initial_provenance_insert(pool, true).await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn callback_effect_collision_never_upgrades_a_historical_observation(pool: PgPool) {
    use crate::payment_attempts::callbacks;
    let (time, frozen) = prepared(&pool).await;
    let id = frozen.snapshot().binding.id.clone();
    let pay = payment(frozen.snapshot());
    let gate = Arc::new(AtomicBool::new(false));
    let server = Server::start(vec![
        profile(),
        profile(),
        Reply::Json(201, pay.clone()),
        Reply::GateJson(gate.clone(), pay.clone()),
        Reply::Json(200, pay),
    ]);
    let client = Arc::new(
        local(&server)
            .verify_profile()
            .await
            .expect("initial profile"),
    );
    let callback_client = local(&server)
        .verify_profile()
        .await
        .expect("callback profile")
        .into_reconciliation();
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
        while server.count() < 4 {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("initial GET held outside DB");
    let callback = callback_client
        .fetch_callback("tr_SYNTHETIC_NotARealPayment")
        .await
        .expect("authenticated callback GET");
    let effect = format!("initial-fetch-{id}");
    let first = callbacks::record_fetched(&pool, time.as_ref(), ids.as_ref(), &effect, &callback)
        .await
        .expect("callback receipt");
    assert!(!first.replayed);
    gate.store(true, Ordering::SeqCst);
    assert_eq!(
        task.await.expect("initial task"),
        Err(InitialDispatchError::Uncertain)
    );
    // Model an observation that existed before 0073: nullable transaction
    // custody must not change accepted historical reads or generic replay.
    pool.execute("ALTER TABLE payment_observations DISABLE TRIGGER USER")
        .await
        .expect("isolated legacy fixture");
    sqlx::query("UPDATE payment_observations SET source_transaction_id=NULL WHERE attempt_id=$1")
        .bind(&id)
        .execute(&pool)
        .await
        .expect("pre-migration row");
    pool.execute("ALTER TABLE payment_observations ENABLE TRIGGER USER")
        .await
        .expect("restore native guards");
    assert!(
        read_recorded(&pool, &id, &effect)
            .await
            .expect("legacy read")
            .is_some()
    );
    let replay = callbacks::record_fetched(&pool, time.as_ref(), ids.as_ref(), &effect, &callback)
        .await
        .expect("generic exact replay");
    assert!(replay.replayed);
    assert_eq!(counts(&pool).await, (1, 1, 1, 1));
    let receipts: i64 =
        sqlx::query_scalar("SELECT count(*) FROM payment_initial_dispatch_provenance")
            .fetch_one(&pool)
            .await
            .expect("no origin receipt");
    assert_eq!(receipts, 0);
    let mut tx = pool.begin().await.expect("historical read");
    let (current, started) = crate::payment_attempts::evidence::context(&mut tx, &id, false)
        .await
        .expect("historical context");
    assert!(
        crate::payment_attempts::observations::initial::read_source(&mut tx, &current, started)
            .await
            .expect("absent source is not evidence")
            .is_none()
    );
    tx.rollback().await.expect("end historical read");
    // A later native transaction cannot attach the old observation/event to a new receipt.
    let upgrade = sqlx::query("INSERT INTO payment_initial_dispatch_provenance(attempt_id,start_id,observation_id,observation_event_id,observation_sha256) SELECT s.attempt_id,s.id,o.id,e.id,payment_initial_observation_digest(o) FROM payment_dispatch_starts s JOIN payment_observations o ON o.attempt_id=s.attempt_id JOIN commerce_outbox e ON e.kind='payment.observation_recorded.v1' AND e.payload->>'observation_id'=o.id WHERE s.attempt_id=$1")
        .bind(&id).execute(&pool).await;
    assert!(upgrade.is_err());
    assert_eq!(
        dispatch_and_observe_test_payment(&pool, time.as_ref(), ids.as_ref(), client.as_ref(), &id)
            .await,
        Ok(InitialDispatchOutcome::Reconcile)
    );
    server
        .inspect(|requests| assert_eq!(requests.iter().filter(|r| r.method == "POST").count(), 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn initial_observation_digest_has_independent_fixed_oracle_and_session_stability(
    pool: PgPool,
) {
    let mut values = Vec::new();
    for settings in [
        "SET LOCAL TIME ZONE 'UTC'; SET LOCAL DateStyle='ISO, MDY'; SET LOCAL bytea_output='hex'",
        "SET LOCAL TIME ZONE 'Pacific/Apia'; SET LOCAL DateStyle='SQL, DMY'; SET LOCAL bytea_output='escape'",
    ] {
        let mut tx = pool.begin().await.expect("transaction");
        tx.execute(settings).await.expect("session settings");
        let digest: String=sqlx::query_scalar(r#"SELECT encode(payment_initial_observation_digest(ROW('a','b','key',1::bigint,1500::bigint,'tr_x','ffff','paid','{"key":"value"}'::jsonb,'42'::xid8)::payment_observations),'hex')"#)
            .fetch_one(&mut *tx).await.expect("digest");
        values.push(digest);
        tx.rollback().await.expect("rollback");
    }
    // Independently SHA-256 hashed exact UTF-8 bytes of the fixed JSONB array.
    assert_eq!(
        values,
        ["89e735fcd18275345e73a1fe6aabc8504b829daa53e8857b1566c7153b8707d4"; 2]
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn initial_provenance_rejects_coherently_changed_identity_until_exact_repair(pool: PgPool) {
    use crate::payment_attempts::{evidence, observations};
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
        .expect("verified loopback");
    let ids = crate::payment_test_fixture::Ids(AtomicU64::new(10000));
    assert_eq!(
        dispatch_and_observe_test_payment(&pool, time.as_ref(), &ids, &client, id).await,
        Ok(InitialDispatchOutcome::Observed)
    );
    let original: (String, serde_json::Value) =
        sqlx::query_as("SELECT provider_id,snapshot FROM payment_observations WHERE attempt_id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .expect("source");
    let receipt: serde_json::Value = sqlx::query_scalar(
        "SELECT to_jsonb(p) FROM payment_initial_dispatch_provenance p WHERE attempt_id=$1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .expect("receipt");
    let replacement = "tr_SYNTHETIC_NotARealPaymenX";
    assert_ne!(original.0, replacement);
    assert_eq!(original.0.len(), replacement.len(), "UTF-8 byte lengths");
    pool.execute("ALTER TABLE payment_observations DISABLE TRIGGER USER")
        .await
        .expect("fault setup");
    sqlx::query("UPDATE payment_observations SET provider_id=$2,snapshot=jsonb_set(snapshot,'{intent,payment,id}',to_jsonb($2::text)) WHERE attempt_id=$1").bind(id).bind(replacement).execute(&pool).await.expect("coherent identity fault");
    pool.execute("ALTER TABLE payment_observations ENABLE TRIGGER USER")
        .await
        .expect("restore guard");
    let changed = read_recorded(&pool, id, &format!("initial-fetch-{id}"))
        .await
        .expect("ordinary decoder accepts structurally valid fault")
        .expect("observation");
    assert_eq!(changed.intent.payment.id, replacement);
    let exact: bool=sqlx::query_scalar("SELECT payment_initial_dispatch_provenance_exact(p) FROM payment_initial_dispatch_provenance p WHERE attempt_id=$1").bind(id).fetch_one(&pool).await.expect("native exact");
    assert!(!exact);
    let mut tx = pool.begin().await.expect("read");
    let (current, started) = evidence::context(&mut tx, id, false)
        .await
        .expect("context");
    assert!(matches!(
        observations::initial::read_source(&mut tx, &current, started).await,
        Err(crate::payment_attempts::PaymentStoreError::InvalidStoredData)
    ));
    tx.rollback().await.expect("rollback");
    assert_eq!(
        sqlx::query_scalar::<_, serde_json::Value>(
            "SELECT to_jsonb(p) FROM payment_initial_dispatch_provenance p WHERE attempt_id=$1"
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .expect("same receipt"),
        receipt
    );
    pool.execute("ALTER TABLE payment_observations DISABLE TRIGGER USER")
        .await
        .expect("repair setup");
    sqlx::query("UPDATE payment_observations SET provider_id=$2,snapshot=$3 WHERE attempt_id=$1")
        .bind(id)
        .bind(&original.0)
        .bind(&original.1)
        .execute(&pool)
        .await
        .expect("exact repair");
    pool.execute("ALTER TABLE payment_observations ENABLE TRIGGER USER")
        .await
        .expect("restore guard");
    let mut tx = pool.begin().await.expect("read");
    let (current, started) = evidence::context(&mut tx, id, false)
        .await
        .expect("context");
    let restored = observations::initial::read_source(&mut tx, &current, started)
        .await
        .expect("valid source")
        .expect("receipt");
    assert_eq!(restored.intent.payment.id, original.0);
    tx.rollback().await.expect("rollback");
    server.inspect(|requests| {
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.method == "POST")
                .count(),
            1
        )
    });
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn initial_provenance_rolls_back_when_stored_identity_differs_from_fetched_source(
    pool: PgPool,
) {
    let (time, frozen) = prepared(&pool).await;
    let id = &frozen.snapshot().binding.id;
    pool.execute("CREATE FUNCTION fixture_change_initial_identity() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN NEW.provider_id='tr_SYNTHETIC_CHANGED'; NEW.snapshot=jsonb_set(NEW.snapshot,'{intent,payment,id}',to_jsonb(NEW.provider_id)); RETURN NEW; END $$; CREATE TRIGGER zz_fixture_change_initial_identity BEFORE INSERT ON payment_observations FOR EACH ROW EXECUTE FUNCTION fixture_change_initial_identity()")
        .await.expect("synthetic readback fault");
    let pay = payment(frozen.snapshot());
    let server = Server::start(vec![
        profile(),
        Reply::Json(201, pay.clone()),
        Reply::Json(200, pay),
    ]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("verified loopback");
    let ids = crate::payment_test_fixture::Ids(AtomicU64::new(10000));
    assert_eq!(
        dispatch_and_observe_test_payment(&pool, time.as_ref(), &ids, &client, id).await,
        Err(InitialDispatchError::Uncertain)
    );
    assert_eq!(counts(&pool).await, (1, 1, 0, 0));
    let receipts: i64 =
        sqlx::query_scalar("SELECT count(*) FROM payment_initial_dispatch_provenance")
            .fetch_one(&pool)
            .await
            .expect("receipt count");
    assert_eq!(receipts, 0);
    assert_eq!(
        dispatch_and_observe_test_payment(&pool, time.as_ref(), &ids, &client, id).await,
        Ok(InitialDispatchOutcome::Reconcile)
    );
    server.inspect(|requests| {
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.method == "POST")
                .count(),
            1
        )
    });
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn create_capability_rejects_a_different_verified_provider_identity(pool: PgPool) {
    use crate::payment_attempts::{DispatchOutcome, start_dispatch};
    let (time, frozen) = prepared(&pool).await;
    let id = &frozen.snapshot().binding.id;
    let pay = payment(frozen.snapshot());
    let mut other = pay.clone();
    other["id"] = "tr_DIFFERENT_Synthetic".into();
    let server = Server::start(vec![
        profile(),
        Reply::Json(201, pay),
        Reply::Json(200, other),
    ]);
    let client = local(&server).verify_profile().await.expect("verified");
    let ids = crate::payment_test_fixture::Ids(AtomicU64::new(10000));
    let DispatchOutcome::Start(permit) =
        start_dispatch(&pool, time.as_ref(), &ids, id, client.configuration())
            .await
            .expect("start")
    else {
        panic!("fresh start")
    };
    let hint = client
        .create(permit, time.as_ref())
        .await
        .expect("initial POST");
    let other = client
        .fetch(frozen.snapshot(), "tr_DIFFERENT_Synthetic")
        .await
        .expect("authenticated different ID");
    assert!(matches!(
        hint.bind(other),
        Err(MollieError::PaymentMismatch)
    ));
    assert_eq!(counts(&pool).await, (1, 1, 0, 0));
    let receipts: i64 =
        sqlx::query_scalar("SELECT count(*) FROM payment_initial_dispatch_provenance")
            .fetch_one(&pool)
            .await
            .expect("no origin receipt");
    assert_eq!(receipts, 0);
}
