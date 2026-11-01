//! Literal admission limits with healthy positive controls, not order-based fixtures.
use super::*;

#[sqlx::test(migrator = "MIGRATOR")]
async fn intake_has_independent_atomic_custody_and_same_provider_notifications_remain_distinct(
    pool: PgPool,
) {
    let clock = time();
    let i = ids();
    let first = admit(&pool, clock.as_ref(), &i, PROFILE, "tr_REPEAT").await;
    let second = admit(&pool, clock.as_ref(), &i, PROFILE, "tr_REPEAT").await;
    assert_ne!(first, second);
    assert_eq!(intake_counts(&pool).await, (2, 2, 2));
    assert_eq!(
        counts(&pool).await,
        (0, 0, 0, 0, 0),
        "admission invents no lookup or order"
    );
    assert!(
        read_callback_job(&pool, &first)
            .await
            .expect("durable job")
            .is_some()
    );
    assert!(
        read_callback_job(&pool, &second)
            .await
            .expect("distinct notification")
            .is_some()
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM commerce_outbox")
            .fetch_one(&pool)
            .await
            .expect("no order events"),
        0
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn profile_rate_thirtieth_concurrent_slot_is_atomic_and_failed_admission_does_not_consume_custody(
    pool: PgPool,
) {
    let clock = time();
    let i = Arc::new(ids());
    for n in 0..29 {
        admit(
            &pool,
            clock.as_ref(),
            i.as_ref(),
            PROFILE,
            &format!("tr_RATE_{n}"),
        )
        .await;
    }
    let mut tasks = Vec::new();
    for n in 0..4 {
        let (p, t, i) = (pool.clone(), clock.clone(), i.clone());
        tasks.push(tokio::spawn(async move {
            admit_callback(
                &p,
                t.as_ref(),
                i.as_ref(),
                PROFILE,
                &format!("tr_CONCURRENT_{n}"),
            )
            .await
        }));
    }
    let mut results = Vec::new();
    for task in tasks {
        results.push(task.await.expect("admission task"));
    }
    assert_eq!(
        results.iter().filter(|r| r.is_ok()).count(),
        1,
        "only the thirtieth profile slot"
    );
    assert_eq!(intake_counts(&pool).await, (30, 30, 30));
    clock.0.store(1560, Ordering::SeqCst);
    admit(&pool, clock.as_ref(), i.as_ref(), PROFILE, "tr_NEXT_WINDOW").await;
    assert_eq!(intake_counts(&pool).await, (31, 31, 31));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn global_rate_and_capacity_apply_across_profiles_and_do_not_depend_on_order_existence(
    pool: PgPool,
) {
    let clock = time();
    let i = ids();
    for profile in 0..4 {
        for n in 0..30 {
            admit(
                &pool,
                clock.as_ref(),
                &i,
                &format!("pfl_RATE_{profile}"),
                &format!("tr_GLOBAL_{profile}_{n}"),
            )
            .await;
        }
    }
    assert!(
        admit_callback(&pool, clock.as_ref(), &i, "pfl_FIFTH", "tr_GLOBAL_OVER")
            .await
            .is_err(),
        "global minute bound is120"
    );
    assert_eq!(intake_counts(&pool).await, (120, 120, 120));
    clock.0.store(1560, Ordering::SeqCst);
    for profile in 0..4 {
        for n in 30..50 {
            admit(
                &pool,
                clock.as_ref(),
                &i,
                &format!("pfl_RATE_{profile}"),
                &format!("tr_GLOBAL_{profile}_{n}"),
            )
            .await;
        }
    }
    clock.0.store(1620, Ordering::SeqCst);
    assert!(
        admit_callback(&pool, clock.as_ref(), &i, "pfl_FIFTH", "tr_CAPACITY_OVER")
            .await
            .is_err(),
        "global unsettled intake capacity is200"
    );
    assert_eq!(intake_counts(&pool).await, (200, 200, 200));
    assert_eq!(counts(&pool).await, (0, 0, 0, 0, 0));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn profile_capacity_invalid_ports_and_clock_rewind_never_bypass_or_partially_consume_admission(
    pool: PgPool,
) {
    let clock = time();
    let i = ids();
    for (profile, id) in [
        ("", "tr_X"),
        ("pfl_", "tr_X"),
        (PROFILE, ""),
        (PROFILE, "tr_"),
        (PROFILE, "tr_X\n"),
    ] {
        assert!(
            admit_callback(&pool, clock.as_ref(), &i, profile, id)
                .await
                .is_err()
        );
    }
    assert_eq!(
        admit_callback(&pool, &BadPorts, &i, PROFILE, "tr_CLOCK")
            .await
            .err(),
        Some(PaymentStoreError::ClockUnavailable)
    );
    assert!(
        admit_callback(&pool, clock.as_ref(), &BadPorts, PROFILE, "tr_ENTROPY")
            .await
            .is_err()
    );
    assert_eq!(intake_counts(&pool).await, (0, 0, 0));
    for n in 0..30 {
        admit(&pool, clock.as_ref(), &i, PROFILE, &format!("tr_FIRST_{n}")).await;
    }
    clock.0.store(1560, Ordering::SeqCst);
    for n in 0..20 {
        admit(
            &pool,
            clock.as_ref(),
            &i,
            PROFILE,
            &format!("tr_SECOND_{n}"),
        )
        .await;
    }
    clock.0.store(1620, Ordering::SeqCst);
    assert!(
        admit_callback(&pool, clock.as_ref(), &i, PROFILE, "tr_PROFILE_OVER")
            .await
            .is_err(),
        "profile unsettled capacity is50"
    );
    assert_eq!(intake_counts(&pool).await, (50, 50, 50));
    let first: String = sqlx::query_scalar("SELECT id FROM callback_intakes ORDER BY id LIMIT 1")
        .fetch_one(&pool)
        .await
        .expect("one active intake");
    let quota_before: Vec<(String, i64, i32, i64)> = sqlx::query_as(
        "SELECT scope,rate_window,admitted,last_at FROM callback_admission_quota ORDER BY scope",
    )
    .fetch_all(&pool)
    .await
    .expect("exact quota before rejected import");
    let mut tx = pool.begin().await.expect("isolated active-profile import");
    tx.execute("ALTER TABLE callback_inbox_active DISABLE TRIGGER USER")
        .await
        .expect("test-only bypass");
    sqlx::query("UPDATE callback_inbox_active SET profile_id='pfl_HIDDEN' WHERE intake_id=$1")
        .bind(&first)
        .execute(&mut *tx)
        .await
        .expect("coherent copied-profile corruption");
    tx.execute("ALTER TABLE callback_inbox_active ENABLE TRIGGER USER")
        .await
        .expect("guard restored");
    tx.commit().await.expect("imported fixture");
    assert!(
        admit_callback(
            &pool,
            clock.as_ref(),
            &i,
            PROFILE,
            "tr_CORRUPT_PROFILE_SLOT"
        )
        .await
        .is_err(),
        "copied profile cannot hide the fiftieth active intake from native admission"
    );
    assert_eq!(intake_counts(&pool).await, (50, 50, 50));
    assert_eq!(
        sqlx::query_as::<_, (String, i64, i32, i64)>(
            "SELECT scope,rate_window,admitted,last_at FROM callback_admission_quota ORDER BY scope"
        )
        .fetch_all(&pool)
        .await
        .expect("quota unchanged after failed admission"),
        quota_before
    );
    let mut tx = pool.begin().await.expect("exact active-header repair");
    tx.execute("ALTER TABLE callback_inbox_active DISABLE TRIGGER USER")
        .await
        .expect("fixture repair");
    sqlx::query("UPDATE callback_inbox_active SET profile_id=$1 WHERE intake_id=$2")
        .bind(PROFILE)
        .bind(&first)
        .execute(&mut *tx)
        .await
        .expect("restore exact original profile");
    tx.execute("ALTER TABLE callback_inbox_active ENABLE TRIGGER USER")
        .await
        .expect("guard restored");
    tx.commit().await.expect("repaired fixture");
    assert!(
        read_callback_job(&pool, &first)
            .await
            .expect("paired healthy row")
            .is_some()
    );
    assert!(
        admit_callback(&pool, clock.as_ref(), &i, PROFILE, "tr_STILL_FULL")
            .await
            .is_err()
    );
    clock.0.store(1559, Ordering::SeqCst);
    assert!(
        admit_callback(&pool, clock.as_ref(), &i, "pfl_OTHER", "tr_REWOUND")
            .await
            .is_err(),
        "global admission time cannot rewind across profile changes"
    );
    assert_eq!(intake_counts(&pool).await, (50, 50, 50));
    clock.0.store(1620, Ordering::SeqCst);
    admit(&pool, clock.as_ref(), &i, "pfl_OTHER", "tr_HEALTHY").await;
    assert_eq!(intake_counts(&pool).await, (51, 51, 51));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn intake_event_insert_wait_clock_rewind_and_cancellation_roll_back_all_admission_rows(
    pool: PgPool,
) {
    let clock = time();
    let i = Arc::new(ids());
    for cancel in [false, true] {
        let mut blocker = pool.begin().await.expect("intake event blocker");
        blocker
            .execute("LOCK TABLE callback_intake_events IN SHARE MODE")
            .await
            .expect("actual source event INSERT wait");
        let (p, t, i2) = (pool.clone(), clock.clone(), i.clone());
        let task = tokio::spawn(async move {
            admit_callback(&p, t.as_ref(), i2.as_ref(), PROFILE, "tr_WAITED_ADMISSION").await
        });
        waiting_query(&pool, "INSERT INTO callback_intake_events%").await;
        if cancel {
            task.abort();
            assert!(
                task.await
                    .err()
                    .expect("cancelled admission")
                    .is_cancelled()
            );
            blocker.commit().await.expect("release");
        } else {
            clock.0.store(1499, Ordering::SeqCst);
            blocker.commit().await.expect("release");
            assert_eq!(
                task.await.expect("admission task").err(),
                Some(PaymentStoreError::ClockUnavailable)
            );
        }
        assert_eq!(intake_counts(&pool).await, (0, 0, 0));
        clock.0.store(1500, Ordering::SeqCst);
    }
    admit(
        &pool,
        clock.as_ref(),
        i.as_ref(),
        PROFILE,
        "tr_AFTER_ROLLBACK",
    )
    .await;
    assert_eq!(intake_counts(&pool).await, (1, 1, 1));
}
