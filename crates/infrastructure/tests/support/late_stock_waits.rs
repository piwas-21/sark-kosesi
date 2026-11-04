//! Real locks prove retained financial custody and rollback at the final write.
use super::*;
async fn wait_for(pool: &PgPool, pattern: &str, count: i64) {
    tokio::time::timeout(Duration::from_secs(3),async{loop{let actual:i64=sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE $1").bind(pattern).fetch_one(pool).await.expect("actual database wait");if actual>=count{return;}tokio::time::sleep(Duration::from_millis(5)).await;}}).await.expect("actual bounded expected lock wait");
}
async fn rr_pool(pool: &PgPool) -> PgPool {
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
    assert_eq!(
        sqlx::query_scalar::<_, String>("SHOW default_transaction_isolation")
            .fetch_one(&rr)
            .await
            .expect("RR default control"),
        "repeatable read"
    );
    rr
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn late_stock_final_outbox_wait_rewind_fresh_expiry_and_cancel_rollback_every_phase(
    pool: PgPool,
) {
    let f = accepted(&pool, false, true).await;
    f.time.0.store(2000, Ordering::SeqCst);
    let before = all_effects(&pool).await;
    for (time, error) in [
        (Some(1999), LateStockError::ClockUnavailable),
        (Some(2060), LateStockError::StateChanged),
        (None, LateStockError::Database),
    ] {
        let mut blocker = pool.begin().await.expect("outbox gate");
        blocker
            .execute("LOCK TABLE commerce_outbox IN SHARE MODE")
            .await
            .expect("actual final event wait");
        let p = pool.clone();
        let t = f.time.clone();
        let ids = f.ids.clone();
        let order = f.order.identity.id.clone();
        let mut task = Some(tokio::spawn(async move {
            recover(&p, t.as_ref(), ids.as_ref(), &order).await
        }));
        crate::payment_test_fixture::waiting(&pool).await;
        assert_eq!(
            all_effects(&pool).await,
            before,
            "no partial groups, movement or receipt visible"
        );
        if let Some(at) = time {
            f.time.0.store(at, Ordering::SeqCst);
        } else {
            let task = task.take().expect("live task");
            task.abort();
            assert!(task.await.is_err_and(|e| e.is_cancelled()));
        }
        blocker.commit().await.expect("release outbox");
        if let Some(task) = task {
            assert_eq!(task.await.expect("allocation task").err(), Some(error));
        }
        let mut proof = pool.begin().await.expect("rollback cleanup proof");
        sqlx::query("SELECT id FROM payment_attempts WHERE id=$1 FOR UPDATE")
            .bind(&f.frozen.snapshot().binding.id)
            .execute(&mut *proof)
            .await
            .expect("attempt released after cancel/failure");
        proof.rollback().await.expect("proof rollback");
        assert_eq!(all_effects(&pool).await, before);
        f.time.0.store(2000, Ordering::SeqCst);
    }
    assert!(
        !recover(&pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
            .await
            .expect("healthy identical retry")
            .replayed
    );
    assert_eq!(allocation_counts(&pool).await, (1, 1, 0, 0, 2, 4, 3, 3, 6));
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn late_stock_actual_component_wait_cannot_extend_prepared_sixty_second_window(pool: PgPool) {
    let f = accepted(&pool, false, true).await;
    f.time.0.store(2000, Ordering::SeqCst);
    let before = all_effects(&pool).await;
    let mut blocker = pool.begin().await.expect("component gate");
    blocker
        .execute("SELECT component_code FROM stock_balances ORDER BY component_code FOR UPDATE")
        .await
        .expect("real component claims");
    let p = pool.clone();
    let t = f.time.clone();
    let ids = f.ids.clone();
    let order = f.order.identity.id.clone();
    let task = tokio::spawn(async move { recover(&p, t.as_ref(), ids.as_ref(), &order).await });
    wait_for(&pool, "SELECT %FROM stock_balances%FOR UPDATE", 1).await;
    f.time.0.store(2060, Ordering::SeqCst);
    blocker
        .commit()
        .await
        .expect("release after prepared window ended");
    assert_eq!(
        task.await.expect("allocation task").err(),
        Some(LateStockError::StateChanged)
    );
    assert_eq!(all_effects(&pool).await, before);
    f.time.0.store(2061, Ordering::SeqCst);
    let Outcome::Late(late) = recover(&pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
        .await
        .expect("fresh later retry")
        .outcome
    else {
        panic!("healthy full demand")
    };
    assert_eq!(late.allocated_at, 2061);
    assert_eq!(late.expires_at, 2121);
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn late_stock_rr_default_wait_observation_wins_and_old_received_pointer_cannot_allocate(
    pool: PgPool,
) {
    let f = accepted(&pool, false, true).await;
    f.time.0.store(2000, Ordering::SeqCst);
    let before = balances(&pool).await;
    let rr = rr_pool(&pool).await;
    let verified = fixture::verified(&f.frozen, "tr_SETTLEMENT", "open").await;
    let mut blocker = pool.begin().await.expect("observation event gate");
    blocker
        .execute("LOCK TABLE commerce_outbox IN SHARE MODE")
        .await
        .expect("append retains attempt");
    let p = pool.clone();
    let t = f.time.clone();
    let ids = f.ids.clone();
    let attempt = f.frozen.snapshot().binding.id.clone();
    let append = tokio::spawn(async move {
        record_verified(
            &p,
            t.as_ref(),
            ids.as_ref(),
            &attempt,
            "late-racing-open",
            &verified,
        )
        .await
    });
    crate::payment_test_fixture::waiting(&pool).await;
    let p = rr.clone();
    let t = f.time.clone();
    let ids = f.ids.clone();
    let order = f.order.identity.id.clone();
    let recovery = tokio::spawn(async move { recover(&p, t.as_ref(), ids.as_ref(), &order).await });
    wait_for(
        &pool,
        "SELECT id FROM payment_attempts WHERE id=%FOR UPDATE",
        1,
    )
    .await;
    blocker.commit().await.expect("observation commits first");
    append.await.expect("append task").expect("new observation");
    assert_eq!(
        recovery.await.expect("recovery task").err(),
        Some(LateStockError::Unavailable)
    );
    assert_eq!(balances(&pool).await, before);
    assert_eq!(allocation_counts(&pool).await, (0, 0, 0, 0, 1, 2, 0, 0, 3));
    rr.close().await;
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn late_stock_keeps_rc_attempt_claim_until_all_effects_commit_before_observation_append(
    pool: PgPool,
) {
    let f = accepted(&pool, false, true).await;
    f.time.0.store(2000, Ordering::SeqCst);
    let rr = rr_pool(&pool).await;
    let verified = fixture::verified(&f.frozen, "tr_SETTLEMENT", "open").await;
    let mut blocker = pool.begin().await.expect("physical gate");
    blocker
        .execute("SELECT component_code FROM stock_balances ORDER BY component_code FOR UPDATE")
        .await
        .expect("all affected balances");
    let p = rr.clone();
    let t = f.time.clone();
    let ids = f.ids.clone();
    let order = f.order.identity.id.clone();
    let recovery = tokio::spawn(async move { recover(&p, t.as_ref(), ids.as_ref(), &order).await });
    wait_for(&pool, "SELECT %FROM stock_balances%FOR UPDATE", 1).await;
    let p = pool.clone();
    let t = f.time.clone();
    let ids = f.ids.clone();
    let attempt = f.frozen.snapshot().binding.id.clone();
    let append = tokio::spawn(async move {
        record_verified(
            &p,
            t.as_ref(),
            ids.as_ref(),
            &attempt,
            "after-late-allocation",
            &verified,
        )
        .await
    });
    wait_for(
        &pool,
        "SELECT left(basket_id,33) FROM payment_attempts%FOR UPDATE",
        1,
    )
    .await;
    blocker.commit().await.expect("whole stock allocation wins");
    let receipt = recovery
        .await
        .expect("recovery task")
        .expect("one late allocation");
    append
        .await
        .expect("append task")
        .expect("append only after stock commit");
    assert_eq!(allocation_counts(&pool).await, (1, 1, 0, 0, 2, 4, 3, 3, 6));
    replay_without_ports(&pool, &f, receipt.outcome).await;
    rr.close().await;
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn late_stock_duplicate_waiters_return_one_canonical_fresh_group_without_double_consumption(
    pool: PgPool,
) {
    let f = accepted(&pool, false, true).await;
    f.time.0.store(2000, Ordering::SeqCst);
    let mut blocker = pool.begin().await.expect("original attempt gate");
    sqlx::query("SELECT id FROM payment_attempts WHERE id=$1 FOR UPDATE")
        .bind(&f.frozen.snapshot().binding.id)
        .execute(&mut *blocker)
        .await
        .expect("actual retained attempt lock");
    let mut tasks = vec![];
    for index in 0..2 {
        let p = pool.clone();
        let t = f.time.clone();
        let ids = f.ids.clone();
        let order = f.order.identity.id.clone();
        tasks.push(tokio::spawn(async move {
            if index == 0 {
                recover(&p, t.as_ref(), ids.as_ref(), &order).await
            } else {
                let bad = super::super::observation_cases::BadPorts;
                recover(&p, &bad, &bad, &order).await
            }
        }));
        wait_for(
            &pool,
            "SELECT id FROM payment_attempts WHERE id=%FOR UPDATE",
            index + 1,
        )
        .await;
    }
    blocker.commit().await.expect("release duplicate waiters");
    let mut outcomes = vec![];
    let mut replays = 0;
    for task in tasks {
        let receipt = task
            .await
            .expect("recovery task")
            .expect("canonical allocation");
        replays += usize::from(receipt.replayed);
        outcomes.push(receipt.outcome);
    }
    assert_eq!(replays, 1);
    assert!(outcomes[0] == outcomes[1]);
    assert_eq!(allocation_counts(&pool).await, (1, 1, 0, 0, 2, 4, 3, 3, 6));
}
#[path = "late_stock_failures.rs"]
mod failures;

#[sqlx::test(migrator = "MIGRATOR")]
async fn late_stock_review_history_claims_captured_active_hold_share_before_later_terminal_custody(
    pool: PgPool,
) {
    let f = accepted(&pool, false, true).await;
    let holds = original_holds(&pool, &f).await;
    transition_hold(
        &pool,
        &holds[0],
        HoldCommand::Release {
            owner_id: &f.order.identity.id,
            actor_id: "fixture-release",
        },
        1500,
    )
    .await
    .expect("genuine release review source");
    f.time.0.store(2000, Ordering::SeqCst);
    let outcome = recover(&pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
        .await
        .expect("captured active companion")
        .outcome;
    let mut blocker = pool
        .begin()
        .await
        .expect("terminal transition physical gate");
    blocker
        .execute("SELECT component_code FROM stock_balances ORDER BY component_code FOR UPDATE")
        .await
        .expect("terminal writer holds original before component effects");
    let p = pool.clone();
    let hold = holds[1].clone();
    let expiry = tokio::spawn(async move {
        transition_hold(
            &p,
            &hold,
            HoldCommand::Expire {
                actor_id: "fixture-later-expiry",
            },
            2000,
        )
        .await
    });
    wait_for(
        &pool,
        "SELECT on_hand,quarantine,reserved FROM stock_balances%FOR UPDATE",
        1,
    )
    .await;
    let p = pool.clone();
    let order = f.order.identity.id.clone();
    let history = tokio::spawn(async move { read_recorded(&p, &order).await });
    wait_for(&pool, "SELECT %FROM basket_hold_members%FOR SHARE OF h", 1).await;
    blocker
        .commit()
        .await
        .expect("complete genuine later terminal custody");
    expiry
        .await
        .expect("expiry task")
        .expect("later active member may expire");
    assert!(
        history
            .await
            .expect("history task")
            .expect("coherent historical assessment")
            == Some(outcome.clone())
    );
    replay_without_ports(&pool, &f, outcome).await;
}
#[path = "late_stock_races.rs"]
mod races;

#[sqlx::test(migrator = "MIGRATOR")]
async fn late_stock_live_original_crossing_its_deadline_during_actual_component_wait_returns_state_changed(
    pool: PgPool,
) {
    let f = accepted(&pool, false, true).await;
    f.time.0.store(1999, Ordering::SeqCst);
    let before = all_effects(&pool).await;
    let mut blocker = pool.begin().await.expect("live-window physical gate");
    blocker
        .execute("SELECT component_code FROM stock_balances ORDER BY component_code FOR UPDATE")
        .await
        .expect("actual component union wait");
    let p = pool.clone();
    let time = f.time.clone();
    let ids = f.ids.clone();
    let order = f.order.identity.id.clone();
    let task = tokio::spawn(async move { recover(&p, time.as_ref(), ids.as_ref(), &order).await });
    wait_for(&pool, "SELECT %FROM stock_balances%FOR UPDATE", 1).await;
    f.time.0.store(2000, Ordering::SeqCst);
    blocker
        .commit()
        .await
        .expect("original deadline passed while assessing live source");
    assert_eq!(
        task.await.expect("live-window task").err(),
        Some(LateStockError::StateChanged)
    );
    assert_eq!(all_effects(&pool).await, before);
    assert!(
        !recover(&pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
            .await
            .expect("new bounded call after deadline can allocate")
            .replayed
    );
}
