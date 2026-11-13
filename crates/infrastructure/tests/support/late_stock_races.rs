//! Ordered real wait queues exercise winner order, rollback and physical scarcity.
use super::*;
use commerce_application::request_context::{Clock, ContextError, Identifiers};
use std::sync::atomic::AtomicU64;
struct Fixed(u64);
impl Clock for Fixed {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        Ok(self.0)
    }
}
async fn cross_path(pool: PgPool, late_first: bool) {
    let f = accepted(&pool, false, true).await;
    let mut blocker = pool.begin().await.expect("physical winner gate");
    blocker
        .execute("SELECT component_code FROM stock_balances ORDER BY component_code FOR UPDATE")
        .await
        .expect("actual union component claims");
    let p = pool.clone();
    let ids = f.ids.clone();
    let order = f.order.identity.id.clone();
    let original = if !late_first {
        Some(tokio::spawn(async move {
            crate::late_stock_wait_budget::scope(crate::order_stock::settle(
                &p,
                &Fixed(1999),
                ids.as_ref(),
                &order,
            ))
            .await
        }))
    } else {
        None
    };
    let p = pool.clone();
    let ids = f.ids.clone();
    let order = f.order.identity.id.clone();
    let late = if late_first {
        Some(tokio::spawn(async move {
            crate::late_stock_wait_budget::scope(recover(&p, &Fixed(2000), ids.as_ref(), &order))
                .await
        }))
    } else {
        None
    };
    wait_for(&pool, "SELECT %FROM stock_balances%FOR UPDATE", 1).await;
    let p = pool.clone();
    let ids = f.ids.clone();
    let order = f.order.identity.id.clone();
    let late = if let Some(late) = late {
        late
    } else {
        tokio::spawn(async move {
            crate::late_stock_wait_budget::scope(recover(&p, &Fixed(2000), ids.as_ref(), &order))
                .await
        })
    };
    let p = pool.clone();
    let ids = f.ids.clone();
    let order = f.order.identity.id.clone();
    let original = if let Some(original) = original {
        original
    } else {
        tokio::spawn(async move {
            crate::late_stock_wait_budget::scope(crate::order_stock::settle(
                &p,
                &Fixed(1999),
                ids.as_ref(),
                &order,
            ))
            .await
        })
    };
    wait_for(
        &pool,
        "SELECT id FROM payment_attempts WHERE id=%FOR UPDATE",
        1,
    )
    .await;
    // The original-winner control holds both real queues beyond the normal
    // one-second lock timeout; success demonstrates the scoped budget is used.
    if !late_first {
        tokio::time::sleep(Duration::from_millis(1200)).await;
    }
    blocker
        .commit()
        .await
        .expect("release ordered first winner");
    let old = original.await.expect("original task");
    let recovery = late
        .await
        .expect("late task")
        .expect("canonical recovery outcome");
    if late_first {
        assert!(matches!(recovery.outcome, Outcome::Late(_)));
        assert_eq!(
            old.err(),
            Some(crate::order_stock::SettlementError::Unavailable)
        );
        assert_eq!(allocation_counts(&pool).await, (1, 1, 0, 0, 2, 4, 3, 3, 6));
    } else {
        let old = old.expect("original wins");
        assert!(recovery.replayed && recovery.outcome == Outcome::Original(old.settlement));
        assert_eq!(allocation_counts(&pool).await, (1, 0, 0, 1, 1, 2, 3, 0, 3));
    }
    assert_eq!(
        balances(&pool).await,
        vec![
            ("SEAT-COVER".into(), 6, 1, 0),
            ("SEAT-FOAM".into(), 2, 1, 0)
        ]
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn late_stock_original_settlement_wins_actual_attempt_wait_and_late_call_replays_it(
    pool: PgPool,
) {
    cross_path(pool, false).await;
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn late_stock_late_replacement_wins_actual_attempt_wait_and_original_path_cannot_double_consume(
    pool: PgPool,
) {
    cross_path(pool, true).await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn late_stock_scarcity_savepoint_releases_balances_and_changed_availability_returns_state_changed_without_case(
    pool: PgPool,
) {
    let f = accepted(&pool, false, true).await;
    expire_original(&pool, &f).await;
    receive(
        &pool,
        "scarce-before-race",
        "SEAT-FOAM",
        WarehouseChange::Count(2),
    )
    .await;
    let mut foam = pool.begin().await.expect("first assessment foam gate");
    foam.execute(
        "SELECT component_code FROM stock_balances WHERE component_code='SEAT-FOAM' FOR UPDATE",
    )
    .await
    .expect("hold second DB-ordered component");
    let mut gate = pool.begin().await.expect("restock phase gate");
    gate.execute("SELECT pg_advisory_xact_lock(938005)")
        .await
        .expect("hold restock after fresh savepoint rollback");
    let p = pool.clone();
    let t = f.time.clone();
    let ids = f.ids.clone();
    let order = f.order.identity.id.clone();
    let recovery = tokio::spawn(async move { recover(&p, t.as_ref(), ids.as_ref(), &order).await });
    wait_for(&pool, "SELECT %FROM stock_balances%FOR UPDATE", 1).await;
    let p = pool.clone();
    let restock = tokio::spawn(async move {
        let mut tx = p.begin().await.expect("queued real restock");
        tx.execute("SELECT component_code FROM stock_balances ORDER BY component_code FOR UPDATE")
            .await
            .expect("claim both components after savepoint releases them");
        tx.execute("SELECT pg_advisory_xact_lock(938005)")
            .await
            .expect("restock gate");
        tx.execute("UPDATE stock_balances SET on_hand=on_hand+3 WHERE component_code='SEAT-FOAM'; INSERT INTO stock_movements(effect_key,component_code,action,quantity,actor,reason,on_hand_delta,quarantine_delta,reserved_delta,on_hand_after,quarantine_after,reserved_after) VALUES('restock-after-rollback','SEAT-FOAM','receive',3,'fixture-warehouse','synthetic physical custody',3,0,0,5,1,0)").await.expect("actual physical restock with exact receive ledger");
        tx.commit().await.expect("restock commit");
    });
    wait_for(&pool, "SELECT %FROM stock_balances%FOR UPDATE", 2).await;
    foam.commit().await.expect("first snapshot remains scarce");
    wait_for(&pool, "SELECT pg_advisory_xact_lock(938005)", 1).await;
    wait_for(&pool, "SELECT %FROM stock_balances%FOR UPDATE", 1).await;
    gate.commit()
        .await
        .expect("let restock finish before second locked assessment");
    restock.await.expect("restock task");
    assert_eq!(
        recovery.await.expect("recovery task").err(),
        Some(LateStockError::StateChanged)
    );
    assert_eq!(allocation_counts(&pool).await, (0, 0, 0, 0, 1, 2, 0, 3, 3));
    assert_eq!(
        balances(&pool).await,
        vec![
            ("SEAT-COVER".into(), 10, 1, 0),
            ("SEAT-FOAM".into(), 5, 1, 0)
        ]
    );
    assert!(matches!(
        recover(&pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
            .await
            .expect("new call after changed assessment can allocate")
            .outcome,
        Outcome::Late(_)
    ));
}

struct DescendingIds(AtomicU64);
impl Identifiers for DescendingIds {
    fn next(&self) -> Result<[u8; 16], ContextError> {
        Ok(u128::from(self.0.fetch_sub(1, Ordering::SeqCst)).to_be_bytes())
    }
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn late_stock_two_opposite_member_orders_race_last_foam_without_deadlock_or_partial_second_allocation(
    pool: PgPool,
) {
    let first = accepted(&pool, false, true).await;
    receive(
        &pool,
        "second-original-foam",
        "SEAT-FOAM",
        WarehouseChange::Receive(2),
    )
    .await;
    let store = crate::guest_baskets::GuestSessionStore::new(
        pool.clone(),
        first.time.clone(),
        Arc::new(DescendingIds(AtomicU64::new(0xf0000000))),
        commerce_application::guest_access::GuestLimits::default(),
    )
    .expect("independent owner with opposite IDs");
    let second = fixture::accepted_with_store(&pool, store, first.time.clone(), false, false).await;
    observe(
        &pool,
        &second,
        "tr_SECOND_LATE_ORDER",
        "paid",
        "second-late-paid",
    )
    .await;
    recognize(
        &pool,
        second.time.as_ref(),
        second.ids.as_ref(),
        &second.frozen.snapshot().binding.id,
    )
    .await
    .expect("second genuine received order");
    let skus: Vec<String> =
        sqlx::query_scalar("SELECT sku FROM stock_holds WHERE basket_group_id=$1 ORDER BY id")
            .bind(&second.order.identity.stock_group_id)
            .fetch_all(&pool)
            .await
            .expect("opposite original hold ordering");
    assert_eq!(skus, vec!["SET-FILLED", "SET-COVERS"]);
    for f in [&first, &second] {
        for hold in original_holds(&pool, f).await {
            transition_hold(
                &pool,
                &hold,
                HoldCommand::Expire {
                    actor_id: "fixture-expiry",
                },
                2400,
            )
            .await
            .expect("complete original expiry");
        }
    }
    first.time.0.store(2400, Ordering::SeqCst);
    receive(
        &pool,
        "only-last-foam-demand",
        "SEAT-FOAM",
        WarehouseChange::Count(4),
    )
    .await;
    let mut blocker = pool.begin().await.expect("shared component race gate");
    blocker
        .execute("SELECT component_code FROM stock_balances ORDER BY component_code FOR UPDATE")
        .await
        .expect("all shared components");
    let mut tasks = vec![];
    for f in [&first, &second] {
        let p = pool.clone();
        let t = f.time.clone();
        let ids = f.ids.clone();
        let order = f.order.identity.id.clone();
        tasks.push(tokio::spawn(async move {
            recover(&p, t.as_ref(), ids.as_ref(), &order).await
        }));
    }
    wait_for(&pool, "SELECT %FROM stock_balances%FOR UPDATE", 2).await;
    blocker
        .commit()
        .await
        .expect("same component order for opposite SKU orders");
    let mut allocated = 0;
    let mut reviewed = 0;
    for task in tasks {
        let receipt = task
            .await
            .expect("recovery task")
            .expect("allocation or immutable shortage stop");
        match receipt.outcome {
            Outcome::Late(_) => allocated += 1,
            Outcome::NeedsReview(r) => {
                assert_eq!(r.reason, ReviewReason::ComponentScarcity);
                reviewed += 1;
            }
            Outcome::Original(_) => panic!("no prior canonical original allocation"),
        }
    }
    assert_eq!((allocated, reviewed), (1, 1));
    assert_eq!(allocation_counts(&pool).await, (1, 1, 1, 0, 3, 6, 3, 6, 9));
    assert_eq!(
        balances(&pool).await,
        vec![
            ("SEAT-COVER".into(), 6, 1, 0),
            ("SEAT-FOAM".into(), 2, 1, 0)
        ]
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn late_stock_expiry_wins_original_hold_before_late_recovery_and_mixed_source_expires_only_remaining_active(
    pool: PgPool,
) {
    let f = accepted(&pool, false, true).await;
    f.time.0.store(2000, Ordering::SeqCst);
    let holds = original_holds(&pool, &f).await;
    let mut blocker = pool.begin().await.expect("expiry physical gate");
    blocker
        .execute("SELECT component_code FROM stock_balances ORDER BY component_code FOR UPDATE")
        .await
        .expect("physical union gate");
    let p = pool.clone();
    let hold = holds[0].clone();
    let expiry = tokio::spawn(async move {
        transition_hold(
            &p,
            &hold,
            HoldCommand::Expire {
                actor_id: "fixture-expiry-first",
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
    let recovery = tokio::spawn(async move { recover(&p, t.as_ref(), ids.as_ref(), &order).await });
    wait_for(&pool, "SELECT %FROM basket_hold_members%FOR UPDATE OF h", 1).await;
    blocker.commit().await.expect("expiry wins first hold");
    assert!(
        !expiry
            .await
            .expect("expiry task")
            .expect("actual original expiry")
            .replayed
    );
    let Outcome::Late(late) = recovery
        .await
        .expect("recovery task")
        .expect("mixed active/expired source eligible")
        .outcome
    else {
        panic!("mixed intact source must recover")
    };
    assert_eq!(
        late.footprint
            .original
            .iter()
            .filter(|m| m.state == OriginalState::Expired)
            .count(),
        1
    );
    assert_eq!(
        late.footprint
            .original
            .iter()
            .filter(|m| m.state == OriginalState::Active)
            .count(),
        1
    );
    assert_eq!(allocation_counts(&pool).await, (1, 1, 0, 0, 2, 4, 3, 3, 6));
    replay_without_ports(&pool, &f, Outcome::Late(late)).await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn late_stock_recovery_wins_all_original_hold_claims_before_expiry_waiter_and_does_not_double_expire(
    pool: PgPool,
) {
    let f = accepted(&pool, false, true).await;
    f.time.0.store(2000, Ordering::SeqCst);
    let holds = original_holds(&pool, &f).await;
    let mut blocker = pool.begin().await.expect("recovery physical gate");
    blocker
        .execute("SELECT component_code FROM stock_balances ORDER BY component_code FOR UPDATE")
        .await
        .expect("physical union gate");
    let p = pool.clone();
    let t = f.time.clone();
    let ids = f.ids.clone();
    let order = f.order.identity.id.clone();
    let recovery = tokio::spawn(async move { recover(&p, t.as_ref(), ids.as_ref(), &order).await });
    wait_for(&pool, "SELECT %FROM stock_balances%FOR UPDATE", 1).await;
    let p = pool.clone();
    let hold = holds[0].clone();
    let expiry = tokio::spawn(async move {
        transition_hold(
            &p,
            &hold,
            HoldCommand::Expire {
                actor_id: "fixture-waiting-expiry",
            },
            2000,
        )
        .await
    });
    wait_for(&pool, "SELECT * FROM stock_holds WHERE id=%FOR UPDATE", 1).await;
    blocker
        .commit()
        .await
        .expect("recovery owns every original hold first");
    let receipt = recovery
        .await
        .expect("recovery task")
        .expect("complete late replacement");
    assert!(
        expiry
            .await
            .expect("expiry task")
            .expect("same already expired source replays")
            .replayed
    );
    assert_eq!(allocation_counts(&pool).await, (1, 1, 0, 0, 2, 4, 3, 3, 6));
    replay_without_ports(&pool, &f, receipt.outcome).await;
}
