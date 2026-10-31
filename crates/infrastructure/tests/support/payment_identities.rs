//! Real disposable PostgreSQL; authenticated synthetic GETs never actual payments.
use super::observation_cases::{BadPorts, fetched, ids, prepared};
use super::*;
use crate::MIGRATOR;
use crate::payment_attempts::{PaymentStoreError, identities::*, observations::record_verified};
use sqlx::{Executor, PgPool};
use std::sync::atomic::Ordering;
#[sqlx::test(migrator = "MIGRATOR")]
async fn identity_receipts_bind_full_prefix_and_preserve_ambiguity_and_paid_on_reordering(
    pool: PgPool,
) {
    let (time, f) = prepared(&pool, true).await;
    let a = &f.snapshot().binding.id;
    let i = ids();
    let empty = resolve(&pool, time.as_ref(), &i, a, "empty")
        .await
        .expect("empty");
    assert_eq!(empty.resolution.through_observation, 0);
    assert!(empty.resolution.summary.provider_ids.is_empty());
    assert!(!empty.replayed);
    for (effect, id, status) in [
        ("pending", "tr_B", "pending"),
        ("paid", "tr_B", "paid"),
        ("old", "tr_B", "open"),
    ] {
        record_verified(
            &pool,
            time.as_ref(),
            &i,
            a,
            effect,
            &fetched(f.snapshot(), id, status, false).await,
        )
        .await
        .expect("verified evidence");
    }
    let one = resolve(&pool, time.as_ref(), &i, a, "one")
        .await
        .expect("resolve")
        .resolution;
    assert_eq!(one.through_observation, 3);
    assert_eq!(one.summary.provider_ids, vec!["tr_B"]);
    assert_eq!(one.summary.paid_provider_ids, vec!["tr_B"]);
    record_verified(
        &pool,
        time.as_ref(),
        &i,
        a,
        "other",
        &fetched(f.snapshot(), "tr_A", "paid", true).await,
    )
    .await
    .expect("rotated late evidence");
    time.0.store(5000, Ordering::SeqCst);
    let many = resolve(&pool, time.as_ref(), &i, a, "many")
        .await
        .expect("ambiguity")
        .resolution;
    assert_eq!(many.summary.provider_ids, vec!["tr_A", "tr_B"]);
    assert_eq!(many.summary.paid_provider_ids, vec!["tr_A", "tr_B"]);
    let old = resolve(&pool, &BadPorts, &BadPorts, a, "one")
        .await
        .expect("historical exact receipt");
    assert!(old.replayed);
    assert!(old.resolution == one);
    assert!(
        read_recorded(&pool, a, "empty")
            .await
            .expect("read")
            .expect("receipt")
            .summary
            .provider_ids
            .is_empty()
    );
    assert!(latest(&pool, a).await.expect("latest") == Some(many));
    assert!(
        read_recorded(&pool, a, "missing")
            .await
            .expect("missing")
            .is_none()
    );
    assert_eq!(
        crate::payment_test_fixture::counts(&pool).await,
        (1, 10, 1, 1, 2)
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn concurrent_resolution_serializes_receipts_and_current_observation_watermark(pool: PgPool) {
    let (time, f) = prepared(&pool, true).await;
    let a = f.snapshot().binding.id.clone();
    let mut tasks = vec![];
    let i = std::sync::Arc::new(ids());
    for effect in ["same", "same", "distinct"] {
        let p = pool.clone();
        let t = time.clone();
        let i = i.clone();
        let a = a.clone();
        tasks.push(tokio::spawn(async move {
            resolve(&p, t.as_ref(), i.as_ref(), &a, effect).await
        }));
    }
    let mut replays = 0;
    for task in tasks {
        if task.await.expect("task").expect("serialized").replayed {
            replays += 1;
        }
    }
    assert_eq!(replays, 1);
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM payment_identity_resolutions")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(n, 2);
    assert_eq!(
        latest(&pool, &a)
            .await
            .expect("latest")
            .expect("found")
            .revision,
        2
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn resolution_rejects_invalid_inputs_unstarted_clock_entropy_and_database_failures(
    pool: PgPool,
) {
    let (time, f) = prepared(&pool, false).await;
    let a = &f.snapshot().binding.id;
    for (id, effect) in [("bad", "a"), (a, "bad effect")] {
        assert_eq!(
            resolve(&pool, time.as_ref(), &ids(), id, effect)
                .await
                .err(),
            Some(PaymentStoreError::InvalidRequest)
        );
        assert_eq!(
            read_recorded(&pool, id, effect).await.err(),
            Some(PaymentStoreError::InvalidRequest)
        );
    }
    assert_eq!(
        latest(&pool, "bad").await.err(),
        Some(PaymentStoreError::InvalidRequest)
    );
    assert_eq!(
        resolve(&pool, time.as_ref(), &ids(), a, "a").await.err(),
        Some(PaymentStoreError::Unavailable)
    );
    assert_eq!(
        latest(&pool, &"f".repeat(32)).await.err(),
        Some(PaymentStoreError::NotFound)
    );
    let c = config(&credential());
    let i = ids();
    start_dispatch(&pool, time.as_ref(), &i, a, &c)
        .await
        .expect("start");
    assert_eq!(
        resolve(&pool, &BadPorts, &ids(), a, "bad-clock")
            .await
            .err(),
        Some(PaymentStoreError::ClockUnavailable)
    );
    assert!(
        resolve(&pool, time.as_ref(), &BadPorts, a, "bad-entropy")
            .await
            .is_err()
    );
    for t in [1499, 9_007_199_254_740_992, u64::MAX] {
        time.0.store(t, Ordering::SeqCst);
        assert_eq!(
            resolve(&pool, time.as_ref(), &ids(), a, "bad-time")
                .await
                .err(),
            Some(PaymentStoreError::ClockUnavailable)
        );
    }
    time.0.store(1600, Ordering::SeqCst);
    resolve(&pool, time.as_ref(), &i, a, "later")
        .await
        .expect("new");
    time.0.store(1599, Ordering::SeqCst);
    assert_eq!(
        resolve(&pool, time.as_ref(), &ids(), a, "rewind")
            .await
            .err(),
        Some(PaymentStoreError::ClockUnavailable)
    );
    pool.close().await;
    assert_eq!(
        resolve(&pool, time.as_ref(), &ids(), a, "closed")
            .await
            .err(),
        Some(PaymentStoreError::Database)
    );
    assert_eq!(
        latest(&pool, a).await.err(),
        Some(PaymentStoreError::Database)
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn native_guard_rejects_false_summary_mutation_and_missing_atomic_event(pool: PgPool) {
    let (time, f) = prepared(&pool, true).await;
    let a = &f.snapshot().binding.id;
    for (summary, watermark) in [
        (
            json!({"provider_ids":["tr_FAKE"],"paid_provider_ids":[]}),
            0,
        ),
        (json!({"provider_ids":[],"paid_provider_ids":[]}), 1),
        (json!({"provider_ids":[],"paid_provider_ids":[]}), 0),
    ] {
        let mut tx = pool.begin().await.expect("tx");
        let r=sqlx::query("INSERT INTO payment_identity_resolutions(id,attempt_id,effect_key,revision,through_observation,recorded_at,summary) VALUES($1,$2,'raw',1,$3,1500,$4)").bind("f".repeat(32)).bind(a).bind(watermark).bind(summary).execute(&mut *tx).await;
        if watermark == 0 && r.is_ok() {
            assert!(tx.commit().await.is_err(), "orphan rejected at commit");
        } else {
            assert!(r.is_err());
            tx.rollback().await.expect("rollback");
        }
    }
    resolve(&pool, time.as_ref(), &ids(), a, "valid")
        .await
        .expect("healthy");
    for sql in [
        "UPDATE payment_identity_resolutions SET recorded_at=1600",
        "DELETE FROM payment_identity_resolutions",
        "UPDATE commerce_outbox SET created_at=1600 WHERE kind='payment.identity_resolved.v1'",
    ] {
        assert!(sqlx::query(sql).execute(&pool).await.is_err());
    }
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn damaged_resolution_or_source_never_falls_back_to_older_receipts(pool: PgPool) {
    let (time, f) = prepared(&pool, true).await;
    let a = &f.snapshot().binding.id;
    let i = ids();
    resolve(&pool, time.as_ref(), &i, a, "empty")
        .await
        .expect("first");
    record_verified(
        &pool,
        time.as_ref(),
        &i,
        a,
        "paid",
        &fetched(f.snapshot(), "tr_B", "paid", false).await,
    )
    .await
    .expect("observed");
    resolve(&pool, time.as_ref(), &i, a, "paid-resolution")
        .await
        .expect("healthy");
    for sql in [
        "UPDATE payment_identity_resolutions SET id=repeat('z',32)",
        "UPDATE payment_identity_resolutions SET effect_key='bad effect' WHERE effect_key='paid-resolution'",
        "UPDATE payment_identity_resolutions SET revision=0 WHERE effect_key='paid-resolution'",
        "UPDATE payment_identity_resolutions SET through_observation=1001 WHERE effect_key='paid-resolution'",
        "UPDATE payment_identity_resolutions SET recorded_at=1499 WHERE effect_key='paid-resolution'",
        "UPDATE payment_identity_resolutions SET through_observation=2 WHERE effect_key='paid-resolution'",
        "UPDATE payment_identity_resolutions SET summary='{}' WHERE effect_key='paid-resolution'",
        "UPDATE payment_identity_resolutions SET summary='null' WHERE effect_key='paid-resolution'",
        "UPDATE payment_identity_resolutions SET summary=jsonb_build_object('padding',repeat('x',524289)) WHERE effect_key='paid-resolution'",
        "UPDATE payment_identity_resolutions SET summary='{}' WHERE effect_key='paid-resolution'",
        "DELETE FROM commerce_outbox WHERE kind='payment.identity_resolved.v1'",
        "UPDATE payment_observations SET snapshot=jsonb_set(snapshot,'{intent,payment,amount_minor}','1')",
    ] {
        let mut tx = pool.begin().await.expect("corrupt fixture");
        tx.execute("ALTER TABLE payment_identity_resolutions DISABLE TRIGGER USER; ALTER TABLE commerce_outbox DISABLE TRIGGER USER; ALTER TABLE payment_observations DISABLE TRIGGER USER").await.expect("fixture only");
        // Drop only checks needed to exercise the runtime reader independently.
        tx.execute("ALTER TABLE payment_identity_resolutions DROP CONSTRAINT payment_identity_resolutions_id_check, DROP CONSTRAINT payment_identity_resolutions_effect_key_check, DROP CONSTRAINT payment_identity_resolutions_revision_check, DROP CONSTRAINT payment_identity_resolutions_through_observation_check, DROP CONSTRAINT payment_identity_resolutions_recorded_at_check, DROP CONSTRAINT payment_identity_resolutions_summary_check").await.expect("owned checks");
        if sql.starts_with("UPDATE payment_identity_resolutions") && !sql.contains("WHERE") {
            let damage = match sql {
                "UPDATE payment_identity_resolutions SET id=repeat('z',32)" => {
                    "UPDATE payment_identity_resolutions SET id=repeat('z',32) WHERE effect_key='paid-resolution'"
                }
                _ => sql,
            };
            sqlx::query(damage).execute(&mut *tx).await.expect("damage");
        } else {
            sqlx::query(sql).execute(&mut *tx).await.expect("damage");
        }
        let (frozen, started) = crate::payment_attempts::evidence::context(&mut tx, a, false)
            .await
            .expect("context");
        assert_eq!(
            crate::payment_attempts::identities::rows::load(
                &mut tx,
                &frozen,
                started,
                Some(if sql.contains("SET effect_key=") {
                    "bad effect"
                } else {
                    "paid-resolution"
                })
            )
            .await
            .err(),
            Some(PaymentStoreError::InvalidStoredData),
            "{sql}"
        );
        tx.rollback().await.expect("restore");
    }
    assert!(latest(&pool, a).await.is_ok());
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn resolution_final_clock_after_event_wait_rolls_back_and_healthy_retry_succeeds(
    pool: PgPool,
) {
    let (time, f) = prepared(&pool, true).await;
    let mut blocker = pool.begin().await.expect("blocker");
    blocker
        .execute("LOCK TABLE commerce_outbox IN SHARE MODE")
        .await
        .expect("event wait");
    let p = pool.clone();
    let t = time.clone();
    let a = f.snapshot().binding.id.clone();
    let task = tokio::spawn(async move { resolve(&p, t.as_ref(), &ids(), &a, "waited").await });
    crate::payment_test_fixture::waiting(&pool).await;
    time.0.store(1499, Ordering::SeqCst);
    blocker.commit().await.expect("release");
    assert_eq!(
        task.await.expect("task").err(),
        Some(PaymentStoreError::ClockUnavailable)
    );
    assert_eq!(
        crate::payment_test_fixture::counts(&pool).await,
        (1, 3, 1, 1, 2)
    );
    time.0.store(1500, Ordering::SeqCst);
    assert!(
        !resolve(
            &pool,
            time.as_ref(),
            &ids(),
            &f.snapshot().binding.id,
            "waited"
        )
        .await
        .expect("restored")
        .replayed
    );
}
