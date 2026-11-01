//! Actual PostgreSQL waits prove rollback and original claim ordering.
use super::*;
use crate::holds::{HoldCommand, transition_hold};
async fn wait_for(pool: &PgPool, pattern: &str, count: i64) {
    tokio::time::timeout(Duration::from_millis(800),async{loop{let actual:i64=sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE $1").bind(pattern).fetch_one(pool).await.expect("actual bounded database wait");if actual>=count{return;}tokio::time::sleep(Duration::from_millis(5)).await;}}).await.expect("reached actual expected claim/write wait");
}
async fn rr_pool(pool: &PgPool) -> PgPool {
    let rr = sqlx::postgres::PgPoolOptions::new()
        .max_connections(6)
        .after_connect(|connection, _| {
            Box::pin(async move {
                connection
                    .execute("SET default_transaction_isolation='repeatable read'")
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
            .expect("default isolation control"),
        "repeatable read"
    );
    rr
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn stock_settlement_final_clock_and_cancellation_after_actual_outbox_wait_rollback_every_member(
    pool: PgPool,
) {
    let f = accepted(&pool, false, true).await;
    let before = footprint(&pool).await;
    for (at, expected) in [
        (Some(1499), SettlementError::ClockUnavailable),
        (Some(2000), SettlementError::RecoveryRequired),
        (None, SettlementError::Database),
    ] {
        let mut blocker = pool.begin().await.expect("outbox blocker");
        blocker
            .execute("LOCK TABLE commerce_outbox IN SHARE MODE")
            .await
            .expect("actual final event write wait");
        let p = pool.clone();
        let t = f.time.clone();
        let ids = f.ids.clone();
        let order = f.order.identity.id.clone();
        let mut task = Some(tokio::spawn(async move {
            settle(&p, t.as_ref(), ids.as_ref(), &order).await
        }));
        crate::payment_test_fixture::waiting(&pool).await;
        assert_eq!(
            footprint(&pool).await,
            before,
            "no partial effects visible while real event write is blocked"
        );
        if let Some(at) = at {
            f.time.0.store(at, Ordering::SeqCst);
        } else {
            let task = task.take().expect("running settlement");
            task.abort();
            assert!(task.await.is_err_and(|error| error.is_cancelled()));
        }
        blocker.commit().await.expect("release event writer");
        if let Some(task) = task.take() {
            assert_eq!(task.await.expect("settlement task").err(), Some(expected));
        }
        let mut recovered = pool.begin().await.expect("rollback claim proof");
        sqlx::query("SELECT id FROM payment_attempts WHERE id=$1 FOR UPDATE")
            .bind(&f.frozen.snapshot().binding.id)
            .execute(&mut *recovered)
            .await
            .expect("cancelled/failed claim recovered");
        recovered.rollback().await.expect("rollback proof");
        assert_eq!(footprint(&pool).await, before);
        f.time.0.store(1500, Ordering::SeqCst);
    }
    assert!(
        !settle(&pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
            .await
            .expect("healthy retry after rollback")
            .replayed
    );
    assert_eq!(counts(&pool).await, (1, 1, 3, 1, 2));
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn stock_settlement_failure_after_first_member_rolls_back_prepared_receipt_and_all_physical_effects(
    pool: PgPool,
) {
    let f = accepted(&pool, false, true).await;
    let before = footprint(&pool).await;
    pool.execute("CREATE FUNCTION fail_settlement_second_member() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.state='consumed' AND NEW.id=(SELECT max(id) FROM stock_holds) THEN IF NOT EXISTS(SELECT 1 FROM stock_holds WHERE id=(SELECT min(id) FROM stock_holds) AND state='consumed') OR (SELECT count(*) FROM stock_movements WHERE action='consume-hold')<>3 THEN RAISE EXCEPTION 'Fixture did not observe completed first member'; END IF; PERFORM pg_advisory_xact_lock(932001); RAISE EXCEPTION 'Injected failure after actual first member'; END IF; RETURN NEW; END $$; CREATE TRIGGER fail_settlement_second_member BEFORE UPDATE ON stock_holds FOR EACH ROW EXECUTE FUNCTION fail_settlement_second_member()").await.expect("test-owned second-member failure instrument");
    let mut gate = pool.begin().await.expect("failure instrument gate");
    gate.execute("SELECT pg_advisory_xact_lock(932001)")
        .await
        .expect("own advisory gate");
    let p = pool.clone();
    let t = f.time.clone();
    let ids = f.ids.clone();
    let order = f.order.identity.id.clone();
    let task = tokio::spawn(async move { settle(&p, t.as_ref(), ids.as_ref(), &order).await });
    wait_for(&pool, "UPDATE stock_holds SET state%", 1).await;
    assert_eq!(footprint(&pool).await, before);
    gate.commit().await.expect(
        "release after instrument saw first consumed member and three actual movement writes",
    );
    assert_eq!(
        task.await.expect("injected task").err(),
        Some(SettlementError::Database)
    );
    assert_eq!(footprint(&pool).await, before);
    pool.execute("DROP TRIGGER fail_settlement_second_member ON stock_holds; DROP FUNCTION fail_settlement_second_member()").await.expect("remove only test-owned failure instrument");
    assert!(
        !settle(&pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
            .await
            .expect("identical healthy operation")
            .replayed
    );
    assert_eq!(counts(&pool).await, (1, 1, 3, 1, 2));
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn stock_settlement_rr_default_wait_after_observation_append_never_uses_old_received_pointer(
    pool: PgPool,
) {
    let f = accepted(&pool, false, true).await;
    let before = footprint(&pool).await;
    let rr = rr_pool(&pool).await;
    let verified = fixture::verified(&f.frozen, "tr_SETTLEMENT", "open").await;
    let mut blocker = pool.begin().await.expect("append outbox gate");
    blocker
        .execute("LOCK TABLE commerce_outbox IN SHARE MODE")
        .await
        .expect("append retains attempt through event");
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
            "racing-open",
            &verified,
        )
        .await
    });
    crate::payment_test_fixture::waiting(&pool).await;
    let p = rr.clone();
    let t = f.time.clone();
    let ids = f.ids.clone();
    let order = f.order.identity.id.clone();
    let settlement =
        tokio::spawn(async move { settle(&p, t.as_ref(), ids.as_ref(), &order).await });
    wait_for(
        &pool,
        "SELECT id FROM payment_attempts WHERE id=%FOR UPDATE",
        1,
    )
    .await;
    blocker.commit().await.expect("append wins");
    append
        .await
        .expect("append task")
        .expect("new evidence commit");
    assert_eq!(
        settlement.await.expect("settlement task").err(),
        Some(SettlementError::Unavailable)
    );
    assert_eq!(footprint(&pool).await, before);
    rr.close().await;
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn stock_settlement_keeps_rc_attempt_claim_through_effects_before_observation_append_can_win(
    pool: PgPool,
) {
    let f = accepted(&pool, false, true).await;
    let rr = rr_pool(&pool).await;
    let verified = fixture::verified(&f.frozen, "tr_SETTLEMENT", "open").await;
    let mut blocker = pool.begin().await.expect("physical gate");
    blocker
        .execute("SELECT component_code FROM stock_balances ORDER BY component_code FOR UPDATE")
        .await
        .expect("all physical components held");
    let p = rr.clone();
    let t = f.time.clone();
    let ids = f.ids.clone();
    let order = f.order.identity.id.clone();
    let settlement =
        tokio::spawn(async move { settle(&p, t.as_ref(), ids.as_ref(), &order).await });
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
            "after-settlement",
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
    blocker
        .commit()
        .await
        .expect("settlement wins physical effects");
    let committed = settlement
        .await
        .expect("settlement task")
        .expect("whole settlement")
        .settlement;
    append
        .await
        .expect("append task")
        .expect("append after settlement commit");
    assert_eq!(counts(&pool).await, (1, 1, 3, 1, 2));
    assert!(
        crate::payment_attempts::recognition::current(&pool, &f.frozen.snapshot().binding.id)
            .await
            .is_err()
    );
    assert!(
        read_settlement(&pool, &f.order.identity.id)
            .await
            .expect("original proof remains historical")
            == Some(committed)
    );
    rr.close().await;
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn stock_settlement_and_expiry_follow_hold_before_component_claims_in_both_winner_orders(
    pool: PgPool,
) {
    let f = accepted(&pool, false, true).await;
    let hold: String = sqlx::query_scalar("SELECT min(id) FROM stock_holds")
        .fetch_one(&pool)
        .await
        .expect("first original member");
    let mut blocker = pool.begin().await.expect("expiry physical gate");
    blocker
        .execute("SELECT component_code FROM stock_balances ORDER BY component_code FOR UPDATE")
        .await
        .expect("component claim");
    let p = pool.clone();
    let id = hold.clone();
    let expiry = tokio::spawn(async move {
        transition_hold(
            &p,
            &id,
            HoldCommand::Expire {
                actor_id: "fixture-expiry",
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
    let t = f.time.clone();
    let ids = f.ids.clone();
    let order = f.order.identity.id.clone();
    let settlement =
        tokio::spawn(async move { settle(&p, t.as_ref(), ids.as_ref(), &order).await });
    wait_for(&pool, "SELECT %FROM basket_hold_members%FOR UPDATE%", 1).await;
    blocker
        .commit()
        .await
        .expect("expiry has original hold first");
    expiry
        .await
        .expect("expiry task")
        .expect("complete expiry wins");
    assert_eq!(
        settlement.await.expect("settlement task").err(),
        Some(SettlementError::RecoveryRequired)
    );
    assert_eq!(counts(&pool).await, (0, 0, 0, 1, 2));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn stock_settlement_wins_hold_and_component_claims_before_waiting_expiry(pool: PgPool) {
    let f = accepted(&pool, false, true).await;
    let hold: String = sqlx::query_scalar("SELECT min(id) FROM stock_holds")
        .fetch_one(&pool)
        .await
        .expect("first original member");
    let mut blocker = pool.begin().await.expect("settlement physical gate");
    blocker
        .execute("SELECT component_code FROM stock_balances ORDER BY component_code FOR UPDATE")
        .await
        .expect("components held");
    let p = pool.clone();
    let t = f.time.clone();
    let ids = f.ids.clone();
    let order = f.order.identity.id.clone();
    let settlement =
        tokio::spawn(async move { settle(&p, t.as_ref(), ids.as_ref(), &order).await });
    wait_for(&pool, "SELECT %FROM stock_balances%FOR UPDATE", 1).await;
    let p = pool.clone();
    let expiry = tokio::spawn(async move {
        transition_hold(
            &p,
            &hold,
            HoldCommand::Expire {
                actor_id: "fixture-expiry",
            },
            2000,
        )
        .await
    });
    wait_for(&pool, "SELECT * FROM stock_holds WHERE id=%FOR UPDATE", 1).await;
    blocker
        .commit()
        .await
        .expect("settlement owns all holds before expiry");
    settlement
        .await
        .expect("settlement task")
        .expect("whole original consumption");
    assert!(
        expiry.await.expect("expiry task").is_err(),
        "consumed hold cannot become expired"
    );
    assert_eq!(counts(&pool).await, (1, 1, 3, 1, 2));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM stock_movements WHERE action='expire-hold'"
        )
        .fetch_one(&pool)
        .await
        .expect("no expiry effects"),
        0
    );
}
struct DescendingIds(AtomicU64);
impl commerce_application::request_context::Identifiers for DescendingIds {
    fn next(&self) -> Result<[u8; 16], commerce_application::request_context::ContextError> {
        Ok(u128::from(self.0.fetch_sub(1, Ordering::SeqCst)).to_be_bytes())
    }
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn opposite_member_sku_ordering_on_shared_components_settles_two_orders_without_deadlock(
    pool: PgPool,
) {
    let first = accepted(&pool, false, true).await;
    receive(
        &pool,
        "second-order-foam",
        "SEAT-FOAM",
        commerce_domain::inventory::WarehouseChange::Receive(2),
    )
    .await;
    let descending = crate::guest_baskets::GuestSessionStore::new(
        pool.clone(),
        first.time.clone(),
        Arc::new(DescendingIds(AtomicU64::new(0xf0000000))),
        commerce_application::guest_access::GuestLimits::default(),
    )
    .expect("independent descending-ID owner");
    let second =
        fixture::accepted_with_store(&pool, descending, first.time.clone(), false, false).await;
    observe(&pool, &second, "tr_SECOND_ORDER", "paid", "second-paid").await;
    recognize(
        &pool,
        second.time.as_ref(),
        second.ids.as_ref(),
        &second.frozen.snapshot().binding.id,
    )
    .await
    .expect("independently paid second original order");
    let first_skus: Vec<String> =
        sqlx::query_scalar("SELECT sku FROM stock_holds WHERE basket_group_id=$1 ORDER BY id")
            .bind(&first.order.identity.stock_group_id)
            .fetch_all(&pool)
            .await
            .expect("first captured order");
    let second_skus: Vec<String> =
        sqlx::query_scalar("SELECT sku FROM stock_holds WHERE basket_group_id=$1 ORDER BY id")
            .bind(&second.order.identity.stock_group_id)
            .fetch_all(&pool)
            .await
            .expect("opposite captured order");
    assert_eq!(first_skus, vec!["SET-COVERS", "SET-FILLED"]);
    assert_eq!(second_skus, vec!["SET-FILLED", "SET-COVERS"]);
    let mut blocker = pool.begin().await.expect("shared physical gate");
    blocker
        .execute("SELECT component_code FROM stock_balances ORDER BY component_code FOR UPDATE")
        .await
        .expect("all shared balances");
    let mut tasks = vec![];
    for f in [&first, &second] {
        let p = pool.clone();
        let t = f.time.clone();
        let ids = f.ids.clone();
        let order = f.order.identity.id.clone();
        tasks.push(tokio::spawn(async move {
            settle(&p, t.as_ref(), ids.as_ref(), &order).await
        }));
    }
    wait_for(&pool, "SELECT %FROM stock_balances%FOR UPDATE", 2).await;
    blocker
        .commit()
        .await
        .expect("release same ordered union claims");
    for task in tasks {
        assert!(
            !task
                .await
                .expect("settlement task")
                .expect("both whole orders fit")
                .replayed
        );
    }
    assert_eq!(counts(&pool).await, (2, 2, 6, 2, 4));
    assert_eq!(
        balances(&pool).await,
        vec![
            ("SEAT-COVER".into(), 2, 1, 0),
            ("SEAT-FOAM".into(), 2, 1, 0)
        ]
    );
    for f in [&first, &second] {
        assert!(
            read_settlement(&pool, &f.order.identity.id)
                .await
                .expect("history survives other order's legitimate physical consumption")
                .is_some()
        );
    }
}
