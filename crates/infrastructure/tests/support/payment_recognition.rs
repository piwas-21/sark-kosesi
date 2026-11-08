//! Independent real PostgreSQL oracles for private gross-payment recognition.
use super::observation_cases::{BadPorts, fetched, ids, prepared};
use super::*;
use crate::{
    MIGRATOR,
    payment_attempts::{
        PaymentStoreError, identities::resolve, observations::record_verified, recognition::*,
    },
};
use commerce_domain::payment_attempts::recognition::State;
use sqlx::{Executor, PgPool};
use std::sync::{Arc, atomic::Ordering};
async fn preserved(pool: &PgPool) -> Value {
    // Old rows lack the new promotion marker. A SQL null adds no custody.
    sqlx::query_scalar("SELECT jsonb_build_object('orders',(SELECT jsonb_agg(CASE WHEN to_jsonb(t)->'promotion_redemption_transaction_id'='null'::jsonb THEN to_jsonb(t)-'promotion_redemption_transaction_id' ELSE to_jsonb(t) END ORDER BY id) FROM checkout_orders t),'attempts',(SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM payment_attempts t),'balances',(SELECT jsonb_agg(to_jsonb(t) ORDER BY component_code) FROM stock_balances t),'holds',(SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM stock_holds t),'movements',(SELECT jsonb_agg(to_jsonb(t) ORDER BY sequence) FROM stock_movements t))").fetch_one(pool).await.expect("immutable money and physical custody")
}
async fn recognition_counts(pool: &PgPool) -> (i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM payment_recognitions),(SELECT count(*) FROM payment_recognition_current),(SELECT count(*) FROM commerce_outbox WHERE kind='payment.recognized.v1')").fetch_one(pool).await.expect("recognition atomic counts")
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn recognition_zero_open_paid_reordered_and_late_paid_are_literal_gross_facts(pool: PgPool) {
    let (time, f) = prepared(&pool, true).await;
    let a = &f.snapshot().binding.id;
    let i = ids();
    let original = preserved(&pool).await;
    assert!(current(&pool, a).await.expect("no pointer").is_none());
    assert_eq!(
        recognize(&pool, time.as_ref(), &i, a).await.err(),
        Some(PaymentStoreError::Unavailable)
    );
    resolve(&pool, time.as_ref(), &i, a, "zero")
        .await
        .expect("zero resolution");
    let zero = recognize(&pool, time.as_ref(), &i, a)
        .await
        .expect("zero prefix")
        .recognition;
    assert_eq!(
        (zero.state, zero.amount_minor, zero.through_observation),
        (State::Unconfirmed, 38245, 0)
    );
    for (effect, status, state, watermark) in [
        ("open", "open", State::Unconfirmed, 1),
        ("paid", "paid", State::Received, 2),
        ("old-open", "open", State::Received, 3),
        ("late-paid", "paid", State::Received, 4),
    ] {
        time.0.store(
            if effect == "late-paid" { 5000 } else { 1500 },
            Ordering::SeqCst,
        );
        record_verified(
            &pool,
            time.as_ref(),
            &i,
            a,
            effect,
            &fetched(f.snapshot(), "tr_A", status, false).await,
        )
        .await
        .expect("fetched evidence");
        assert_eq!(
            current(&pool, a).await.err(),
            Some(PaymentStoreError::Unavailable),
            "new evidence makes old pointer stale"
        );
        resolve(&pool, time.as_ref(), &i, a, effect)
            .await
            .expect("complete resolution");
        let receipt = recognize(&pool, time.as_ref(), &i, a)
            .await
            .expect("recognition");
        assert!(!receipt.replayed);
        assert_eq!(
            (
                receipt.recognition.state,
                receipt.recognition.amount_minor,
                receipt.recognition.through_observation
            ),
            (state, 38245, watermark)
        );
        assert!(current(&pool, a).await.expect("fresh pointer") == Some(receipt.recognition));
    }
    assert!(read_recorded(&pool, a, 0).await.expect("historical zero") == Some(zero));
    assert_eq!(recognition_counts(&pool).await, (5, 1, 5));
    assert_eq!(preserved(&pool).await, original);
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn recognition_canonical_concurrent_replay_retains_original_resolution_without_ports(
    pool: PgPool,
) {
    let (time, f) = prepared(&pool, true).await;
    let a = f.snapshot().binding.id.clone();
    let i = Arc::new(ids());
    record_verified(
        &pool,
        time.as_ref(),
        i.as_ref(),
        &a,
        "paid",
        &fetched(f.snapshot(), "tr_A", "paid", false).await,
    )
    .await
    .expect("paid");
    let resolution = resolve(&pool, time.as_ref(), i.as_ref(), &a, "first-resolution")
        .await
        .expect("resolution")
        .resolution;
    let mut tasks = vec![];
    for _ in 0..4 {
        let p = pool.clone();
        let t = time.clone();
        let i = i.clone();
        let a = a.clone();
        tasks.push(tokio::spawn(async move {
            recognize(&p, t.as_ref(), i.as_ref(), &a).await
        }));
    }
    let mut canonical = None;
    let mut replays = 0;
    for task in tasks {
        let receipt = task.await.expect("task").expect("serialized");
        replays += i32::from(receipt.replayed);
        assert_eq!(receipt.recognition.resolution_id, resolution.id);
        if let Some(ref old) = canonical {
            assert!(old == &receipt.recognition);
        }
        canonical = Some(receipt.recognition);
    }
    assert_eq!(replays, 3);
    assert_eq!(recognition_counts(&pool).await, (1, 1, 1));
    resolve(
        &pool,
        time.as_ref(),
        i.as_ref(),
        &a,
        "same-prefix-different-resolution",
    )
    .await
    .expect("duplicate resolution");
    let replay = recognize(&pool, &BadPorts, &BadPorts, &a)
        .await
        .expect("canonical receipt with no time or entropy");
    assert!(replay.replayed);
    assert!(Some(replay.recognition) == canonical);
    assert!(
        read_recorded(&pool, &a, 1)
            .await
            .expect("original exact resolution survives")
            == canonical
    );
    record_verified(
        &pool,
        time.as_ref(),
        i.as_ref(),
        &a,
        "other",
        &fetched(f.snapshot(), "tr_B", "open", true).await,
    )
    .await
    .expect("second identity");
    resolve(&pool, time.as_ref(), i.as_ref(), &a, "ambiguous")
        .await
        .expect("ambiguous resolution");
    let review = recognize(&pool, time.as_ref(), i.as_ref(), &a)
        .await
        .expect("review")
        .recognition;
    assert_eq!(review.state, State::NeedsReview);
    assert_eq!(review.amount_minor, 38245);
    assert!(
        read_recorded(&pool, &a, 1)
            .await
            .expect("paid historical receipt")
            == canonical
    );
    assert_eq!(recognition_counts(&pool).await, (2, 1, 2));
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn recognition_and_current_refresh_after_actual_attempt_wait_with_repeatable_read_pool_default(
    pool: PgPool,
) {
    let (time, f) = prepared(&pool, true).await;
    let a = f.snapshot().binding.id.clone();
    let i = ids();
    resolve(&pool, time.as_ref(), &i, &a, "zero")
        .await
        .expect("zero resolution");
    recognize(&pool, time.as_ref(), &i, &a)
        .await
        .expect("old unconfirmed pointer");
    let rr = sqlx::postgres::PgPoolOptions::new()
        .max_connections(6)
        .after_connect(|c, _| {
            Box::pin(async move {
                c.execute("SET default_transaction_isolation='repeatable read'")
                    .await?;
                Ok(())
            })
        })
        .connect_with(pool.connect_options().as_ref().clone())
        .await
        .expect("owned RR-default pool");
    let isolation: String = sqlx::query_scalar("SHOW default_transaction_isolation")
        .fetch_one(&rr)
        .await
        .expect("instrument positive control");
    assert_eq!(isolation, "repeatable read");
    let verified = fetched(f.snapshot(), "tr_A", "paid", false).await;
    let snapshot = commerce_domain::payment_attempts::observations::ObservationSnapshot {
        schema_version: 1,
        id: "a".repeat(32),
        attempt_id: a.clone(),
        effect_key: "racing-paid".into(),
        revision: 1,
        recorded_at: 1500,
        intent: commerce_domain::payment_attempts::observations::ObservationIntent {
            payment: verified.payment().clone(),
            credential_fingerprint: verified.credential_fingerprint().into(),
        },
    };
    let mut writer = pool.begin().await.expect("writer");
    sqlx::query("SELECT id FROM payment_attempts WHERE id=$1 FOR UPDATE")
        .bind(&a)
        .execute(&mut *writer)
        .await
        .expect("separate attempt lock");
    sqlx::query("INSERT INTO payment_observations(id,attempt_id,effect_key,revision,recorded_at,provider_id,credential_fingerprint,status,snapshot) VALUES($1,$2,'racing-paid',1,1500,'tr_A',$3,'paid',$4)").bind(&snapshot.id).bind(&a).bind(&snapshot.intent.credential_fingerprint).bind(serde_json::to_value(&snapshot).expect("snapshot JSON")).execute(&mut *writer).await.expect("native observation");
    sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) VALUES($1,$2,'payment.observation_recorded.v1',$3,1500)").bind("b".repeat(32)).bind(&f.snapshot().binding.order_id).bind(json!({"schema_version":1,"order_id":f.snapshot().binding.order_id,"attempt_id":a,"observation_id":snapshot.id})).execute(&mut *writer).await.expect("atomic source event");
    let p = rr.clone();
    let id = a.clone();
    let reader = tokio::spawn(async move { current(&p, &id).await });
    let p = rr.clone();
    let id = a.clone();
    let t = time.clone();
    let writer_recognition =
        tokio::spawn(async move { recognize(&p, t.as_ref(), &ids(), &id).await });
    tokio::time::timeout(Duration::from_millis(800),async{loop{let n:i64=sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE '%payment_attempts%FOR UPDATE%'").fetch_one(&pool).await.expect("actual attempt wait");if n>=2{break;}tokio::time::sleep(Duration::from_millis(5)).await;}}).await.expect("both paths actually blocked");
    writer.commit().await.expect("paid append commit");
    assert_eq!(
        reader.await.expect("reader task").err(),
        Some(PaymentStoreError::Unavailable),
        "old unconfirmed cannot escape after append"
    );
    assert_eq!(
        writer_recognition.await.expect("writer task").err(),
        Some(PaymentStoreError::Unavailable),
        "old resolution cannot recognize newer prefix"
    );
    resolve(&rr, time.as_ref(), &i, &a, "paid")
        .await
        .expect("RC resolution");
    let fresh = recognize(&rr, time.as_ref(), &i, &a)
        .await
        .expect("RC recognition")
        .recognition;
    assert_eq!(
        (fresh.state, fresh.through_observation),
        (State::Received, 1)
    );
    assert!(current(&rr, &a).await.expect("RC current") == Some(fresh));
    rr.close().await;
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn recognition_final_clock_after_actual_outbox_wait_and_cancellation_roll_back(pool: PgPool) {
    let (time, f) = prepared(&pool, true).await;
    let a = f.snapshot().binding.id.clone();
    let i = ids();
    resolve(&pool, time.as_ref(), &i, &a, "ready")
        .await
        .expect("resolution");
    let original = preserved(&pool).await;
    for cancel in [false, true] {
        let mut blocker = pool.begin().await.expect("blocker");
        blocker
            .execute("LOCK TABLE commerce_outbox IN SHARE MODE")
            .await
            .expect("real event wait");
        let p = pool.clone();
        let t = time.clone();
        let a = a.clone();
        let mut task = Some(tokio::spawn(async move {
            let fresh_ids =
                crate::payment_test_fixture::Ids(std::sync::atomic::AtomicU64::new(50_000));
            recognize(&p, t.as_ref(), &fresh_ids, &a).await
        }));
        crate::payment_test_fixture::waiting(&pool).await;
        assert_eq!(recognition_counts(&pool).await, (0, 0, 0));
        if cancel {
            let cancelled = task.take().expect("running task");
            cancelled.abort();
            assert!(cancelled.await.is_err_and(|e| e.is_cancelled()));
        } else {
            time.0.store(1499, Ordering::SeqCst);
        }
        blocker.commit().await.expect("release actual wait");
        if !cancel {
            assert_eq!(
                task.take()
                    .expect("running clock task")
                    .await
                    .expect("clock task")
                    .err(),
                Some(PaymentStoreError::ClockUnavailable)
            );
        }
        time.0.store(1500, Ordering::SeqCst);
        // Acquiring the same attempt proves cancellation released/rolled back its transaction.
        let mut check = pool.begin().await.expect("rollback proof");
        sqlx::query("SELECT id FROM payment_attempts WHERE id=$1 FOR UPDATE")
            .bind(&f.snapshot().binding.id)
            .execute(&mut *check)
            .await
            .expect("lock recovered");
        check.rollback().await.expect("rollback");
        assert_eq!(recognition_counts(&pool).await, (0, 0, 0));
        assert_eq!(preserved(&pool).await, original);
    }
    assert!(
        !recognize(&pool, time.as_ref(), &i, &a)
            .await
            .expect("healthy retry")
            .replayed
    );
    assert_eq!(recognition_counts(&pool).await, (1, 1, 1));
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn recognition_request_clock_entropy_missing_attempt_and_database_fail_closed(pool: PgPool) {
    let (time, f) = prepared(&pool, false).await;
    let a = &f.snapshot().binding.id;
    let i = ids();
    for id in ["bad", ""] {
        assert_eq!(
            recognize(&pool, time.as_ref(), &i, id).await.err(),
            Some(PaymentStoreError::InvalidRequest)
        );
        assert_eq!(
            current(&pool, id).await.err(),
            Some(PaymentStoreError::InvalidRequest)
        );
        assert_eq!(
            read_recorded(&pool, id, 0).await.err(),
            Some(PaymentStoreError::InvalidRequest)
        );
    }
    for n in [-1, 1001, i64::MAX] {
        assert_eq!(
            read_recorded(&pool, a, n).await.err(),
            Some(PaymentStoreError::InvalidRequest)
        );
    }
    assert_eq!(
        current(&pool, &"f".repeat(32)).await.err(),
        Some(PaymentStoreError::NotFound)
    );
    assert_eq!(
        recognize(&pool, time.as_ref(), &i, a).await.err(),
        Some(PaymentStoreError::Unavailable)
    );
    start_dispatch(&pool, time.as_ref(), &i, a, &config(&credential()))
        .await
        .expect("dispatch");
    resolve(&pool, time.as_ref(), &i, a, "zero")
        .await
        .expect("resolve");
    assert_eq!(
        recognize(&pool, &BadPorts, &i, a).await.err(),
        Some(PaymentStoreError::ClockUnavailable)
    );
    assert!(recognize(&pool, time.as_ref(), &BadPorts, a).await.is_err());
    for n in [1499, 9_007_199_254_740_992, u64::MAX] {
        time.0.store(n, Ordering::SeqCst);
        assert_eq!(
            recognize(&pool, time.as_ref(), &i, a).await.err(),
            Some(PaymentStoreError::ClockUnavailable)
        );
    }
    assert_eq!(recognition_counts(&pool).await, (0, 0, 0));
    time.0.store(1500, Ordering::SeqCst);
    assert!(
        read_recorded(&pool, a, 0)
            .await
            .expect("absent historical")
            .is_none()
    );
    pool.close().await;
    assert_eq!(
        recognize(&pool, time.as_ref(), &i, a).await.err(),
        Some(PaymentStoreError::Database)
    );
    assert_eq!(
        current(&pool, a).await.err(),
        Some(PaymentStoreError::Database)
    );
    assert_eq!(
        read_recorded(&pool, a, 0).await.err(),
        Some(PaymentStoreError::Database)
    );
}

#[path = "payment_recognition_corruption.rs"]
mod corruption;
#[path = "payment_recognition_native.rs"]
mod native;

#[sqlx::test(migrations = false)]
async fn recognition_schema30_upgrade_preserves_complete_original_paid_custody_and_seeds_no_authority(
    pool: PgPool,
) {
    MIGRATOR
        .run_to(30, &pool)
        .await
        .expect("exact prior schema30");
    let (time, f) = prepared(&pool, true).await;
    let a = &f.snapshot().binding.id;
    let i = ids();
    let observation = record_verified(
        &pool,
        time.as_ref(),
        &i,
        a,
        "prior-paid",
        &fetched(f.snapshot(), "tr_PRIOR", "paid", false).await,
    )
    .await
    .expect("prior authenticated paid receipt")
    .snapshot;
    let resolution = resolve(&pool, time.as_ref(), &i, a, "prior-resolution")
        .await
        .expect("prior exact prefix and ownership")
        .resolution;
    let before = preserved(&pool).await;
    let source_sql = "SELECT jsonb_build_object('observations',(SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM payment_observations t),'resolutions',(SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM payment_identity_resolutions t),'claims',(SELECT jsonb_agg(to_jsonb(t) ORDER BY profile_id,provider_id) FROM payment_provider_identity_claims t),'events',(SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM commerce_outbox t),'authority',(SELECT jsonb_agg(to_jsonb(t) ORDER BY staff_id,capability) FROM staff_capabilities t))";
    let sources: Value = sqlx::query_scalar(source_sql)
        .fetch_one(&pool)
        .await
        .expect("all prior source custody");
    MIGRATOR.run(&pool).await.expect("31 upgrade");
    MIGRATOR.run(&pool).await.expect("idempotent upgrade");
    assert_eq!(preserved(&pool).await, before);
    assert_eq!(
        sqlx::query_scalar::<_, Value>(source_sql)
            .fetch_one(&pool)
            .await
            .expect("unchanged source rows"),
        sources
    );
    assert_eq!(recognition_counts(&pool).await, (0, 0, 0));
    assert!(
        current(&pool, a)
            .await
            .expect("no implicit decision")
            .is_none()
    );
    assert!(
        crate::payment_attempts::observations::read_recorded(&pool, a, "prior-paid")
            .await
            .expect("prior observation")
            == Some(observation)
    );
    assert!(
        crate::payment_attempts::identities::read_recorded(&pool, a, "prior-resolution")
            .await
            .expect("prior resolution")
            == Some(resolution.clone())
    );
    let receipt = recognize(&pool, time.as_ref(), &i, a)
        .await
        .expect("explicit recognition only after upgrade")
        .recognition;
    assert_eq!(
        (
            receipt.state,
            receipt.amount_minor,
            receipt.through_observation
        ),
        (State::Received, 38245, 1)
    );
    assert_eq!(receipt.resolution_id, resolution.id);
    assert_eq!(receipt.order_id, f.snapshot().binding.order_id);
    assert_eq!(preserved(&pool).await, before);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn recognition_never_rewinds_prior_recognition_time_when_newer_fact_has_an_older_clock(
    pool: PgPool,
) {
    let (time, f) = prepared(&pool, true).await;
    let a = &f.snapshot().binding.id;
    let i = ids();
    resolve(&pool, time.as_ref(), &i, a, "zero")
        .await
        .expect("prefix zero resolution at1500");
    time.0.store(1600, Ordering::SeqCst);
    let zero = recognize(&pool, time.as_ref(), &i, a)
        .await
        .expect("first recognition at1600")
        .recognition;
    assert_eq!((zero.state, zero.recorded_at), (State::Unconfirmed, 1600));
    // Observation and resolution journals have their own monotonic source clock.
    time.0.store(1500, Ordering::SeqCst);
    record_verified(
        &pool,
        time.as_ref(),
        &i,
        a,
        "paid",
        &fetched(f.snapshot(), "tr_A", "paid", false).await,
    )
    .await
    .expect("valid new fact at1500");
    let resolution = resolve(&pool, time.as_ref(), &i, a, "paid")
        .await
        .expect("valid new resolution at1500")
        .resolution;
    assert_eq!(
        (resolution.through_observation, resolution.recorded_at),
        (1, 1500)
    );
    assert_eq!(
        recognize(&pool, time.as_ref(), &i, a).await.err(),
        Some(PaymentStoreError::ClockUnavailable),
        "recognition clock may not rewind past its prior journal"
    );
    assert_eq!(recognition_counts(&pool).await, (1, 1, 1));
    assert!(
        read_recorded(&pool, a, 0)
            .await
            .expect("original receipt survives")
            == Some(zero)
    );
    assert_eq!(
        current(&pool, a).await.err(),
        Some(PaymentStoreError::Unavailable)
    );
    time.0.store(1600, Ordering::SeqCst);
    let paid = recognize(&pool, time.as_ref(), &i, a)
        .await
        .expect("nondecreasing healthy retry")
        .recognition;
    assert_eq!(
        (paid.state, paid.recorded_at, paid.through_observation),
        (State::Received, 1600, 1)
    );
    assert_eq!(recognition_counts(&pool).await, (2, 1, 2));
}
