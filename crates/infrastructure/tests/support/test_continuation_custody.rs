//! Synthetic database custody only; no provider or network operations.
use super::{
    observation_cases::{self, BadPorts},
    payment, wire,
};
use crate::{
    MIGRATOR,
    payment_attempts::{
        PaymentStoreError,
        callbacks::lookup::{self, continuation},
    },
};
use commerce_application::payment_requests::FrozenAttempt;
use commerce_domain::payment_attempts::observations::{ObservationIntent, ObservationSnapshot};
use sqlx::{Executor, PgPool};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

const OBS: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const EVENT: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

// Construct an explicitly synthetic, database-valid provenance source without I/O.
// SQL custody does not establish a real POST or authenticated provider response.
async fn synthetic_source(
    pool: &PgPool,
) -> (Arc<crate::payment_test_fixture::quote::Time>, FrozenAttempt) {
    let (clock, frozen) = observation_cases::prepared(pool, true).await;
    let (provider, _) = wire::payment(
        &serde_json::to_vec(&payment(frozen.snapshot())).expect("synthetic JSON"),
        frozen.snapshot(),
        None,
    )
    .expect("synthetic projection");
    let attempt = &frozen.snapshot().binding.id;
    let snapshot = ObservationSnapshot {
        schema_version: 1,
        id: OBS.into(),
        attempt_id: attempt.clone(),
        effect_key: format!("initial-fetch-{attempt}"),
        revision: 1,
        recorded_at: 1500,
        intent: ObservationIntent {
            payment: provider,
            credential_fingerprint: frozen
                .snapshot()
                .binding
                .profile
                .credential_fingerprint
                .clone(),
        },
    };
    let mut tx = pool.begin().await.expect("source transaction");
    sqlx::query("INSERT INTO payment_observations(id,attempt_id,effect_key,revision,recorded_at,provider_id,credential_fingerprint,status,snapshot) VALUES($1,$2,$3,1,1500,$4,$5,'paid',$6)")
        .bind(OBS).bind(attempt).bind(&snapshot.effect_key).bind(&snapshot.intent.payment.id).bind(&snapshot.intent.credential_fingerprint)
        .bind(serde_json::to_value(&snapshot).expect("snapshot")).execute(&mut *tx).await.expect("synthetic observation");
    sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) VALUES($1,$2,'payment.observation_recorded.v1',$3,1500)")
        .bind(EVENT).bind(&frozen.snapshot().binding.order_id).bind(serde_json::json!({"schema_version":1,"order_id":frozen.snapshot().binding.order_id,"attempt_id":attempt,"observation_id":OBS})).execute(&mut *tx).await.expect("synthetic event");
    sqlx::query("INSERT INTO payment_initial_dispatch_provenance(attempt_id,start_id,observation_id,observation_event_id,observation_sha256) SELECT $1,s.id,$2,$3,payment_initial_observation_digest(o) FROM payment_dispatch_starts s JOIN payment_observations o ON o.id=$2 AND o.attempt_id=s.attempt_id WHERE s.attempt_id=$1")
        .bind(attempt).bind(OBS).bind(EVENT).execute(&mut *tx).await.expect("synthetic provenance");
    tx.commit().await.expect("source commit");
    (clock, frozen)
}

async fn counts(pool: &PgPool) -> (i64, i64, i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM payment_test_continuations),(SELECT count(*) FROM callback_lookup_operations),(SELECT count(*) FROM callback_lookup_fetch_starts),(SELECT count(*) FROM callback_lookup_receipts),(SELECT count(*) FROM payment_observations)")
        .fetch_one(pool).await.expect("counts")
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn continuation_acquisition_rejects_changed_persisted_operation(pool: PgPool) {
    let (clock, frozen) = synthetic_source(&pool).await;
    let id = &frozen.snapshot().binding.id;
    let ids = observation_cases::ids();
    pool.execute(
        "CREATE SEQUENCE public.acquisition_readback_witness;
         CREATE FUNCTION public.change_continuation_operation() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN
             PERFORM pg_catalog.nextval('public.acquisition_readback_witness');
             NEW.created_at := NEW.created_at + 1;
             RETURN NEW;
         END $$;
         CREATE TRIGGER zz_change_continuation_operation BEFORE INSERT ON public.callback_lookup_operations
         FOR EACH ROW WHEN (NEW.continuation_attempt_id IS NOT NULL)
         EXECUTE FUNCTION public.change_continuation_operation()",
    )
    .await
    .expect("isolated payload mutation");
    assert!(matches!(
        continuation::acquire(&pool, clock.as_ref(), &ids, id).await,
        Err(PaymentStoreError::InvalidStoredData)
    ));
    let witness: (i64, bool) =
        sqlx::query_as("SELECT last_value,is_called FROM public.acquisition_readback_witness")
            .fetch_one(&pool)
            .await
            .expect("mutation reached");
    assert_eq!(witness, (1, true));
    assert_eq!(counts(&pool).await, (0, 0, 0, 0, 1));
    pool.execute(
        "DROP TRIGGER zz_change_continuation_operation ON public.callback_lookup_operations",
    )
    .await
    .expect("exact source restored");
    let created = continuation::acquire(&pool, clock.as_ref(), &ids, id)
        .await
        .expect("healthy acquisition");
    let operation = lookup::read_operation(&pool, &created.operation_id)
        .await
        .expect("stored operation");
    assert_eq!(operation.snapshot().created_at, 1500);
    assert_eq!(counts(&pool).await, (1, 1, 0, 0, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn offline_continuation_inspection_fails_closed_on_corrupt_start_time(pool: PgPool) {
    let (clock, frozen) = synthetic_source(&pool).await;
    let id = &frozen.snapshot().binding.id;
    let binding = continuation::acquire(&pool, clock.as_ref(), &observation_cases::ids(), id)
        .await
        .expect("binding");
    let start = continuation::start(&pool, clock.as_ref(), id)
        .await
        .expect("retained start");
    assert_eq!(start.started_at(), 1500);
    for (time, valid) in [(1499_i64, false), (1500, true)] {
        let mut tx = pool
            .begin()
            .await
            .expect("isolated historical fault transaction");
        sqlx::query("ALTER TABLE public.callback_lookup_fetch_starts DISABLE TRIGGER USER")
            .execute(&mut *tx)
            .await
            .expect("isolated history edit");
        let changed = sqlx::query(
            "UPDATE public.callback_lookup_fetch_starts SET started_at=$2 WHERE operation_id=$1",
        )
        .bind(&binding.operation_id)
        .bind(time)
        .execute(&mut *tx)
        .await
        .expect("change one start");
        assert_eq!(changed.rows_affected(), 1);
        sqlx::query("ALTER TABLE public.callback_lookup_fetch_starts ENABLE TRIGGER USER")
            .execute(&mut *tx)
            .await
            .expect("restore guards");
        tx.commit().await.expect("isolated edit");
        let fenced: bool = sqlx::query_scalar("SELECT public.payment_test_continuation_fenced($1)")
            .bind(id)
            .fetch_one(&pool)
            .await
            .expect("exact native proof");
        assert_eq!(fenced, valid);
        let inspection = continuation::inspect(&pool, id).await;
        if valid {
            assert_eq!(
                inspection,
                Ok(Some(continuation::ContinuationOutcome::Unknown))
            );
        } else {
            assert_eq!(inspection, Err(PaymentStoreError::InvalidStoredData));
        }
    }
    assert_eq!(counts(&pool).await, (1, 1, 1, 0, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn continuation_concurrent_acquisition_and_recovery_are_once_only(pool: PgPool) {
    let (clock, frozen) = synthetic_source(&pool).await;
    let id = &frozen.snapshot().binding.id;
    assert!(continuation::read(&pool, id).await.expect("read").is_none());
    let before: (i64,i64,i64) = sqlx::query_as("SELECT (SELECT count(*) FROM commerce_outbox),(SELECT count(*) FROM stock_movements),(SELECT count(*) FROM payment_recognitions)").fetch_one(&pool).await.expect("business effects");
    let ids = Arc::new(crate::payment_test_fixture::Ids(AtomicU64::new(25000)));
    let mut blocker = pool.begin().await.expect("blocker");
    sqlx::query("SELECT id FROM payment_attempts WHERE id=$1 FOR UPDATE")
        .bind(id)
        .execute(&mut *blocker)
        .await
        .expect("hold attempt");
    let task = |pool: PgPool,
                clock: Arc<crate::payment_test_fixture::quote::Time>,
                ids: Arc<crate::payment_test_fixture::Ids>,
                id: String| {
        tokio::spawn(async move {
            continuation::acquire(&pool, clock.as_ref(), ids.as_ref(), &id).await
        })
    };
    let a = task(pool.clone(), clock.clone(), ids.clone(), id.clone());
    let b = task(pool.clone(), clock.clone(), ids.clone(), id.clone());
    tokio::time::timeout(std::time::Duration::from_secs(1),async { loop {
        let waiting: i64=sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'SELECT left(basket_id,33) FROM payment_attempts%'").fetch_one(&pool).await.expect("waiters");
        if waiting>=2 { break; } tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }}).await.expect("actual concurrent attempt wait");
    blocker.commit().await.expect("release attempt");
    let (a, b) = (
        a.await.expect("task").expect("first"),
        b.await.expect("task").expect("second"),
    );
    assert_eq!(a.operation_id, b.operation_id);
    assert_ne!(a.replayed, b.replayed);
    assert_eq!(counts(&pool).await, (1, 1, 0, 0, 1));
    let after: (i64,i64,i64) = sqlx::query_as("SELECT (SELECT count(*) FROM commerce_outbox),(SELECT count(*) FROM stock_movements),(SELECT count(*) FROM payment_recognitions)").fetch_one(&pool).await.expect("business effects");
    assert_eq!(after, before);
    let reread = continuation::read(&pool, id)
        .await
        .expect("read")
        .expect("saved");
    assert_eq!(reread.operation_id, a.operation_id);
    let replay = continuation::acquire(&pool, &BadPorts, &BadPorts, id)
        .await
        .expect("clockless replay");
    assert!(replay.replayed);
    assert_eq!(replay.operation_id, a.operation_id);
    let operation = lookup::read_operation(&pool, &a.operation_id)
        .await
        .expect("existing lookup");
    assert_eq!(
        operation.snapshot().profile_id,
        frozen.snapshot().binding.profile.id
    );
    assert_eq!(
        operation.snapshot().provider_id,
        "tr_SYNTHETIC_NotARealPayment"
    );
    assert_eq!(counts(&pool).await, (1, 1, 0, 0, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn continuation_rejects_missing_or_corrupt_source_and_clock_failure(pool: PgPool) {
    let (clock, frozen) = observation_cases::prepared(&pool, true).await;
    let id = &frozen.snapshot().binding.id;
    let ids = observation_cases::ids();
    assert!(matches!(
        continuation::acquire(&pool, clock.as_ref(), &ids, id).await,
        Err(PaymentStoreError::Unavailable)
    ));
    assert_eq!(counts(&pool).await, (0, 0, 0, 0, 0));
    // Separate fixture exercises malformed source in its own isolated database.
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn continuation_failure_rolls_back_operation_and_repairs_source(pool: PgPool) {
    let (clock, frozen) = synthetic_source(&pool).await;
    let id = &frozen.snapshot().binding.id;
    let ids = observation_cases::ids();
    assert!(matches!(
        continuation::acquire(&pool, &BadPorts, &ids, id).await,
        Err(PaymentStoreError::ClockUnavailable)
    ));
    pool.execute("ALTER TABLE commerce_outbox DISABLE TRIGGER USER; UPDATE commerce_outbox SET payload='{}' WHERE id='bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb'; ALTER TABLE commerce_outbox ENABLE TRIGGER USER").await.expect("damage source");
    assert!(matches!(
        continuation::acquire(&pool, clock.as_ref(), &ids, id).await,
        Err(PaymentStoreError::InvalidStoredData)
    ));
    assert_eq!(counts(&pool).await, (0, 0, 0, 0, 1));
    pool.execute("ALTER TABLE commerce_outbox DISABLE TRIGGER USER")
        .await
        .expect("repair setup");
    sqlx::query("UPDATE commerce_outbox SET payload=jsonb_build_object('schema_version',1,'order_id',$1,'attempt_id',$2,'observation_id',$3) WHERE id=$4")
        .bind(&frozen.snapshot().binding.order_id).bind(id).bind(OBS).bind(EVENT).execute(&pool).await.expect("repair event");
    pool.execute("ALTER TABLE commerce_outbox ENABLE TRIGGER USER")
        .await
        .expect("restore guard");
    pool.execute("CREATE FUNCTION suppress_test_continuation() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RETURN NULL; END $$; CREATE TRIGGER zz_suppress_test_continuation BEFORE INSERT ON payment_test_continuations FOR EACH ROW EXECUTE FUNCTION suppress_test_continuation()")
        .await.expect("fault");
    assert!(
        continuation::acquire(&pool, clock.as_ref(), &ids, id)
            .await
            .is_err()
    );
    assert_eq!(counts(&pool).await, (0, 0, 0, 0, 1));
    pool.execute("DROP TRIGGER zz_suppress_test_continuation ON payment_test_continuations")
        .await
        .expect("remove fault");
    assert!(
        !continuation::acquire(&pool, clock.as_ref(), &ids, id)
            .await
            .expect("repaired")
            .replayed
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn continuation_rejects_invalid_attempt_ids_before_clock_or_effects(pool: PgPool) {
    let (clock, frozen) = synthetic_source(&pool).await;
    let before = counts(&pool).await;
    let overlong = "a".repeat(33);
    for invalid in [
        "",
        "bad",
        "g0000000000000000000000000000000",
        overlong.as_str(),
    ] {
        assert!(matches!(
            continuation::read(&pool, invalid).await,
            Err(PaymentStoreError::InvalidRequest)
        ));
        assert!(matches!(
            continuation::acquire(&pool, &BadPorts, &BadPorts, invalid).await,
            Err(PaymentStoreError::InvalidRequest)
        ));
        assert!(matches!(
            continuation::start(&pool, &BadPorts, invalid).await,
            Err(PaymentStoreError::InvalidRequest)
        ));
        assert_eq!(counts(&pool).await, before);
    }
    let receipt = continuation::acquire(
        &pool,
        clock.as_ref(),
        &observation_cases::ids(),
        &frozen.snapshot().binding.id,
    )
    .await
    .expect("valid source still acquires");
    assert!(!receipt.replayed);
    assert_eq!(counts(&pool).await, (1, 1, 0, 0, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn continuation_clock_before_source_or_operation_never_creates_work(pool: PgPool) {
    let (clock, frozen) = synthetic_source(&pool).await;
    let id = &frozen.snapshot().binding.id;
    let ids = observation_cases::ids();
    clock.0.store(1499, Ordering::SeqCst);
    assert!(matches!(
        continuation::acquire(&pool, clock.as_ref(), &ids, id).await,
        Err(PaymentStoreError::ClockUnavailable)
    ));
    assert_eq!(counts(&pool).await, (0, 0, 0, 0, 1));
    clock.0.store(1500, Ordering::SeqCst);
    let binding = continuation::acquire(&pool, clock.as_ref(), &ids, id)
        .await
        .expect("source-time acquisition");
    assert!(!binding.replayed);
    clock.0.store(1499, Ordering::SeqCst);
    assert!(matches!(
        continuation::start(&pool, clock.as_ref(), id).await,
        Err(PaymentStoreError::ClockUnavailable)
    ));
    assert_eq!(counts(&pool).await, (1, 1, 0, 0, 1));
    clock.0.store(1500, Ordering::SeqCst);
    let start = continuation::start(&pool, clock.as_ref(), id)
        .await
        .expect("operation-time start");
    assert_eq!(start.operation_id(), binding.operation_id);
    assert_eq!(start.started_at(), 1500);
    assert_eq!(counts(&pool).await, (1, 1, 1, 0, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn continuation_native_guards_enforce_reciprocity_and_immutable_scope(pool: PgPool) {
    let (clock, frozen) = synthetic_source(&pool).await;
    let attempt = &frozen.snapshot().binding.id;
    let profile = &frozen.snapshot().binding.profile.id;
    let ids = observation_cases::ids();
    let mut orphan = pool.begin().await.expect("transaction");
    sqlx::query("INSERT INTO callback_lookup_operations(id,profile_id,provider_id,created_at,continuation_attempt_id) VALUES($1,$2,'tr_SYNTHETIC_NotARealPayment',1500,$3)")
        .bind("cccccccccccccccccccccccccccccccc").bind(profile).bind(attempt).execute(&mut *orphan).await.expect("marked operation");
    assert!(orphan.commit().await.is_err());
    let generic = lookup::prepare_lookup(
        &pool,
        clock.as_ref(),
        &ids,
        profile,
        "tr_SYNTHETIC_NotARealPayment",
    )
    .await
    .expect("generic operation");
    assert!(
        sqlx::query(
            "INSERT INTO payment_test_continuations(attempt_id,operation_id) VALUES($1,$2)"
        )
        .bind(attempt)
        .bind(&generic.snapshot().id)
        .execute(&pool)
        .await
        .is_err()
    );
    let mut wrong = pool.begin().await.expect("transaction");
    sqlx::query("INSERT INTO callback_lookup_operations(id,profile_id,provider_id,created_at,continuation_attempt_id) VALUES($1,$2,'tr_WRONG',1500,$3)")
        .bind("dddddddddddddddddddddddddddddddd").bind(profile).bind(attempt).execute(&mut *wrong).await.expect("wrong identity operation");
    assert!(
        sqlx::query(
            "INSERT INTO payment_test_continuations(attempt_id,operation_id) VALUES($1,$2)"
        )
        .bind(attempt)
        .bind("dddddddddddddddddddddddddddddddd")
        .execute(&mut *wrong)
        .await
        .is_err()
    );
    wrong.rollback().await.expect("rollback");
    let receipt = continuation::acquire(&pool, clock.as_ref(), &ids, attempt)
        .await
        .expect("valid");
    assert_ne!(receipt.operation_id, generic.snapshot().id);
    assert!(
        sqlx::query(
            "UPDATE payment_test_continuations SET operation_id=operation_id WHERE attempt_id=$1"
        )
        .bind(attempt)
        .execute(&pool)
        .await
        .is_err()
    );
    assert!(
        sqlx::query("DELETE FROM payment_test_continuations WHERE attempt_id=$1")
            .bind(attempt)
            .execute(&pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("UPDATE callback_lookup_operations SET provider_id='tr_WRONG' WHERE id=$1")
            .bind(&receipt.operation_id)
            .execute(&pool)
            .await
            .is_err()
    );
    assert_eq!(counts(&pool).await, (1, 2, 0, 0, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn continuation_native_source_checks_ignore_shadowed_xid_and_helper(pool: PgPool) {
    let (_clock, frozen) = synthetic_source(&pool).await;
    let attempt = &frozen.snapshot().binding.id;
    let profile = &frozen.snapshot().binding.profile.id;
    pool.execute("CREATE SCHEMA fixture_continuation_shadow; CREATE FUNCTION fixture_continuation_shadow.pg_current_xact_id() RETURNS pg_catalog.xid8 LANGUAGE sql STABLE AS $$ SELECT '42'::pg_catalog.xid8 $$; CREATE FUNCTION fixture_continuation_shadow.payment_test_continuation_exact(public.payment_test_continuations) RETURNS boolean LANGUAGE sql IMMUTABLE AS $$ SELECT true $$")
        .await.expect("isolated shadow functions");
    let mut wrong = pool.begin().await.expect("wrong source transaction");
    sqlx::query("SET LOCAL search_path=fixture_continuation_shadow,pg_catalog,public")
        .execute(&mut *wrong)
        .await
        .expect("hostile path");
    let xid: (String, String) =
        sqlx::query_as("SELECT pg_current_xact_id()::text,pg_catalog.pg_current_xact_id()::text")
            .fetch_one(&mut *wrong)
            .await
            .expect("spoof and real XID");
    assert_eq!(xid.0, "42");
    assert_ne!(xid.0, xid.1);
    sqlx::query("INSERT INTO public.callback_lookup_operations(id,profile_id,provider_id,created_at,continuation_attempt_id) VALUES($1,$2,'tr_WRONG',1500,$3)")
        .bind("c".repeat(32)).bind(profile).bind(attempt).execute(&mut *wrong).await.expect("wrong operation");
    let assigned: String = sqlx::query_scalar("SELECT source_transaction_id::text FROM public.callback_lookup_operations WHERE id OPERATOR(pg_catalog.=) $1")
        .bind("c".repeat(32)).fetch_one(&mut *wrong).await.expect("assigned operation XID");
    assert_eq!(assigned, xid.1);
    let error = sqlx::query(
        "INSERT INTO public.payment_test_continuations(attempt_id,operation_id) VALUES($1,$2)",
    )
    .bind(attempt)
    .bind("c".repeat(32))
    .execute(&mut *wrong)
    .await
    .expect_err("shadow helper cannot approve wrong source");
    assert_eq!(
        error.as_database_error().map(|e| e.message()),
        Some("Test continuation requires its exact fresh operation and source")
    );
    wrong.rollback().await.expect("discard wrong source");

    let mut valid = pool.begin().await.expect("valid transaction");
    sqlx::query("SET LOCAL search_path=fixture_continuation_shadow,pg_catalog,public")
        .execute(&mut *valid)
        .await
        .expect("hostile path");
    let real: String = sqlx::query_scalar("SELECT pg_catalog.pg_current_xact_id()::text")
        .fetch_one(&mut *valid)
        .await
        .expect("real XID");
    assert_ne!(real, xid.1);
    sqlx::query("INSERT INTO public.callback_lookup_operations(id,profile_id,provider_id,created_at,continuation_attempt_id) VALUES($1,$2,'tr_SYNTHETIC_NotARealPayment',1500,$3)")
        .bind("d".repeat(32)).bind(profile).bind(attempt).execute(&mut *valid).await.expect("valid operation");
    sqlx::query(
        "INSERT INTO public.payment_test_continuations(attempt_id,operation_id) VALUES($1,$2)",
    )
    .bind(attempt)
    .bind("d".repeat(32))
    .execute(&mut *valid)
    .await
    .expect("valid continuation");
    let pair: (String, String) = sqlx::query_as("SELECT o.source_transaction_id::text,c.transaction_id::text FROM public.callback_lookup_operations o JOIN public.payment_test_continuations c ON c.operation_id OPERATOR(pg_catalog.=) o.id WHERE o.id OPERATOR(pg_catalog.=) $1")
        .bind("d".repeat(32)).fetch_one(&mut *valid).await.expect("same real transaction");
    assert_eq!(pair, (real.clone(), real));
    valid.commit().await.expect("native reciprocal commit");
    assert_eq!(
        continuation::read(&pool, attempt)
            .await
            .expect("read")
            .expect("saved")
            .operation_id,
        "d".repeat(32)
    );
    assert_eq!(counts(&pool).await, (1, 1, 0, 0, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn continuation_orphaned_mapping_is_not_replaced_and_exact_repair_recovers(pool: PgPool) {
    let (clock, frozen) = synthetic_source(&pool).await;
    let id = &frozen.snapshot().binding.id;
    let ids = observation_cases::ids();
    let saved = continuation::acquire(&pool, clock.as_ref(), &ids, id)
        .await
        .expect("acquire");
    let xid: String = sqlx::query_scalar(
        "SELECT transaction_id::text FROM payment_test_continuations WHERE attempt_id=$1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .expect("transaction");
    pool.execute("ALTER TABLE payment_test_continuations DISABLE TRIGGER USER")
        .await
        .expect("fault setup");
    sqlx::query("DELETE FROM payment_test_continuations WHERE attempt_id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .expect("orphan operation");
    pool.execute("ALTER TABLE payment_test_continuations ENABLE TRIGGER USER")
        .await
        .expect("restore guard");
    assert!(matches!(
        continuation::read(&pool, id).await,
        Err(PaymentStoreError::InvalidStoredData)
    ));
    assert!(matches!(
        continuation::acquire(&pool, clock.as_ref(), &ids, id).await,
        Err(PaymentStoreError::InvalidStoredData)
    ));
    assert_eq!(counts(&pool).await, (0, 1, 0, 0, 1));
    pool.execute("ALTER TABLE payment_test_continuations DISABLE TRIGGER USER")
        .await
        .expect("repair setup");
    sqlx::query("INSERT INTO payment_test_continuations(attempt_id,operation_id,transaction_id) VALUES($1,$2,$3::text::xid8)").bind(id).bind(&saved.operation_id).bind(&xid).execute(&pool).await.expect("exact repair");
    pool.execute("ALTER TABLE payment_test_continuations ENABLE TRIGGER USER")
        .await
        .expect("restore guard");
    assert_eq!(
        continuation::read(&pool, id)
            .await
            .expect("read")
            .expect("saved")
            .operation_id,
        saved.operation_id
    );
    assert_eq!(counts(&pool).await, (1, 1, 0, 0, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn continuation_rejects_equal_length_coherent_source_rewrite_until_repair(pool: PgPool) {
    let (clock, frozen) = synthetic_source(&pool).await;
    let id = &frozen.snapshot().binding.id;
    let ids = observation_cases::ids();
    let saved = continuation::acquire(&pool, clock.as_ref(), &ids, id)
        .await
        .expect("original operation");
    let original: (String, serde_json::Value) =
        sqlx::query_as("SELECT provider_id,snapshot FROM payment_observations WHERE attempt_id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .expect("source");
    let replacement = "tr_SYNTHETIC_NotARealPaymenX";
    assert_ne!(original.0, replacement);
    assert_eq!(original.0.len(), replacement.len(), "UTF-8 byte lengths");
    pool.execute("ALTER TABLE payment_observations DISABLE TRIGGER USER")
        .await
        .expect("fault setup");
    sqlx::query("UPDATE payment_observations SET provider_id=$2,snapshot=jsonb_set(snapshot,'{intent,payment,id}',to_jsonb($2::text)) WHERE attempt_id=$1").bind(id).bind(replacement).execute(&pool).await.expect("coherent rewrite");
    pool.execute("ALTER TABLE payment_observations ENABLE TRIGGER USER")
        .await
        .expect("restore guard");
    assert!(matches!(
        continuation::read(&pool, id).await,
        Err(PaymentStoreError::InvalidStoredData)
    ));
    assert!(matches!(
        continuation::acquire(&pool, clock.as_ref(), &ids, id).await,
        Err(PaymentStoreError::InvalidStoredData)
    ));
    assert!(matches!(
        continuation::start(&pool, clock.as_ref(), id).await,
        Err(PaymentStoreError::InvalidStoredData)
    ));
    assert_eq!(counts(&pool).await, (1, 1, 0, 0, 1));
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
    assert_eq!(
        continuation::read(&pool, id)
            .await
            .expect("read")
            .expect("source")
            .operation_id,
        saved.operation_id
    );
    assert_eq!(counts(&pool).await, (1, 1, 0, 0, 1));
}

async fn insert_supervisor(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    attempt: &str,
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query("INSERT INTO payment_reconciliation_jobs(attempt_id,source_event_id,created_at,updated_at,due_at) SELECT s.attempt_id,e.id,s.started_at,1500,1500 FROM payment_dispatch_starts s JOIN payment_attempts a ON a.id=s.attempt_id JOIN commerce_outbox e ON e.aggregate_id=a.order_id AND e.kind='payment.reconciliation_required.v1' AND e.created_at=s.started_at AND e.payload=jsonb_build_object('schema_version',1,'order_id',a.order_id,'attempt_id',a.id,'dispatch_id',s.id) WHERE s.attempt_id=$1")
        .bind(attempt).execute(&mut **tx).await?;
    Ok(result.rows_affected())
}

async fn wait_for_blocker(pool: &PgPool, blocker: i32, query: &str) {
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE $2)")
                .bind(blocker).bind(query).fetch_one(pool).await.expect("waiter");
            if waiting { break; }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }).await.expect("actual dispatch lock wait");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn continuation_generic_paths_reject_marked_and_legacy_stays_usable(pool: PgPool) {
    let (clock, frozen) = synthetic_source(&pool).await;
    let id = &frozen.snapshot().binding.id;
    let ids = observation_cases::ids();
    let binding = continuation::acquire(&pool, clock.as_ref(), &ids, id)
        .await
        .expect("binding");
    let operation = lookup::read_operation(&pool, &binding.operation_id)
        .await
        .expect("read operation");
    assert!(matches!(
        continuation::start(&pool, &BadPorts, id).await,
        Err(PaymentStoreError::ClockUnavailable)
    ));
    assert_eq!(counts(&pool).await, (1, 1, 0, 0, 1));
    assert!(matches!(
        lookup::start_lookup(&pool, clock.as_ref(), &operation).await,
        Err(PaymentStoreError::Unavailable)
    ));
    assert!(matches!(
        lookup::read_lookup(&pool, &binding.operation_id).await,
        Err(PaymentStoreError::Unavailable)
    ));
    assert!(matches!(
        lookup::process_lookup(&pool, clock.as_ref(), &ids, &binding.operation_id).await,
        Err(PaymentStoreError::Unavailable)
    ));
    let legacy = lookup::prepare_lookup(
        &pool,
        clock.as_ref(),
        &ids,
        &frozen.snapshot().binding.profile.id,
        "tr_SYNTHETIC_Independent",
    )
    .await
    .expect("legacy operation");
    let _permit = lookup::start_lookup(&pool, clock.as_ref(), &legacy)
        .await
        .expect("legacy start");
    let marked = continuation::start(&pool, clock.as_ref(), id)
        .await
        .expect("marked start");
    assert_eq!(marked.operation_id(), binding.operation_id);
    assert!(marked.started_at() >= operation.snapshot().created_at);
    assert!(matches!(
        continuation::start(&pool, clock.as_ref(), id).await,
        Err(PaymentStoreError::Unavailable)
    ));
    assert_eq!(counts(&pool).await, (1, 2, 2, 0, 1));
    let mut job = pool.begin().await.expect("job");
    assert_eq!(
        insert_supervisor(&mut job, id)
            .await
            .expect("fenced candidate skipped"),
        0
    );
    job.rollback().await.expect("rollback");
    crate::payment_attempts::reconciliation::jobs::enqueue(
        &pool,
        clock.as_ref(),
        &frozen.snapshot().binding.profile.id,
    )
    .await
    .expect("fenced attempt skipped");
    assert!(
        crate::payment_attempts::reconciliation::read_job(&pool, id)
            .await
            .expect("read")
            .is_none()
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn continuation_start_wins_against_waiting_supervisor(pool: PgPool) {
    let (clock, frozen) = synthetic_source(&pool).await;
    let id = frozen.snapshot().binding.id.clone();
    let binding = continuation::acquire(&pool, clock.as_ref(), &observation_cases::ids(), &id)
        .await
        .expect("binding");
    let mut winner = pool.begin().await.expect("start transaction");
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *winner)
        .await
        .expect("pid");
    sqlx::query("SELECT id FROM payment_attempts WHERE id=$1 FOR UPDATE")
        .bind(&id)
        .execute(&mut *winner)
        .await
        .expect("attempt");
    sqlx::query("SELECT attempt_id FROM payment_dispatch_starts WHERE attempt_id=$1 FOR UPDATE")
        .bind(&id)
        .execute(&mut *winner)
        .await
        .expect("dispatch");
    sqlx::query(
        "INSERT INTO callback_lookup_fetch_starts(operation_id,started_at) VALUES($1,1500)",
    )
    .bind(&binding.operation_id)
    .execute(&mut *winner)
    .await
    .expect("marked start");
    let p = pool.clone();
    let attempt = id.clone();
    let loser = tokio::spawn(async move {
        let mut tx = p.begin().await.expect("job tx");
        insert_supervisor(&mut tx, &attempt).await
    });
    wait_for_blocker(&pool, pid, "INSERT INTO payment_reconciliation_jobs%").await;
    winner.commit().await.expect("commit start");
    assert_eq!(
        loser.await.expect("job task").expect("candidate skipped"),
        0
    );
    assert!(matches!(
        continuation::start(&pool, clock.as_ref(), &id).await,
        Err(PaymentStoreError::Unavailable)
    ));
    let jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM payment_reconciliation_jobs")
        .fetch_one(&pool)
        .await
        .expect("jobs");
    assert_eq!(jobs, 0);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn supervisor_wins_against_waiting_continuation_start(pool: PgPool) {
    let (clock, frozen) = synthetic_source(&pool).await;
    let id = frozen.snapshot().binding.id.clone();
    continuation::acquire(&pool, clock.as_ref(), &observation_cases::ids(), &id)
        .await
        .expect("binding");
    let mut winner = pool.begin().await.expect("job transaction");
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *winner)
        .await
        .expect("pid");
    assert_eq!(
        insert_supervisor(&mut winner, &id)
            .await
            .expect("job insert"),
        1
    );
    let p = pool.clone();
    let time = clock.clone();
    let attempt = id.clone();
    let loser = tokio::spawn(async move { continuation::start(&p, time.as_ref(), &attempt).await });
    wait_for_blocker(
        &pool,
        pid,
        "SELECT attempt_id FROM public.payment_dispatch_starts%FOR UPDATE",
    )
    .await;
    winner.commit().await.expect("commit job");
    assert!(loser.await.expect("start task").is_err());
    let starts: i64 = sqlx::query_scalar("SELECT count(*) FROM callback_lookup_fetch_starts")
        .fetch_one(&pool)
        .await
        .expect("starts");
    assert_eq!(starts, 0);
    assert_eq!(
        crate::payment_attempts::reconciliation::read_job(&pool, &id)
            .await
            .expect("legacy job")
            .expect("job")
            .generation,
        0
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn supervisor_legacy_enqueue_remains_eligible(pool: PgPool) {
    let (clock, frozen) = observation_cases::prepared(&pool, true).await;
    let id = &frozen.snapshot().binding.id;
    crate::payment_attempts::reconciliation::jobs::enqueue(
        &pool,
        clock.as_ref(),
        &frozen.snapshot().binding.profile.id,
    )
    .await
    .expect("legacy enqueue");
    assert_eq!(
        crate::payment_attempts::reconciliation::read_job(&pool, id)
            .await
            .expect("read")
            .expect("job")
            .generation,
        0
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn continuation_fence_rejects_stale_isolation_on_both_paths(pool: PgPool) {
    let (clock, frozen) = synthetic_source(&pool).await;
    let id = &frozen.snapshot().binding.id;
    let binding = continuation::acquire(&pool, clock.as_ref(), &observation_cases::ids(), id)
        .await
        .expect("binding");
    pool.execute("CREATE FUNCTION public.current_setting(text) RETURNS text LANGUAGE sql STABLE AS $$ SELECT CASE WHEN $1 OPERATOR(pg_catalog.=) 'transaction_isolation' THEN 'read committed' ELSE pg_catalog.current_setting($1) END $$")
        .await.expect("isolated isolation spoof");
    let mut tx = pool.begin().await.expect("transaction");
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .execute(&mut *tx)
        .await
        .expect("isolation");
    sqlx::query("SET LOCAL search_path=public,pg_catalog")
        .execute(&mut *tx)
        .await
        .expect("hostile path");
    let values: (String, String) = sqlx::query_as("SELECT current_setting('transaction_isolation'::text),pg_catalog.current_setting('transaction_isolation')")
        .fetch_one(&mut *tx).await.expect("actual and spoofed isolation");
    assert_eq!(values, ("read committed".into(), "repeatable read".into()));
    let error = sqlx::query(
        "INSERT INTO callback_lookup_fetch_starts(operation_id,started_at) VALUES($1,1500)",
    )
    .bind(&binding.operation_id)
    .execute(&mut *tx)
    .await
    .expect_err("native fetch fence refuses repeatable read");
    assert_eq!(
        error.as_database_error().map(|e| e.message()),
        Some("Continuation arbitration requires read committed")
    );
    tx.rollback().await.expect("rollback");
    let mut tx = pool.begin().await.expect("transaction");
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .execute(&mut *tx)
        .await
        .expect("isolation");
    sqlx::query("SET LOCAL search_path=public,pg_catalog")
        .execute(&mut *tx)
        .await
        .expect("hostile path");
    let values: (String, String) = sqlx::query_as("SELECT current_setting('transaction_isolation'::text),pg_catalog.current_setting('transaction_isolation')")
        .fetch_one(&mut *tx).await.expect("actual and spoofed isolation");
    assert_eq!(values, ("read committed".into(), "repeatable read".into()));
    let error = insert_supervisor(&mut tx, id)
        .await
        .expect_err("native supervisor fence refuses repeatable read");
    assert_eq!(
        error.as_database_error().map(|e| e.message()),
        Some("Supervisor arbitration requires read committed")
    );
    tx.rollback().await.expect("rollback");
    assert_eq!(counts(&pool).await, (1, 1, 0, 0, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn continuation_fenced_candidate_does_not_abort_actual_enqueue_batch(pool: PgPool) {
    let (clock, first) = synthetic_source(&pool).await;
    let profile = first.snapshot().binding.profile.id.clone();
    let second = super::identity_ownership::additional(&pool, clock.clone(), &profile).await;
    let first_id = first.snapshot().binding.id.clone();
    let second_id = second.snapshot().binding.id.clone();
    assert_ne!(first_id, second_id);
    let binding =
        continuation::acquire(&pool, clock.as_ref(), &observation_cases::ids(), &first_id)
            .await
            .expect("binding");
    let mut start = pool.begin().await.expect("start transaction");
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *start)
        .await
        .expect("pid");
    sqlx::query("SELECT id FROM payment_attempts WHERE id=$1 FOR UPDATE")
        .bind(&first_id)
        .execute(&mut *start)
        .await
        .expect("attempt");
    sqlx::query("SELECT attempt_id FROM payment_dispatch_starts WHERE attempt_id=$1 FOR UPDATE")
        .bind(&first_id)
        .execute(&mut *start)
        .await
        .expect("dispatch");
    sqlx::query(
        "INSERT INTO callback_lookup_fetch_starts(operation_id,started_at) VALUES($1,1500)",
    )
    .bind(&binding.operation_id)
    .execute(&mut *start)
    .await
    .expect("uncommitted start");
    let p = pool.clone();
    let time = clock.clone();
    let profile_id = profile.clone();
    let enqueue = tokio::spawn(async move {
        crate::payment_attempts::reconciliation::jobs::enqueue(&p, time.as_ref(), &profile_id).await
    });
    wait_for_blocker(&pool, pid, "INSERT INTO payment_reconciliation_jobs%").await;
    start.commit().await.expect("commit start");
    enqueue
        .await
        .expect("enqueue task")
        .expect("other candidate survives");
    assert!(
        crate::payment_attempts::reconciliation::read_job(&pool, &first_id)
            .await
            .expect("first read")
            .is_none()
    );
    assert!(
        crate::payment_attempts::reconciliation::read_job(&pool, &second_id)
            .await
            .expect("second read")
            .is_some()
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM payment_reconciliation_jobs")
        .fetch_one(&pool)
        .await
        .expect("job count");
    assert_eq!(count, 1);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn continuation_native_start_rejects_missing_and_damaged_source_then_recovers(pool: PgPool) {
    let (clock, frozen) = synthetic_source(&pool).await;
    let id = &frozen.snapshot().binding.id;
    let binding = continuation::acquire(&pool, clock.as_ref(), &observation_cases::ids(), id)
        .await
        .expect("binding");
    // Faults exist only inside rolled-back test transactions. These INSERTs go
    // straight to PostgreSQL and do not invoke the Rust source preflight.
    let mut missing = pool.begin().await.expect("missing transaction");
    sqlx::query("ALTER TABLE payment_test_continuations DISABLE TRIGGER USER")
        .execute(&mut *missing)
        .await
        .expect("fault setup");
    assert_eq!(
        sqlx::query("DELETE FROM payment_test_continuations WHERE attempt_id=$1")
            .bind(id)
            .execute(&mut *missing)
            .await
            .expect("remove mapping")
            .rows_affected(),
        1
    );
    let failure = sqlx::query(
        "INSERT INTO callback_lookup_fetch_starts(operation_id,started_at) VALUES($1,1500)",
    )
    .bind(&binding.operation_id)
    .execute(&mut *missing)
    .await
    .expect_err("native fence rejects source");
    let error = failure.as_database_error().expect("database rejection");
    assert_eq!(error.code().as_deref(), Some("23514"));
    assert_eq!(error.message(), "Continuation arbitration unavailable");
    missing.rollback().await.expect("restore exact mapping");
    let mut damaged = pool.begin().await.expect("damaged transaction");
    sqlx::query("ALTER TABLE payment_initial_dispatch_provenance DISABLE TRIGGER USER")
        .execute(&mut *damaged)
        .await
        .expect("fault setup");
    assert_eq!(sqlx::query("UPDATE payment_initial_dispatch_provenance SET observation_sha256=decode(repeat('00',32),'hex') WHERE attempt_id=$1 AND observation_sha256<>decode(repeat('00',32),'hex')").bind(id).execute(&mut *damaged).await.expect("damage digest").rows_affected(), 1);
    let failure = sqlx::query(
        "INSERT INTO callback_lookup_fetch_starts(operation_id,started_at) VALUES($1,1500)",
    )
    .bind(&binding.operation_id)
    .execute(&mut *damaged)
    .await
    .expect_err("native fence rejects source");
    let error = failure.as_database_error().expect("database rejection");
    assert_eq!(error.code().as_deref(), Some("23514"));
    assert_eq!(error.message(), "Continuation arbitration unavailable");
    damaged.rollback().await.expect("restore exact provenance");
    let exact: bool = sqlx::query_scalar("SELECT payment_test_continuation_exact(c) FROM payment_test_continuations c WHERE c.attempt_id=$1").bind(id).fetch_one(&pool).await.expect("repaired custody");
    assert!(exact);
    assert_eq!(
        sqlx::query(
            "INSERT INTO callback_lookup_fetch_starts(operation_id,started_at) VALUES($1,1500)"
        )
        .bind(&binding.operation_id)
        .execute(&pool)
        .await
        .expect("exact repaired native start")
        .rows_affected(),
        1
    );
    assert_eq!(counts(&pool).await, (1, 1, 1, 0, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn supervisor_enqueue_rejects_damaged_fence_and_exact_repair_skips(pool: PgPool) {
    let (clock, frozen) = synthetic_source(&pool).await;
    let id = &frozen.snapshot().binding.id;
    continuation::acquire(&pool, clock.as_ref(), &observation_cases::ids(), id)
        .await
        .expect("binding");
    let _start = continuation::start(&pool, clock.as_ref(), id)
        .await
        .expect("start");
    let original: Vec<u8> = sqlx::query_scalar(
        "SELECT observation_sha256 FROM payment_initial_dispatch_provenance WHERE attempt_id=$1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .expect("original digest");
    pool.execute("ALTER TABLE payment_initial_dispatch_provenance DISABLE TRIGGER USER")
        .await
        .expect("fault setup");
    assert_eq!(sqlx::query("UPDATE payment_initial_dispatch_provenance SET observation_sha256=set_byte(observation_sha256,0,get_byte(observation_sha256,0) # 1) WHERE attempt_id=$1").bind(id).execute(&pool).await.expect("damage").rows_affected(), 1);
    pool.execute("ALTER TABLE payment_initial_dispatch_provenance ENABLE TRIGGER USER")
        .await
        .expect("restore guard");
    let mut direct = pool.begin().await.expect("native job transaction");
    let failure = insert_supervisor(&mut direct, id)
        .await
        .expect_err("invalid fence rejected");
    let error = failure.as_database_error().expect("database rejection");
    assert_eq!(error.code().as_deref(), Some("23514"));
    assert_eq!(error.message(), "Invalid continuation arbitration custody");
    direct.rollback().await.expect("rollback");
    let profile = &frozen.snapshot().binding.profile.id;
    assert!(
        crate::payment_attempts::reconciliation::jobs::enqueue(&pool, clock.as_ref(), profile)
            .await
            .is_err()
    );
    assert!(
        crate::payment_attempts::reconciliation::read_job(&pool, id)
            .await
            .expect("read")
            .is_none()
    );
    pool.execute("ALTER TABLE payment_initial_dispatch_provenance DISABLE TRIGGER USER")
        .await
        .expect("repair setup");
    assert_eq!(sqlx::query("UPDATE payment_initial_dispatch_provenance SET observation_sha256=$2 WHERE attempt_id=$1").bind(id).bind(&original).execute(&pool).await.expect("exact repair").rows_affected(), 1);
    pool.execute("ALTER TABLE payment_initial_dispatch_provenance ENABLE TRIGGER USER")
        .await
        .expect("restore guard");
    let restored: Vec<u8> = sqlx::query_scalar(
        "SELECT observation_sha256 FROM payment_initial_dispatch_provenance WHERE attempt_id=$1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .expect("restored digest");
    assert_eq!(restored, original);
    crate::payment_attempts::reconciliation::jobs::enqueue(&pool, clock.as_ref(), profile)
        .await
        .expect("valid fenced candidate skipped");
    assert!(
        crate::payment_attempts::reconciliation::read_job(&pool, id)
            .await
            .expect("read")
            .is_none()
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn cleared_marker_with_retained_mapping_cannot_use_generic_or_native_start(pool: PgPool) {
    let (clock, frozen) = synthetic_source(&pool).await;
    let id = &frozen.snapshot().binding.id;
    let binding = continuation::acquire(&pool, clock.as_ref(), &observation_cases::ids(), id)
        .await
        .expect("binding");
    let operation = lookup::read_operation(&pool, &binding.operation_id)
        .await
        .expect("operation");
    pool.execute("ALTER TABLE callback_lookup_operations DISABLE TRIGGER USER")
        .await
        .expect("fault setup");
    assert_eq!(sqlx::query("UPDATE callback_lookup_operations SET continuation_attempt_id=NULL WHERE id=$1 AND continuation_attempt_id=$2").bind(&binding.operation_id).bind(id).execute(&pool).await.expect("damage marker").rows_affected(), 1);
    pool.execute("ALTER TABLE callback_lookup_operations ENABLE TRIGGER USER")
        .await
        .expect("restore guard");
    assert!(matches!(
        continuation::read(&pool, id).await,
        Err(PaymentStoreError::InvalidStoredData)
    ));
    assert!(matches!(
        continuation::acquire(&pool, &BadPorts, &BadPorts, id).await,
        Err(PaymentStoreError::InvalidStoredData)
    ));
    assert!(matches!(
        continuation::start(&pool, clock.as_ref(), id).await,
        Err(PaymentStoreError::InvalidStoredData)
    ));
    assert!(matches!(
        lookup::start_lookup(&pool, clock.as_ref(), &operation).await,
        Err(PaymentStoreError::Unavailable)
    ));
    assert!(matches!(
        lookup::read_lookup(&pool, &binding.operation_id).await,
        Err(PaymentStoreError::Unavailable)
    ));
    let failure = sqlx::query(
        "INSERT INTO callback_lookup_fetch_starts(operation_id,started_at) VALUES($1,1500)",
    )
    .bind(&binding.operation_id)
    .execute(&pool)
    .await
    .expect_err("native fence rejects missing marker");
    let error = failure.as_database_error().expect("database rejection");
    assert_eq!(error.code().as_deref(), Some("23514"));
    assert_eq!(error.message(), "Continuation arbitration unavailable");
    assert_eq!(counts(&pool).await, (1, 1, 0, 0, 1));
    pool.execute("ALTER TABLE callback_lookup_operations DISABLE TRIGGER USER")
        .await
        .expect("repair setup");
    assert_eq!(sqlx::query("UPDATE callback_lookup_operations SET continuation_attempt_id=$2 WHERE id=$1 AND continuation_attempt_id IS NULL").bind(&binding.operation_id).bind(id).execute(&pool).await.expect("exact repair").rows_affected(), 1);
    pool.execute("ALTER TABLE callback_lookup_operations ENABLE TRIGGER USER")
        .await
        .expect("restore guard");
    let repaired = continuation::read(&pool, id)
        .await
        .expect("repaired read")
        .expect("original binding");
    assert_eq!(repaired.operation_id, binding.operation_id);
    assert!(
        continuation::acquire(&pool, &BadPorts, &BadPorts, id)
            .await
            .expect("original replay")
            .replayed
    );
    let start = continuation::start(&pool, clock.as_ref(), id)
        .await
        .expect("repaired start");
    assert_eq!(start.operation_id(), binding.operation_id);
    assert_eq!(counts(&pool).await, (1, 1, 1, 0, 1));
    // Even with a durable start, a missing marker must reach the native guard
    // through the retained mapping rather than vanish in the enqueue prefilter.
    pool.execute("ALTER TABLE callback_lookup_operations DISABLE TRIGGER USER")
        .await
        .expect("fault setup");
    assert_eq!(sqlx::query("UPDATE callback_lookup_operations SET continuation_attempt_id=NULL WHERE id=$1 AND continuation_attempt_id=$2").bind(&binding.operation_id).bind(id).execute(&pool).await.expect("damage started marker").rows_affected(), 1);
    pool.execute("ALTER TABLE callback_lookup_operations ENABLE TRIGGER USER")
        .await
        .expect("restore guard");
    assert!(
        crate::payment_attempts::reconciliation::jobs::enqueue(
            &pool,
            clock.as_ref(),
            &frozen.snapshot().binding.profile.id
        )
        .await
        .is_err()
    );
    pool.execute("ALTER TABLE callback_lookup_operations DISABLE TRIGGER USER")
        .await
        .expect("repair setup");
    assert_eq!(sqlx::query("UPDATE callback_lookup_operations SET continuation_attempt_id=$2 WHERE id=$1 AND continuation_attempt_id IS NULL").bind(&binding.operation_id).bind(id).execute(&pool).await.expect("restore marker").rows_affected(), 1);
    pool.execute("ALTER TABLE callback_lookup_operations ENABLE TRIGGER USER")
        .await
        .expect("restore guard");
    crate::payment_attempts::reconciliation::jobs::enqueue(
        &pool,
        clock.as_ref(),
        &frozen.snapshot().binding.profile.id,
    )
    .await
    .expect("valid fence skipped");
    assert!(
        crate::payment_attempts::reconciliation::read_job(&pool, id)
            .await
            .expect("read")
            .is_none()
    );
}

#[path = "test_continuation_receipt.rs"]
mod receipt_cases;
