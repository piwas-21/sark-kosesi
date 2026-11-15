//! Synthetic loopback and isolated PostgreSQL only.
use super::super::{
    ReconciliationClient, Reply, Server, TestCredential, VerifiedReconciliationClient, http,
    payment, profile,
};
use super::{counts, observation_cases, synthetic_source};
use crate::{
    MIGRATOR,
    mollie::MollieError,
    payment_attempts::PaymentStoreError,
    payment_attempts::callbacks::lookup::{
        self,
        continuation::{self, ContinuationOutcome},
    },
};
use sqlx::{Executor, PgPool};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

async fn client(server: &Server, rotated: bool) -> (VerifiedReconciliationClient, String) {
    let raw = if rotated {
        format!("test_{}", "rotated".repeat(5))
    } else {
        format!("test_{}", "synthetic".repeat(4))
    };
    let key =
        TestCredential::capture(&raw, "pfl_SYNTHETIC_NotARealProfile").expect("synthetic key");
    let fingerprint = key.binding.credential_fingerprint.clone();
    let client = ReconciliationClient {
        http: http::Http::build(
            server.base.clone(),
            key.authorization,
            Duration::from_secs(2),
        )
        .expect("loopback"),
        profile: key.binding,
    }
    .verify_profile()
    .await
    .expect("profile proof");
    (client, fingerprint)
}
fn get_only(server: &Server) {
    server.inspect(|rs| {
        assert!(rs.iter().all(|r| r.method == "GET"
            && r.body.is_empty()
            && r.header("Idempotency-Key").is_none()))
    });
}

async fn downstream(pool: &PgPool) -> (i64, i64, i64, serde_json::Value) {
    sqlx::query_as("SELECT (SELECT count(*) FROM commerce_outbox),(SELECT count(*) FROM stock_movements),(SELECT count(*) FROM payment_recognitions),(SELECT COALESCE(jsonb_agg(to_jsonb(b) ORDER BY component_code),'[]'::jsonb) FROM stock_balances b)")
        .fetch_one(pool).await.expect("downstream snapshot")
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn continuation_execution_rejects_invalid_ids_and_admitted_supervisor(pool: PgPool) {
    let (clock, frozen) = synthetic_source(&pool).await;
    let id = &frozen.snapshot().binding.id;
    let ids = observation_cases::ids();
    let before = downstream(&pool).await;
    continuation::acquire(&pool, clock.as_ref(), &ids, id)
        .await
        .expect("operation");
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, payment(frozen.snapshot())),
    ]);
    let (client, _) = client(&server, false).await;
    let overlong = "a".repeat(33);
    for invalid in [
        "",
        "bad",
        "g0000000000000000000000000000000",
        overlong.as_str(),
    ] {
        assert!(matches!(
            continuation::inspect(&pool, invalid).await,
            Err(PaymentStoreError::InvalidRequest)
        ));
        assert!(matches!(
            continuation::execute(
                &pool,
                &observation_cases::BadPorts,
                &observation_cases::BadPorts,
                invalid,
                &client
            )
            .await,
            Err(MollieError::Custody(PaymentStoreError::InvalidRequest))
        ));
    }
    assert_eq!(counts(&pool).await, (1, 1, 0, 0, 1));
    assert_eq!(server.count(), 1, "profile verification only");
    assert_eq!(
        continuation::inspect(&pool, id)
            .await
            .expect("valid inspection"),
        None
    );
    crate::payment_attempts::reconciliation::jobs::enqueue(
        &pool,
        clock.as_ref(),
        &frozen.snapshot().binding.profile.id,
    )
    .await
    .expect("real supervisor admission");
    assert!(
        crate::payment_attempts::reconciliation::read_job(&pool, id)
            .await
            .expect("supervisor read")
            .is_some()
    );
    assert!(matches!(
        continuation::inspect(&pool, id).await,
        Err(PaymentStoreError::InvalidStoredData)
    ));
    assert!(matches!(
        continuation::execute(&pool, clock.as_ref(), &ids, id, &client).await,
        Err(MollieError::Custody(PaymentStoreError::InvalidStoredData))
    ));
    assert_eq!(counts(&pool).await, (1, 1, 0, 0, 1));
    assert_eq!(server.count(), 1);
    get_only(&server);
    assert_eq!(downstream(&pool).await, before);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn continuation_execution_rejects_early_clock_and_suppressed_start_before_get(pool: PgPool) {
    let (clock, frozen) = synthetic_source(&pool).await;
    let id = &frozen.snapshot().binding.id;
    let ids = observation_cases::ids();
    let before = downstream(&pool).await;
    continuation::acquire(&pool, clock.as_ref(), &ids, id)
        .await
        .expect("operation at 1500");
    pool.execute("CREATE SEQUENCE public.test_suppressed_start_witness; CREATE FUNCTION public.test_suppress_start() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_catalog.nextval('public.test_suppressed_start_witness'); RETURN NULL; END $$; CREATE TRIGGER zz_test_suppress_start BEFORE INSERT ON public.callback_lookup_fetch_starts FOR EACH ROW EXECUTE FUNCTION public.test_suppress_start()").await.expect("synthetic suppressor");
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, payment(frozen.snapshot())),
    ]);
    let (client, _) = client(&server, false).await;
    clock.0.store(1499, Ordering::SeqCst);
    assert!(matches!(
        continuation::execute(&pool, clock.as_ref(), &ids, id, &client).await,
        Err(MollieError::Custody(PaymentStoreError::ClockUnavailable))
    ));
    let witness: (i64, bool) =
        sqlx::query_as("SELECT last_value,is_called FROM public.test_suppressed_start_witness")
            .fetch_one(&pool)
            .await
            .expect("early clock witness");
    assert_eq!(witness, (1, false));
    assert_eq!(counts(&pool).await, (1, 1, 0, 0, 1));
    assert_eq!(server.count(), 1);
    clock.0.store(1500, Ordering::SeqCst);
    assert!(matches!(
        continuation::execute(&pool, clock.as_ref(), &ids, id, &client).await,
        Err(MollieError::Custody(PaymentStoreError::Unavailable))
    ));
    let witness: (i64, bool) =
        sqlx::query_as("SELECT last_value,is_called FROM public.test_suppressed_start_witness")
            .fetch_one(&pool)
            .await
            .expect("suppression witness");
    assert_eq!(witness, (1, true));
    assert_eq!(counts(&pool).await, (1, 1, 0, 0, 1));
    assert_eq!(server.count(), 1);
    pool.execute("DROP TRIGGER zz_test_suppress_start ON public.callback_lookup_fetch_starts; DROP FUNCTION public.test_suppress_start()").await.expect("remove suppressor");
    assert_eq!(
        continuation::execute(&pool, clock.as_ref(), &ids, id, &client)
            .await
            .expect("repaired start"),
        ContinuationOutcome::Recorded { replayed: false }
    );
    assert_eq!(counts(&pool).await, (1, 1, 1, 1, 1));
    assert_eq!(server.count(), 2);
    get_only(&server);
    assert_eq!(downstream(&pool).await, before);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn continuation_execution_start_commit_failure_stays_unknown_and_sends_no_get(pool: PgPool) {
    let (clock, frozen) = synthetic_source(&pool).await;
    let id = &frozen.snapshot().binding.id;
    let ids = observation_cases::ids();
    let before = downstream(&pool).await;
    continuation::acquire(&pool, clock.as_ref(), &ids, id)
        .await
        .expect("operation");
    pool.execute("CREATE SEQUENCE public.test_start_commit_witness; CREATE FUNCTION public.test_fail_start_commit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_catalog.nextval('public.test_start_commit_witness'); RAISE EXCEPTION 'synthetic deferred start failure'; END $$; CREATE CONSTRAINT TRIGGER test_fail_start_commit AFTER INSERT ON public.callback_lookup_fetch_starts DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION public.test_fail_start_commit()").await.expect("deferred start fault");
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, payment(frozen.snapshot())),
    ]);
    let (client, _) = client(&server, false).await;
    assert_eq!(
        continuation::execute(&pool, clock.as_ref(), &ids, id, &client)
            .await
            .expect("commit outcome"),
        ContinuationOutcome::Unknown
    );
    let witness: (i64, bool) =
        sqlx::query_as("SELECT last_value,is_called FROM public.test_start_commit_witness")
            .fetch_one(&pool)
            .await
            .expect("deferred start witness");
    assert_eq!(witness, (1, true));
    assert_eq!(counts(&pool).await, (1, 1, 0, 0, 1));
    assert_eq!(
        continuation::inspect(&pool, id)
            .await
            .expect("offline status"),
        None
    );
    assert_eq!(
        server.count(),
        1,
        "profile verification only; no payment GET"
    );
    pool.execute("DROP TRIGGER test_fail_start_commit ON public.callback_lookup_fetch_starts; DROP FUNCTION public.test_fail_start_commit()").await.expect("remove deferred fault");
    assert_eq!(
        continuation::inspect(&pool, id)
            .await
            .expect("unchanged offline status"),
        None
    );
    assert_eq!(server.count(), 1);
    get_only(&server);
    assert_eq!(downstream(&pool).await, before);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn continuation_execution_receipt_commit_failure_preserves_unknown_without_refetch(
    pool: PgPool,
) {
    let (clock, frozen) = synthetic_source(&pool).await;
    let id = &frozen.snapshot().binding.id;
    let ids = observation_cases::ids();
    let before = downstream(&pool).await;
    continuation::acquire(&pool, clock.as_ref(), &ids, id)
        .await
        .expect("operation");
    pool.execute("CREATE SEQUENCE public.test_receipt_commit_witness; CREATE FUNCTION public.test_fail_receipt_commit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_catalog.nextval('public.test_receipt_commit_witness'); RAISE EXCEPTION 'synthetic deferred receipt failure'; END $$; CREATE CONSTRAINT TRIGGER test_fail_receipt_commit AFTER INSERT ON public.callback_lookup_receipts DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION public.test_fail_receipt_commit()").await.expect("deferred receipt fault");
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, payment(frozen.snapshot())),
    ]);
    let (client, _) = client(&server, false).await;
    assert_eq!(
        continuation::execute(&pool, clock.as_ref(), &ids, id, &client)
            .await
            .expect("receipt outcome"),
        ContinuationOutcome::Unknown
    );
    let witness: (i64, bool) =
        sqlx::query_as("SELECT last_value,is_called FROM public.test_receipt_commit_witness")
            .fetch_one(&pool)
            .await
            .expect("deferred receipt witness");
    assert_eq!(witness, (1, true));
    assert_eq!(counts(&pool).await, (1, 1, 1, 0, 1));
    let events: i64 = sqlx::query_scalar("SELECT count(*) FROM public.callback_processing_events")
        .fetch_one(&pool)
        .await
        .expect("rolled back receipt event");
    assert_eq!(events, 0);
    assert_eq!(
        continuation::inspect(&pool, id)
            .await
            .expect("offline status"),
        Some(ContinuationOutcome::Unknown)
    );
    assert_eq!(server.count(), 2, "one verified payment GET");
    pool.execute("DROP TRIGGER test_fail_receipt_commit ON public.callback_lookup_receipts; DROP FUNCTION public.test_fail_receipt_commit()").await.expect("remove deferred fault");
    assert_eq!(
        continuation::execute(&pool, clock.as_ref(), &ids, id, &client)
            .await
            .expect("unknown replay"),
        ContinuationOutcome::Unknown
    );
    assert_eq!(
        continuation::inspect(&pool, id)
            .await
            .expect("offline replay"),
        Some(ContinuationOutcome::Unknown)
    );
    assert_eq!(counts(&pool).await, (1, 1, 1, 0, 1));
    assert_eq!(server.count(), 2, "no second payment GET");
    get_only(&server);
    assert_eq!(downstream(&pool).await, before);
}

// The gate witnesses that the one checked GET has left the database transaction.
async fn wait_for_payment_get(server: &Server) {
    tokio::time::timeout(Duration::from_secs(1), async {
        while server.count() < 2 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("payment GET reached loopback gate");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn continuation_changed_operation_during_get_is_unknown_and_exact_repair_never_refetches(
    pool: PgPool,
) {
    let (clock, frozen) = synthetic_source(&pool).await;
    let id = frozen.snapshot().binding.id.clone();
    let ids = Arc::new(observation_cases::ids());
    let binding = continuation::acquire(&pool, clock.as_ref(), ids.as_ref(), &id)
        .await
        .expect("operation at source time");
    let before = downstream(&pool).await;
    // Leave room for a different valid operation timestamp between the source and start.
    clock.0.store(1501, Ordering::SeqCst);
    let gate = Arc::new(AtomicBool::new(false));
    let server = Server::start(vec![
        profile(),
        Reply::GateJson(gate.clone(), payment(frozen.snapshot())),
    ]);
    let (client, _) = client(&server, false).await;
    let client = Arc::new(client);
    let (p, time, i, attempt, c) = (
        pool.clone(),
        clock.clone(),
        ids.clone(),
        id.clone(),
        client.clone(),
    );
    let task = tokio::spawn(async move {
        continuation::execute(&p, time.as_ref(), i.as_ref(), &attempt, c.as_ref()).await
    });
    wait_for_payment_get(&server).await;
    let original: (i64, i64) = sqlx::query_as("SELECT o.created_at,s.started_at FROM public.callback_lookup_operations o JOIN public.callback_lookup_fetch_starts s ON s.operation_id=o.id WHERE o.id=$1")
        .bind(&binding.operation_id).fetch_one(&pool).await.expect("committed start");
    assert_eq!(original, (1500, 1501));
    pool.execute("ALTER TABLE public.callback_lookup_operations DISABLE TRIGGER guard_callback_lookup_operation").await.expect("isolated import fault");
    assert_eq!(sqlx::query("UPDATE public.callback_lookup_operations SET created_at=1501 WHERE id=$1 AND created_at=1500").bind(&binding.operation_id).execute(&pool).await.expect("change operation timestamp").rows_affected(), 1);
    pool.execute("ALTER TABLE public.callback_lookup_operations ENABLE TRIGGER guard_callback_lookup_operation").await.expect("restore guard");
    let exact: bool = sqlx::query_scalar("SELECT public.payment_test_continuation_exact(c) FROM public.payment_test_continuations c WHERE c.attempt_id=$1")
        .bind(&id).fetch_one(&pool).await.expect("modified binding remains structurally exact");
    assert!(exact);
    gate.store(true, Ordering::SeqCst);
    assert_eq!(
        task.await.expect("GET task").expect("uncertain record"),
        ContinuationOutcome::Unknown
    );
    assert_eq!(counts(&pool).await, (1, 1, 1, 0, 1));
    assert_eq!(
        continuation::inspect(&pool, &id)
            .await
            .expect("changed operation status"),
        Some(ContinuationOutcome::Unknown)
    );
    pool.execute("ALTER TABLE public.callback_lookup_operations DISABLE TRIGGER guard_callback_lookup_operation").await.expect("repair setup");
    assert_eq!(sqlx::query("UPDATE public.callback_lookup_operations SET created_at=$2 WHERE id=$1 AND created_at=1501").bind(&binding.operation_id).bind(original.0).execute(&pool).await.expect("exact timestamp repair").rows_affected(), 1);
    pool.execute("ALTER TABLE public.callback_lookup_operations ENABLE TRIGGER guard_callback_lookup_operation").await.expect("restore guard");
    assert_eq!(
        continuation::inspect(&pool, &id)
            .await
            .expect("repaired status"),
        Some(ContinuationOutcome::Unknown)
    );
    assert_eq!(
        continuation::execute(&pool, clock.as_ref(), ids.as_ref(), &id, client.as_ref())
            .await
            .expect("no retry"),
        ContinuationOutcome::Unknown
    );
    assert_eq!(server.count(), 2);
    assert_eq!(downstream(&pool).await, before);
    get_only(&server);
}

// This is an explicitly synthetic native receipt, not proof of provider I/O.
// Its 0077 original-binding guard and the reciprocal event constraint stay enabled.
async fn install_synthetic_receipt_while_get_waits(
    pool: &PgPool,
    snapshot: &commerce_domain::payment_attempts::callbacks::LookupSnapshot,
) {
    let value = serde_json::to_value(snapshot).expect("synthetic snapshot");
    let mut tx = pool.begin().await.expect("fixture transaction");
    assert_eq!(sqlx::query("INSERT INTO public.callback_lookup_receipts(id,operation_id,recorded_at,snapshot) VALUES($1,$2,$3,$4)")
        .bind(&snapshot.id).bind(&snapshot.operation.id).bind(snapshot.recorded_at).bind(value)
        .execute(&mut *tx).await.expect("native guarded receipt").rows_affected(), 1);
    assert_eq!(sqlx::query("INSERT INTO public.callback_processing_events(id,operation_id,receipt_id,created_at,kind,payload) SELECT $1,r.operation_id,r.id,r.recorded_at,'payment.callback_lookup_recorded.v1',pg_catalog.jsonb_build_object('schema_version',1,'operation_id',r.operation_id,'receipt_id',r.id,'snapshot_sha256',pg_catalog.encode(pg_catalog.sha256(pg_catalog.convert_to(r.snapshot::pg_catalog.text,'UTF8')),'hex')) FROM public.callback_lookup_receipts r WHERE r.id=$2")
        .bind("d".repeat(32)).bind(&snapshot.id).execute(&mut *tx).await.expect("native reciprocal event").rows_affected(), 1);
    tx.commit().await.expect("native complete receipt");
}

async fn preexisting_receipt_race(pool: PgPool, contradictory: bool) {
    use commerce_domain::payment_attempts::callbacks::LookupSnapshot;
    let (clock, frozen) = synthetic_source(&pool).await;
    let id = frozen.snapshot().binding.id.clone();
    let ids = Arc::new(observation_cases::ids());
    let binding = continuation::acquire(&pool, clock.as_ref(), ids.as_ref(), &id)
        .await
        .expect("operation");
    let operation = lookup::read_operation(&pool, &binding.operation_id)
        .await
        .expect("operation snapshot")
        .snapshot()
        .clone();
    let before = downstream(&pool).await;
    let gate = Arc::new(AtomicBool::new(false));
    let server = Server::start(vec![
        profile(),
        Reply::GateJson(gate.clone(), payment(frozen.snapshot())),
    ]);
    let (client, actual_fingerprint) = client(&server, false).await;
    let client = Arc::new(client);
    let (p, time, i, attempt, c) = (
        pool.clone(),
        clock.clone(),
        ids.clone(),
        id.clone(),
        client.clone(),
    );
    let task = tokio::spawn(async move {
        continuation::execute(&p, time.as_ref(), i.as_ref(), &attempt, c.as_ref()).await
    });
    wait_for_payment_get(&server).await;
    let (provider, _) = super::super::wire::payment(
        &serde_json::to_vec(&payment(frozen.snapshot())).expect("synthetic response"),
        frozen.snapshot(),
        None,
    )
    .expect("synthetic matching projection");
    let fingerprint = if contradictory {
        let different = "b".repeat(64);
        assert_ne!(different, actual_fingerprint);
        different
    } else {
        actual_fingerprint
    };
    let snapshot = LookupSnapshot {
        schema_version: 1,
        id: "e".repeat(32),
        operation,
        started_at: 1500,
        recorded_at: 1500,
        payment: provider,
        credential_fingerprint: fingerprint,
    };
    snapshot.validate().expect("valid synthetic projection");
    install_synthetic_receipt_while_get_waits(&pool, &snapshot).await;
    assert_eq!(
        continuation::inspect(&pool, &id)
            .await
            .expect("concurrent receipt"),
        Some(ContinuationOutcome::Recorded { replayed: true })
    );
    gate.store(true, Ordering::SeqCst);
    let expected = if contradictory {
        ContinuationOutcome::Unknown
    } else {
        ContinuationOutcome::Recorded { replayed: false }
    };
    assert_eq!(
        task.await.expect("GET task").expect("record outcome"),
        expected
    );
    assert_eq!(
        continuation::inspect(&pool, &id)
            .await
            .expect("preserved receipt"),
        Some(ContinuationOutcome::Recorded { replayed: true })
    );
    assert_eq!(
        continuation::execute(&pool, clock.as_ref(), ids.as_ref(), &id, client.as_ref())
            .await
            .expect("offline replay"),
        ContinuationOutcome::Recorded { replayed: true }
    );
    let retained: serde_json::Value = sqlx::query_scalar(
        "SELECT snapshot FROM public.callback_lookup_receipts WHERE operation_id=$1",
    )
    .bind(&binding.operation_id)
    .fetch_one(&pool)
    .await
    .expect("retained exact receipt");
    assert_eq!(
        retained,
        serde_json::to_value(&snapshot).expect("synthetic expectation")
    );
    assert_eq!(counts(&pool).await, (1, 1, 1, 1, 1));
    assert_eq!(server.count(), 2);
    assert_eq!(downstream(&pool).await, before);
    get_only(&server);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn continuation_matching_receipt_committed_during_get_is_reused(pool: PgPool) {
    preexisting_receipt_race(pool, false).await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn continuation_contradictory_receipt_committed_during_get_is_not_overwritten(pool: PgPool) {
    preexisting_receipt_race(pool, true).await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn continuation_coherently_mutated_insert_rolls_back_and_never_refetches(pool: PgPool) {
    let (clock, frozen) = synthetic_source(&pool).await;
    let id = &frozen.snapshot().binding.id;
    let ids = observation_cases::ids();
    continuation::acquire(&pool, clock.as_ref(), &ids, id)
        .await
        .expect("source");
    let before = downstream(&pool).await;
    // The original guard, ordinary receipt guard, and event guard stay enabled.
    // The fixture trigger runs before both native receipt guards and changes
    // only a valid payment status; their ordinary checks still run.
    pool.execute("CREATE SEQUENCE public.test_mutated_receipt_witness; CREATE SEQUENCE public.test_mutated_event_witness; CREATE FUNCTION public.test_mutate_receipt() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_catalog.nextval('public.test_mutated_receipt_witness'); NEW.snapshot=pg_catalog.jsonb_set(NEW.snapshot,'{payment,status}','\"open\"'::jsonb); RETURN NEW; END $$; CREATE TRIGGER a0_test_mutate_receipt BEFORE INSERT ON public.callback_lookup_receipts FOR EACH ROW EXECUTE FUNCTION public.test_mutate_receipt(); CREATE FUNCTION public.test_observe_receipt_event() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_catalog.nextval('public.test_mutated_event_witness'); RETURN NEW; END $$; CREATE TRIGGER zz_test_observe_receipt_event AFTER INSERT ON public.callback_processing_events FOR EACH ROW EXECUTE FUNCTION public.test_observe_receipt_event()").await.expect("synthetic valid mutation and event witnesses");
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, payment(frozen.snapshot())),
    ]);
    let (client, _) = client(&server, false).await;
    assert_eq!(
        continuation::execute(&pool, clock.as_ref(), &ids, id, &client)
            .await
            .expect("changed readback"),
        ContinuationOutcome::Unknown
    );
    for sql in [
        "SELECT last_value,is_called FROM public.test_mutated_receipt_witness",
        "SELECT last_value,is_called FROM public.test_mutated_event_witness",
    ] {
        let (count, called): (i64, bool) = sqlx::query_as(sql)
            .fetch_one(&pool)
            .await
            .expect("nontransactional witness");
        assert_eq!((count, called), (1, true));
    }
    assert_eq!(counts(&pool).await, (1, 1, 1, 0, 1));
    let events: i64 = sqlx::query_scalar("SELECT count(*) FROM public.callback_processing_events")
        .fetch_one(&pool)
        .await
        .expect("rolled-back event");
    assert_eq!(events, 0);
    pool.execute("DROP TRIGGER a0_test_mutate_receipt ON public.callback_lookup_receipts; DROP FUNCTION public.test_mutate_receipt(); DROP TRIGGER zz_test_observe_receipt_event ON public.callback_processing_events; DROP FUNCTION public.test_observe_receipt_event()").await.expect("remove fixture triggers");
    assert_eq!(
        continuation::inspect(&pool, id)
            .await
            .expect("offline status"),
        Some(ContinuationOutcome::Unknown)
    );
    assert_eq!(
        continuation::execute(&pool, clock.as_ref(), &ids, id, &client)
            .await
            .expect("no retry"),
        ContinuationOutcome::Unknown
    );
    assert_eq!(server.count(), 2);
    assert_eq!(downstream(&pool).await, before);
    get_only(&server);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn continuation_receipt_rotated_key_race_and_replay_are_one_get(pool: PgPool) {
    let (clock, frozen) = synthetic_source(&pool).await;
    let id = frozen.snapshot().binding.id.clone();
    let ids = Arc::new(observation_cases::ids());
    let binding = continuation::acquire(&pool, clock.as_ref(), ids.as_ref(), &id)
        .await
        .expect("source");
    let gate = Arc::new(AtomicBool::new(false));
    let server = Server::start(vec![
        profile(),
        Reply::GateJson(gate.clone(), payment(frozen.snapshot())),
    ]);
    let (client, actual_fingerprint) = client(&server, true).await;
    assert_ne!(
        actual_fingerprint,
        frozen.snapshot().binding.profile.credential_fingerprint
    );
    let client = Arc::new(client);
    let before: (i64,i64,i64) = sqlx::query_as("SELECT (SELECT count(*) FROM commerce_outbox),(SELECT count(*) FROM stock_movements),(SELECT count(*) FROM payment_recognitions)").fetch_one(&pool).await.expect("effects");
    let p = pool.clone();
    let time = clock.clone();
    let i = ids.clone();
    let attempt = id.clone();
    let c = client.clone();
    let first = tokio::spawn(async move {
        continuation::execute(&p, time.as_ref(), i.as_ref(), &attempt, c.as_ref()).await
    });
    tokio::time::timeout(Duration::from_secs(1), async {
        while server.count() < 2 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("GET reached loopback");
    assert_eq!(
        continuation::execute(&pool, clock.as_ref(), ids.as_ref(), &id, client.as_ref())
            .await
            .expect("concurrent"),
        ContinuationOutcome::Unknown
    );
    gate.store(true, Ordering::SeqCst);
    assert_eq!(
        first.await.expect("task").expect("execute"),
        ContinuationOutcome::Recorded { replayed: false }
    );
    assert_eq!(
        continuation::execute(
            &pool,
            &observation_cases::BadPorts,
            &observation_cases::BadPorts,
            &id,
            client.as_ref()
        )
        .await
        .expect("historical replay"),
        ContinuationOutcome::Recorded { replayed: true }
    );
    assert_eq!(
        continuation::inspect(&pool, &id)
            .await
            .expect("offline inspection"),
        Some(ContinuationOutcome::Recorded { replayed: true })
    );
    // Corrupt both projection and reciprocal digest, so original matching is
    // independently required when inspecting retained history.
    let original: (serde_json::Value, serde_json::Value) = sqlx::query_as("SELECT r.snapshot,e.payload FROM callback_lookup_receipts r JOIN callback_processing_events e ON e.receipt_id=r.id WHERE r.operation_id=$1").bind(&binding.operation_id).fetch_one(&pool).await.expect("original receipt");
    pool.execute("ALTER TABLE callback_lookup_receipts DISABLE TRIGGER USER; ALTER TABLE callback_processing_events DISABLE TRIGGER USER").await.expect("fault setup");
    sqlx::query("UPDATE callback_lookup_receipts SET snapshot=jsonb_set(snapshot,'{payment,amount_minor}','1'::jsonb) WHERE operation_id=$1").bind(&binding.operation_id).execute(&pool).await.expect("damage amount");
    sqlx::query("UPDATE callback_processing_events e SET payload=jsonb_set(e.payload,'{snapshot_sha256}',to_jsonb(encode(sha256(convert_to(r.snapshot::text,'UTF8')),'hex'))) FROM callback_lookup_receipts r WHERE e.receipt_id=r.id AND r.operation_id=$1").bind(&binding.operation_id).execute(&pool).await.expect("coherent digest");
    pool.execute("ALTER TABLE callback_lookup_receipts ENABLE TRIGGER USER; ALTER TABLE callback_processing_events ENABLE TRIGGER USER").await.expect("restore guards");
    assert!(continuation::inspect(&pool, &id).await.is_err());
    pool.execute("ALTER TABLE callback_lookup_receipts DISABLE TRIGGER USER; ALTER TABLE callback_processing_events DISABLE TRIGGER USER").await.expect("repair setup");
    sqlx::query("UPDATE callback_lookup_receipts SET snapshot=$2 WHERE operation_id=$1")
        .bind(&binding.operation_id)
        .bind(&original.0)
        .execute(&pool)
        .await
        .expect("restore receipt");
    sqlx::query("UPDATE callback_processing_events SET payload=$2 WHERE operation_id=$1")
        .bind(&binding.operation_id)
        .bind(&original.1)
        .execute(&pool)
        .await
        .expect("restore event");
    pool.execute("ALTER TABLE callback_lookup_receipts ENABLE TRIGGER USER; ALTER TABLE callback_processing_events ENABLE TRIGGER USER").await.expect("restore guards");
    assert_eq!(
        continuation::inspect(&pool, &id)
            .await
            .expect("exact restored inspection"),
        Some(ContinuationOutcome::Recorded { replayed: true })
    );
    assert_eq!(server.count(), 2);
    get_only(&server);
    server.inspect(|r| assert_eq!(r[1].path, "/v2/payments/tr_SYNTHETIC_NotARealPayment"));
    let fingerprint: String = sqlx::query_scalar("SELECT snapshot->>'credential_fingerprint' FROM callback_lookup_receipts WHERE operation_id=$1").bind(&binding.operation_id).fetch_one(&pool).await.expect("fingerprint");
    assert_eq!(fingerprint, actual_fingerprint);
    assert_eq!(counts(&pool).await, (1, 1, 1, 1, 1));
    let after: (i64,i64,i64) = sqlx::query_as("SELECT (SELECT count(*) FROM commerce_outbox),(SELECT count(*) FROM stock_movements),(SELECT count(*) FROM payment_recognitions)").fetch_one(&pool).await.expect("effects");
    assert_eq!(after, before);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn continuation_wrong_payment_leaves_unknown_and_never_refetches(pool: PgPool) {
    let (clock, frozen) = synthetic_source(&pool).await;
    let id = &frozen.snapshot().binding.id;
    let ids = observation_cases::ids();
    continuation::acquire(&pool, clock.as_ref(), &ids, id)
        .await
        .expect("source");
    let mut wrong = payment(frozen.snapshot());
    wrong["amount"]["value"] = "382.46".into();
    let server = Server::start(vec![profile(), Reply::Json(200, wrong)]);
    let (client, _) = client(&server, false).await;
    assert_eq!(
        continuation::execute(&pool, clock.as_ref(), &ids, id, &client)
            .await
            .expect("unknown"),
        ContinuationOutcome::Unknown
    );
    assert_eq!(
        continuation::execute(&pool, clock.as_ref(), &ids, id, &client)
            .await
            .expect("unknown replay"),
        ContinuationOutcome::Unknown
    );
    assert_eq!(server.count(), 2);
    get_only(&server);
    assert_eq!(counts(&pool).await, (1, 1, 1, 0, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn continuation_successful_get_with_storage_failure_never_refetches(pool: PgPool) {
    let (clock, frozen) = synthetic_source(&pool).await;
    let id = &frozen.snapshot().binding.id;
    let ids = observation_cases::ids();
    continuation::acquire(&pool, clock.as_ref(), &ids, id)
        .await
        .expect("source");
    pool.execute("CREATE FUNCTION fail_test_receipt() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'synthetic failure'; END $$; CREATE TRIGGER zz_fail_test_receipt BEFORE INSERT ON callback_lookup_receipts FOR EACH ROW EXECUTE FUNCTION fail_test_receipt()").await.expect("fault");
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, payment(frozen.snapshot())),
    ]);
    let (client, _) = client(&server, false).await;
    assert_eq!(
        continuation::execute(&pool, clock.as_ref(), &ids, id, &client)
            .await
            .expect("unknown"),
        ContinuationOutcome::Unknown
    );
    pool.execute("DROP TRIGGER zz_fail_test_receipt ON callback_lookup_receipts; DROP FUNCTION fail_test_receipt()").await.expect("remove fault");
    assert_eq!(
        continuation::execute(&pool, clock.as_ref(), &ids, id, &client)
            .await
            .expect("unknown replay"),
        ContinuationOutcome::Unknown
    );
    assert_eq!(server.count(), 2);
    get_only(&server);
    assert_eq!(counts(&pool).await, (1, 1, 1, 0, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn continuation_wrong_verified_profile_cannot_fetch(pool: PgPool) {
    let (clock, frozen) = synthetic_source(&pool).await;
    let id = &frozen.snapshot().binding.id;
    let ids = observation_cases::ids();
    continuation::acquire(&pool, clock.as_ref(), &ids, id)
        .await
        .expect("source");
    let server = Server::start(vec![Reply::Json(
        200,
        serde_json::json!({"resource":"profile","id":"pfl_OTHER_SYNTHETIC"}),
    )]);
    let key = TestCredential::capture(
        &format!("test_{}", "synthetic".repeat(4)),
        "pfl_OTHER_SYNTHETIC",
    )
    .expect("test key");
    let client = ReconciliationClient {
        http: http::Http::build(
            server.base.clone(),
            key.authorization,
            Duration::from_secs(2),
        )
        .expect("loopback"),
        profile: key.binding,
    }
    .verify_profile()
    .await
    .expect("independently verified other profile");
    assert!(matches!(
        continuation::execute(&pool, clock.as_ref(), &ids, id, &client).await,
        Err(super::super::MollieError::ProfileMismatch)
    ));
    assert_eq!(server.count(), 1);
    get_only(&server);
    assert_eq!(counts(&pool).await, (1, 1, 0, 0, 1));

    let permit = continuation::start(&pool, clock.as_ref(), id)
        .await
        .expect("separate committed start");
    assert!(matches!(
        client.fetch_continuation(permit).await,
        Err(super::super::MollieError::ProfileMismatch)
    ));
    assert_eq!(server.count(), 1);
    assert_eq!(
        continuation::inspect(&pool, id)
            .await
            .expect("retained start"),
        Some(ContinuationOutcome::Unknown)
    );
    assert_eq!(counts(&pool).await, (1, 1, 1, 0, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn continuation_receipt_native_guard_checks_original_and_accepts_exact_projection(
    pool: PgPool,
) {
    use commerce_domain::payment_attempts::callbacks::LookupSnapshot;
    let (clock, frozen) = synthetic_source(&pool).await;
    let id = &frozen.snapshot().binding.id;
    let ids = observation_cases::ids();
    let binding = continuation::acquire(&pool, clock.as_ref(), &ids, id)
        .await
        .expect("source");
    let operation = lookup::read_operation(&pool, &binding.operation_id)
        .await
        .expect("operation");
    let _start = continuation::start(&pool, clock.as_ref(), id)
        .await
        .expect("start");
    let (provider, _) = super::super::wire::payment(
        &serde_json::to_vec(&payment(frozen.snapshot())).expect("synthetic"),
        frozen.snapshot(),
        None,
    )
    .expect("synthetic projection");
    let snapshot = LookupSnapshot {
        schema_version: 1,
        id: "e".repeat(32),
        operation: operation.snapshot().clone(),
        started_at: 1500,
        recorded_at: 1500,
        payment: provider,
        credential_fingerprint: "a".repeat(64),
    };
    let exact = serde_json::to_value(&snapshot).expect("projection");
    pool.execute("CREATE SCHEMA fixture_receipt_shadow; CREATE FUNCTION fixture_receipt_shadow.always_equal(text,text) RETURNS boolean LANGUAGE sql IMMUTABLE AS $$ SELECT true $$; CREATE OPERATOR fixture_receipt_shadow.= (LEFTARG=pg_catalog.text,RIGHTARG=pg_catalog.text,FUNCTION=fixture_receipt_shadow.always_equal); CREATE FUNCTION fixture_receipt_shadow.always_equal_jsonb(jsonb,jsonb) RETURNS boolean LANGUAGE sql IMMUTABLE AS $$ SELECT true $$; CREATE OPERATOR fixture_receipt_shadow.= (LEFTARG=pg_catalog.jsonb,RIGHTARG=pg_catalog.jsonb,FUNCTION=fixture_receipt_shadow.always_equal_jsonb)")
        .await.expect("isolated structural spoof");
    let mut probe = pool.begin().await.expect("operator probe");
    sqlx::query("SET LOCAL search_path=fixture_receipt_shadow,pg_catalog,public")
        .execute(&mut *probe)
        .await
        .expect("hostile probe");
    let (spoofed, real, spoofed_json, real_json): (bool, bool, bool, bool) =
        sqlx::query_as("SELECT 'a'::text='b'::text,'a'::text OPERATOR(pg_catalog.=) 'b'::text,'1'::jsonb='2'::jsonb,'1'::jsonb OPERATOR(pg_catalog.=) '2'::jsonb")
            .fetch_one(&mut *probe)
            .await
            .expect("structural operator probe");
    assert_eq!(
        (spoofed, real, spoofed_json, real_json),
        (true, false, true, false)
    );
    probe.rollback().await.expect("close probe");
    for (field, value) in [
        ("order_id", serde_json::json!("f".repeat(32))),
        ("attempt_id", serde_json::json!("f".repeat(32))),
        ("amount_minor", serde_json::json!(1)),
    ] {
        let mut damaged = exact.clone();
        damaged["payment"][field] = value;
        let mut tx = pool.begin().await.expect("negative transaction");
        sqlx::query("SET LOCAL search_path=fixture_receipt_shadow,pg_catalog,public")
            .execute(&mut *tx)
            .await
            .expect("hostile caller");
        let failure = sqlx::query("INSERT INTO public.callback_lookup_receipts(id,operation_id,recorded_at,snapshot) VALUES($1,$2,1500,$3)").bind(&snapshot.id).bind(&binding.operation_id).bind(damaged).execute(&mut *tx).await.expect_err("native original check");
        let error = failure.as_database_error().expect("database error");
        assert_eq!(error.code().as_deref(), Some("23514"));
        assert_eq!(error.message(), "Continuation receipt custody invalid");
        tx.rollback().await.expect("negative rollback");
    }
    let mut tx = pool.begin().await.expect("positive transaction");
    sqlx::query("INSERT INTO callback_lookup_receipts(id,operation_id,recorded_at,snapshot) VALUES($1,$2,1500,$3)").bind(&snapshot.id).bind(&binding.operation_id).bind(exact).execute(&mut *tx).await.expect("synthetic structural positive");
    sqlx::query("INSERT INTO callback_processing_events(id,operation_id,receipt_id,created_at,kind,payload) SELECT $1,r.operation_id,r.id,r.recorded_at,'payment.callback_lookup_recorded.v1',jsonb_build_object('schema_version',1,'operation_id',r.operation_id,'receipt_id',r.id,'snapshot_sha256',encode(sha256(convert_to(r.snapshot::text,'UTF8')),'hex')) FROM callback_lookup_receipts r WHERE r.id=$2").bind("d".repeat(32)).bind(&snapshot.id).execute(&mut *tx).await.expect("event");
    tx.commit().await.expect("paired custody");
    assert!(
        lookup::read_lookup(&pool, &binding.operation_id)
            .await
            .is_err()
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn continuation_lost_start_is_offline_unknown_and_never_gets(pool: PgPool) {
    let (clock, frozen) = synthetic_source(&pool).await;
    let id = &frozen.snapshot().binding.id;
    let ids = observation_cases::ids();
    continuation::acquire(&pool, clock.as_ref(), &ids, id)
        .await
        .expect("source");
    let _lost = continuation::start(&pool, clock.as_ref(), id)
        .await
        .expect("committed start");
    assert_eq!(
        continuation::inspect(&pool, id)
            .await
            .expect("offline unknown"),
        Some(ContinuationOutcome::Unknown)
    );
    let server = Server::start(vec![profile()]);
    let (client, _) = client(&server, false).await;
    assert_eq!(
        continuation::execute(
            &pool,
            &observation_cases::BadPorts,
            &observation_cases::BadPorts,
            id,
            &client
        )
        .await
        .expect("unknown replay"),
        ContinuationOutcome::Unknown
    );
    assert_eq!(server.count(), 1);
    get_only(&server);
    assert_eq!(counts(&pool).await, (1, 1, 1, 0, 1));
}

async fn wrong_get(pool: PgPool, path: &str, replacement: serde_json::Value) {
    let (clock, frozen) = synthetic_source(&pool).await;
    let id = &frozen.snapshot().binding.id;
    let ids = observation_cases::ids();
    continuation::acquire(&pool, clock.as_ref(), &ids, id)
        .await
        .expect("source");
    let mut invalid = payment(frozen.snapshot());
    *invalid.pointer_mut(path).expect("synthetic field") = replacement;
    let server = Server::start(vec![profile(), Reply::Json(200, invalid)]);
    let (client, _) = client(&server, false).await;
    assert_eq!(
        continuation::execute(&pool, clock.as_ref(), &ids, id, &client)
            .await
            .expect("unknown"),
        ContinuationOutcome::Unknown
    );
    assert_eq!(
        continuation::inspect(&pool, id)
            .await
            .expect("unknown custody"),
        Some(ContinuationOutcome::Unknown)
    );
    assert_eq!(
        continuation::execute(&pool, clock.as_ref(), &ids, id, &client)
            .await
            .expect("no retry"),
        ContinuationOutcome::Unknown
    );
    assert_eq!(server.count(), 2);
    get_only(&server);
    assert_eq!(counts(&pool).await, (1, 1, 1, 0, 1));
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn continuation_wrong_provider_id_stays_unknown(pool: PgPool) {
    wrong_get(pool, "/id", serde_json::json!("tr_OTHER_SYNTHETIC")).await;
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn continuation_wrong_original_attempt_stays_unknown(pool: PgPool) {
    wrong_get(
        pool,
        "/metadata/attempt_id",
        serde_json::json!("f".repeat(32)),
    )
    .await;
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn continuation_wrong_mode_stays_unknown(pool: PgPool) {
    wrong_get(pool, "/mode", serde_json::json!("live")).await;
}

async fn inherited_async_pool(pool: &PgPool) -> PgPool {
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
        .expect("owned async-default pool");
    let settings: (String, String) =
        sqlx::query_as("SELECT current_setting('synchronous_commit'),current_setting('fsync')")
            .fetch_one(&result)
            .await
            .expect("session flags");
    assert_eq!(settings, ("off".into(), "on".into()));
    result
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn continuation_forces_durable_start_and_receipt_from_async_session(pool: PgPool) {
    let (clock, frozen) = synthetic_source(&pool).await;
    let id = &frozen.snapshot().binding.id;
    let ids = observation_cases::ids();
    continuation::acquire(&pool, clock.as_ref(), &ids, id)
        .await
        .expect("source");
    pool.execute("CREATE FUNCTION assert_test_durable() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF current_setting('synchronous_commit')<>'on' OR current_setting('fsync')<>'on' THEN RAISE EXCEPTION 'not durable'; END IF; RETURN NEW; END $$; CREATE TRIGGER zz_assert_test_start BEFORE INSERT ON callback_lookup_fetch_starts FOR EACH ROW EXECUTE FUNCTION assert_test_durable(); CREATE TRIGGER zz_assert_test_receipt BEFORE INSERT ON callback_lookup_receipts FOR EACH ROW EXECUTE FUNCTION assert_test_durable()").await.expect("flag observers");
    let async_pool = inherited_async_pool(&pool).await;
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, payment(frozen.snapshot())),
    ]);
    let (client, _) = client(&server, false).await;
    assert_eq!(
        continuation::execute(&async_pool, clock.as_ref(), &ids, id, &client)
            .await
            .expect("execute"),
        ContinuationOutcome::Recorded { replayed: false }
    );
    assert_eq!(counts(&pool).await, (1, 1, 1, 1, 1));
    let after: String = sqlx::query_scalar("SHOW synchronous_commit")
        .fetch_one(&async_pool)
        .await
        .expect("session reset");
    assert_eq!(after, "off");
    assert_eq!(server.count(), 2);
    get_only(&server);
    async_pool.close().await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn continuation_durability_sabotage_rolls_back_both_start_entry_points(pool: PgPool) {
    let (clock, frozen) = synthetic_source(&pool).await;
    let id = &frozen.snapshot().binding.id;
    let ids = observation_cases::ids();
    continuation::acquire(&pool, clock.as_ref(), &ids, id)
        .await
        .expect("source");
    pool.execute("CREATE FUNCTION sabotage_test_durable() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM set_config('synchronous_commit','off',true); RETURN NEW; END $$; CREATE TRIGGER zz_sabotage_test_start BEFORE INSERT ON callback_lookup_fetch_starts FOR EACH ROW EXECUTE FUNCTION sabotage_test_durable()").await.expect("fault");
    let async_pool = inherited_async_pool(&pool).await;
    assert!(
        continuation::start(&async_pool, clock.as_ref(), id)
            .await
            .is_err()
    );
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, payment(frozen.snapshot())),
    ]);
    let (client, _) = client(&server, false).await;
    assert!(
        continuation::execute(&async_pool, clock.as_ref(), &ids, id, &client)
            .await
            .is_err()
    );
    assert_eq!(counts(&pool).await, (1, 1, 0, 0, 1));
    assert_eq!(server.count(), 1);
    pool.execute("DROP TRIGGER zz_sabotage_test_start ON callback_lookup_fetch_starts; DROP FUNCTION sabotage_test_durable()").await.expect("repair");
    assert_eq!(
        continuation::execute(&async_pool, clock.as_ref(), &ids, id, &client)
            .await
            .expect("healthy"),
        ContinuationOutcome::Recorded { replayed: false }
    );
    assert_eq!(server.count(), 2);
    get_only(&server);
    async_pool.close().await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn continuation_receipt_durability_sabotage_stays_unknown_without_retry(pool: PgPool) {
    let (clock, frozen) = synthetic_source(&pool).await;
    let id = &frozen.snapshot().binding.id;
    let ids = observation_cases::ids();
    continuation::acquire(&pool, clock.as_ref(), &ids, id)
        .await
        .expect("source");
    pool.execute("CREATE FUNCTION sabotage_test_durable() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM set_config('synchronous_commit','off',true); RETURN NEW; END $$; CREATE TRIGGER zz_sabotage_test_receipt BEFORE INSERT ON callback_lookup_receipts FOR EACH ROW EXECUTE FUNCTION sabotage_test_durable()").await.expect("fault");
    let async_pool = inherited_async_pool(&pool).await;
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, payment(frozen.snapshot())),
    ]);
    let (client, _) = client(&server, false).await;
    assert_eq!(
        continuation::execute(&async_pool, clock.as_ref(), &ids, id, &client)
            .await
            .expect("unknown"),
        ContinuationOutcome::Unknown
    );
    assert_eq!(counts(&pool).await, (1, 1, 1, 0, 1));
    pool.execute("DROP TRIGGER zz_sabotage_test_receipt ON callback_lookup_receipts; DROP FUNCTION sabotage_test_durable()").await.expect("repair");
    assert_eq!(
        continuation::execute(&async_pool, clock.as_ref(), &ids, id, &client)
            .await
            .expect("no retry"),
        ContinuationOutcome::Unknown
    );
    assert_eq!(server.count(), 2);
    get_only(&server);
    async_pool.close().await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn continuation_receipt_rejects_late_shadowed_durability_settings(pool: PgPool) {
    let (clock, frozen) = synthetic_source(&pool).await;
    let id = &frozen.snapshot().binding.id;
    let ids = observation_cases::ids();
    continuation::acquire(&pool, clock.as_ref(), &ids, id)
        .await
        .expect("source");
    pool.execute("CREATE FUNCTION public.current_setting(text) RETURNS text LANGUAGE sql STABLE AS $$ SELECT CASE WHEN $1 IN ('fsync','synchronous_commit') THEN 'on' ELSE pg_catalog.current_setting($1) END $$; CREATE FUNCTION public.fixture_false_durability_equal(text,text) RETURNS boolean LANGUAGE sql IMMUTABLE AS $$ SELECT CASE WHEN $1 OPERATOR(pg_catalog.=) 'off' AND $2 OPERATOR(pg_catalog.=) 'on' THEN true ELSE $1 OPERATOR(pg_catalog.=) $2 END $$; CREATE OPERATOR public.= (LEFTARG=pg_catalog.text,RIGHTARG=pg_catalog.text,FUNCTION=public.fixture_false_durability_equal); CREATE SEQUENCE public.fixture_continuation_path_witness; CREATE FUNCTION public.fixture_continuation_late_path() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_catalog.set_config('synchronous_commit','off',true); PERFORM pg_catalog.set_config('search_path','public,pg_catalog',true); IF pg_catalog.current_setting('search_path') OPERATOR(pg_catalog.=) 'public,pg_catalog' THEN PERFORM pg_catalog.nextval('public.fixture_continuation_path_witness'); ELSE RAISE EXCEPTION 'fixture path not installed'; END IF; RETURN NEW; END $$; CREATE TRIGGER zz_fixture_continuation_late_path AFTER INSERT ON public.callback_processing_events FOR EACH ROW EXECUTE FUNCTION public.fixture_continuation_late_path()")
        .await.expect("isolated operator and late-path fixture");
    let mut probe = pool.begin().await.expect("operator probe");
    sqlx::query("SET LOCAL search_path=public,pg_catalog")
        .execute(&mut *probe)
        .await
        .expect("hostile probe path");
    let (spoofed, real): (bool, bool) = sqlx::query_as(
        "SELECT 'off'::text='on'::text,'off'::text OPERATOR(pg_catalog.=) 'on'::text",
    )
    .fetch_one(&mut *probe)
    .await
    .expect("operator probe");
    assert_eq!((spoofed, real), (true, false));
    probe.rollback().await.expect("close probe");
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, payment(frozen.snapshot())),
    ]);
    let (client, _) = client(&server, false).await;
    assert_eq!(
        continuation::execute(&pool, clock.as_ref(), &ids, id, &client)
            .await
            .expect("unknown"),
        ContinuationOutcome::Unknown
    );
    let witness: (i64, bool) =
        sqlx::query_as("SELECT last_value,is_called FROM public.fixture_continuation_path_witness")
            .fetch_one(&pool)
            .await
            .expect("post-event path witness");
    assert_eq!(witness, (1, true));
    assert_eq!(counts(&pool).await, (1, 1, 1, 0, 1));
    assert_eq!(
        continuation::execute(&pool, clock.as_ref(), &ids, id, &client)
            .await
            .expect("no retry"),
        ContinuationOutcome::Unknown
    );
    assert_eq!(server.count(), 2);
    get_only(&server);
}

// Only the dedicated wrapper runs this ignored control against its own cluster.
async fn exclusive_fsync(pool: &PgPool, enabled: bool) {
    let marker = std::env::var("P1_POSTGRES_DISPOSABLE_MARKER").expect("exclusive marker");
    assert!(
        marker.len() == 64
            && marker
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
        "invalid exclusive marker"
    );
    assert!(
        std::env::var("APP_ENV").as_deref() == Ok("test"),
        "test mode required"
    );
    let expected = std::path::PathBuf::from(
        std::env::var("SARK_EXCLUSIVE_FSYNC_DATA").expect("exclusive directory"),
    );
    let root = expected.parent().expect("owned root");
    assert!(
        root.file_name()
            .and_then(|v| v.to_str())
            .is_some_and(|v| v.starts_with("sark-continuation-fsync-")),
        "invalid owned root"
    );
    assert!(
        expected.file_name().is_some_and(|v| v == "data"),
        "invalid owned data"
    );
    let mut connection = pool.acquire().await.expect("exclusive connection");
    let (actual_marker, actual_dir, role, local, others): (Option<String>, String, String, bool, i64) = sqlx::query_as("SELECT current_setting('sark.disposable_marker',true),current_setting('data_directory'),current_user::text,inet_server_addr()='127.0.0.1'::inet,(SELECT count(*) FROM pg_stat_activity WHERE backend_type='client backend' AND pid<>pg_backend_pid())").fetch_one(&mut *connection).await.expect("exclusive proof");
    assert!(
        actual_marker.as_deref() == Some(marker.as_str())
            && role == "p1_test"
            && local
            && others == 0,
        "exclusive cluster proof failed"
    );
    assert!(
        std::fs::canonicalize(&expected).ok() == std::fs::canonicalize(&actual_dir).ok()
            && expected.is_dir(),
        "exclusive directory proof failed"
    );
    let command = if enabled {
        "ALTER SYSTEM RESET fsync"
    } else {
        "ALTER SYSTEM SET fsync='off'"
    };
    sqlx::query(command)
        .execute(&mut *connection)
        .await
        .expect("owned cluster setting");
    let reloaded: bool = sqlx::query_scalar("SELECT pg_reload_conf()")
        .fetch_one(&mut *connection)
        .await
        .expect("owned reload");
    assert!(reloaded, "owned reload rejected");
    let expected_mode = if enabled { "on" } else { "off" };
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let actual: String = sqlx::query_scalar("SHOW fsync")
                .fetch_one(&mut *connection)
                .await
                .expect("active fsync");
            if actual == expected_mode {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("active fsync changed");
}

#[tokio::test]
#[ignore = "exclusive disposable cluster: scripts/test_continuation_fsync.py"]
async fn continuation_exclusive_fsync_off_denies_starts_and_receipt() {
    assert!(
        std::env::var("APP_ENV").as_deref() == Ok("test"),
        "test mode required"
    );
    let marker = std::env::var("P1_POSTGRES_DISPOSABLE_MARKER").expect("exclusive marker");
    assert!(
        marker.len() == 64
            && marker
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
        "invalid exclusive marker"
    );
    let expected = std::path::PathBuf::from(
        std::env::var("SARK_EXCLUSIVE_FSYNC_DATA").expect("exclusive directory"),
    );
    assert!(
        expected.is_dir()
            && expected.file_name().is_some_and(|v| v == "data")
            && expected
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|v| v.to_str())
                .is_some_and(|v| v.starts_with("sark-continuation-fsync-")),
        "invalid owned data"
    );
    let database = std::env::var("DATABASE_URL").expect("exclusive database");
    let address = url::Url::parse(&database).expect("exclusive URL");
    assert!(
        address.scheme() == "postgresql"
            && address.host_str() == Some("127.0.0.1")
            && address.username() == "p1_test"
            && address.path() == "/postgres"
            && address.query().is_none(),
        "invalid exclusive URL"
    );
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&database)
        .await
        .expect("exclusive cluster");
    // Verify ownership and one-client exclusivity before any configuration change.
    let initial: String = sqlx::query_scalar("SHOW fsync")
        .fetch_one(&pool)
        .await
        .expect("baseline fsync");
    assert_eq!(initial, "on");
    // This proves exclusive ownership before both the setting and migrations.
    exclusive_fsync(&pool, true).await;
    MIGRATOR.run(&pool).await.expect("isolated migrations");
    let (clock, frozen) = synthetic_source(&pool).await;
    let id = &frozen.snapshot().binding.id;
    let ids = observation_cases::ids();
    continuation::acquire(&pool, clock.as_ref(), &ids, id)
        .await
        .expect("source");
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, payment(frozen.snapshot())),
    ]);
    let (client, _) = client(&server, false).await;
    exclusive_fsync(&pool, false).await;
    assert!(matches!(
        continuation::start(&pool, clock.as_ref(), id).await,
        Err(crate::payment_attempts::PaymentStoreError::Unavailable)
    ));
    assert!(matches!(
        continuation::execute(&pool, clock.as_ref(), &ids, id, &client).await,
        Err(super::super::MollieError::Custody(
            crate::payment_attempts::PaymentStoreError::Unavailable
        ))
    ));
    assert_eq!(counts(&pool).await, (1, 1, 0, 0, 1));
    assert_eq!(server.count(), 1);
    exclusive_fsync(&pool, true).await;
    let permit = continuation::start(&pool, clock.as_ref(), id)
        .await
        .expect("fsync-on start");
    let fetched = client
        .fetch_continuation(permit)
        .await
        .expect("synthetic loopback GET");
    exclusive_fsync(&pool, false).await;
    assert!(matches!(
        continuation::record_fsync_test(&pool, clock.as_ref(), &ids, id, &fetched).await,
        Err(crate::payment_attempts::PaymentStoreError::Unavailable)
    ));
    assert_eq!(counts(&pool).await, (1, 1, 1, 0, 1));
    exclusive_fsync(&pool, true).await;
    continuation::record_fsync_test(&pool, clock.as_ref(), &ids, id, &fetched)
        .await
        .expect("fsync-on receipt");
    assert_eq!(
        continuation::inspect(&pool, id)
            .await
            .expect("offline receipt"),
        Some(ContinuationOutcome::Recorded { replayed: true })
    );
    assert_eq!(counts(&pool).await, (1, 1, 1, 1, 1));
    assert_eq!(server.count(), 2);
    get_only(&server);
    pool.close().await;
}
