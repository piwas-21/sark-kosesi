//! Only deadline-past intact active/expired custody permits automatic replacement.
use super::*;

#[sqlx::test(migrator = "MIGRATOR")]
async fn late_stock_active_window_and_unconfirmed_stale_or_ambiguous_finance_never_allocate(
    pool: PgPool,
) {
    let f = accepted(&pool, false, false).await;
    let order = &f.order.identity.id;
    let attempt = &f.frozen.snapshot().binding.id;
    let before = all_effects(&pool).await;
    assert!(
        read_recorded(&pool, order)
            .await
            .expect("no history")
            .is_none()
    );
    assert_eq!(
        recover(&pool, f.time.as_ref(), f.ids.as_ref(), order)
            .await
            .err(),
        Some(LateStockError::Unavailable)
    );
    resolve(&pool, f.time.as_ref(), f.ids.as_ref(), attempt, "zero")
        .await
        .expect("complete zero identity prefix");
    recognize(&pool, f.time.as_ref(), f.ids.as_ref(), attempt)
        .await
        .expect("unconfirmed receipt");
    let financial = all_effects(&pool).await;
    assert_eq!(
        recover(&pool, f.time.as_ref(), f.ids.as_ref(), order)
            .await
            .err(),
        Some(LateStockError::Unavailable)
    );
    assert_eq!(all_effects(&pool).await, financial);
    observe(&pool, &f, "tr_SETTLEMENT", "paid", "paid").await;
    let financial = all_effects(&pool).await;
    assert_eq!(
        recover(&pool, f.time.as_ref(), f.ids.as_ref(), order)
            .await
            .err(),
        Some(LateStockError::Unavailable)
    );
    assert_eq!(
        all_effects(&pool).await,
        financial,
        "stale pointer cannot allocate"
    );
    recognize(&pool, f.time.as_ref(), f.ids.as_ref(), attempt)
        .await
        .expect("received");
    let financial = all_effects(&pool).await;
    assert_eq!(
        recover(&pool, f.time.as_ref(), f.ids.as_ref(), order)
            .await
            .err(),
        Some(LateStockError::NotApplicable)
    );
    assert_eq!(all_effects(&pool).await, financial, "original still live");
    observe(&pool, &f, "tr_OTHER", "open", "ambiguous").await;
    recognize(&pool, f.time.as_ref(), f.ids.as_ref(), attempt)
        .await
        .expect("needs review");
    f.time.0.store(2000, Ordering::SeqCst);
    let financial = all_effects(&pool).await;
    assert_eq!(
        recover(&pool, f.time.as_ref(), f.ids.as_ref(), order)
            .await
            .err(),
        Some(LateStockError::Unavailable)
    );
    assert_eq!(all_effects(&pool).await, financial);
    assert_ne!(
        financial, before,
        "financial control actually changed its source"
    );
    assert_eq!(allocation_counts(&pool).await, (0, 0, 0, 0, 1, 2, 0, 0, 3));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn late_stock_release_intent_creates_one_immutable_review_without_allocation_or_retry_after_restock(
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
    .expect("genuine first release");
    f.time.0.store(2000, Ordering::SeqCst);
    let before = balances(&pool).await;
    let source = original_snapshot(&pool).await;
    let receipt = recover(&pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
        .await
        .expect("durable explicit review stop");
    assert!(!receipt.replayed);
    let Outcome::NeedsReview(case) = &receipt.outcome else {
        panic!("release intent cannot authorize stock")
    };
    assert_eq!(case.reason, ReviewReason::ReleaseIntentUnresolved);
    assert_eq!(case.actor, "late-stock-review");
    assert_eq!(case.assessed_at, 2000);
    assert!(
        case.footprint
            .original
            .iter()
            .any(|m| m.state == OriginalState::Released)
    );
    assert!(
        case.footprint
            .original
            .iter()
            .any(|m| m.state == OriginalState::Active)
    );
    assert_eq!(balances(&pool).await, before);
    assert_eq!(original_snapshot(&pool).await, source);
    assert_eq!(allocation_counts(&pool).await, (0, 0, 1, 0, 1, 2, 0, 0, 3));
    receive(
        &pool,
        "later-restock",
        "SEAT-COVER",
        WarehouseChange::Receive(7),
    )
    .await;
    transition_hold(
        &pool,
        &holds[1],
        HoldCommand::Expire {
            actor_id: "fixture-later-expiry",
        },
        2000,
    )
    .await
    .expect("captured active can later expire coherently");
    replay_without_ports(&pool, &f, receipt.outcome).await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn late_stock_prior_consumption_without_allocation_requires_review_instead_of_adoption(
    pool: PgPool,
) {
    let f = accepted(&pool, false, true).await;
    let holds = original_holds(&pool, &f).await;
    for hold in holds {
        transition_hold(
            &pool,
            &hold,
            HoldCommand::Consume {
                owner_id: &f.order.identity.id,
                actor_id: "fixture-prior-consumer",
            },
            1500,
        )
        .await
        .expect("complete physical consumption without allocation journal");
    }
    f.time.0.store(2000, Ordering::SeqCst);
    let before = balances(&pool).await;
    let receipt = recover(&pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
        .await
        .expect("prior consumption stop");
    let Outcome::NeedsReview(case) = &receipt.outcome else {
        panic!("never adopt past consumption")
    };
    assert_eq!(case.reason, ReviewReason::PriorConsumptionWithoutAllocation);
    assert!(
        case.footprint
            .original
            .iter()
            .all(|m| m.state == OriginalState::Consumed)
    );
    assert_eq!(balances(&pool).await, before);
    assert_eq!(allocation_counts(&pool).await, (0, 0, 1, 0, 1, 2, 3, 0, 3));
    replay_without_ports(&pool, &f, receipt.outcome).await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn late_stock_shortage_stops_once_without_fresh_rows_and_restock_cannot_retry_it(
    pool: PgPool,
) {
    let f = accepted(&pool, false, true).await;
    expire_original(&pool, &f).await;
    receive(
        &pool,
        "actual-foam-shortage",
        "SEAT-FOAM",
        WarehouseChange::Count(2),
    )
    .await;
    let before = balances(&pool).await;
    assert_eq!(
        before,
        vec![
            ("SEAT-COVER".into(), 10, 1, 0),
            ("SEAT-FOAM".into(), 2, 1, 0)
        ]
    );
    let receipt = recover(&pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
        .await
        .expect("immutable scarce assessment");
    let Outcome::NeedsReview(case) = &receipt.outcome else {
        panic!("whole demand is scarce")
    };
    assert_eq!(case.reason, ReviewReason::ComponentScarcity);
    assert_eq!(balances(&pool).await, before);
    assert_eq!(allocation_counts(&pool).await, (0, 0, 1, 0, 1, 2, 0, 3, 3));
    receive(
        &pool,
        "scarcity-resolved-later",
        "SEAT-FOAM",
        WarehouseChange::Receive(20),
    )
    .await;
    replay_without_ports(&pool, &f, receipt.outcome).await;
    assert_eq!(allocation_counts(&pool).await, (0, 0, 1, 0, 1, 2, 0, 3, 3));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn late_stock_bad_request_absent_order_clock_and_entropy_fail_without_partial_custody(
    pool: PgPool,
) {
    let f = accepted(&pool, false, true).await;
    f.time.0.store(2000, Ordering::SeqCst);
    let before = all_effects(&pool).await;
    for invalid in ["", "bad"] {
        assert_eq!(
            recover(&pool, f.time.as_ref(), f.ids.as_ref(), invalid)
                .await
                .err(),
            Some(LateStockError::InvalidRequest)
        );
        assert_eq!(
            read_recorded(&pool, invalid).await.err(),
            Some(LateStockError::InvalidRequest)
        );
    }
    assert_eq!(
        recover(&pool, f.time.as_ref(), f.ids.as_ref(), &"f".repeat(32))
            .await
            .err(),
        Some(LateStockError::NotFound)
    );
    let bad = &super::super::observation_cases::BadPorts;
    assert_eq!(
        recover(&pool, bad, f.ids.as_ref(), &f.order.identity.id)
            .await
            .err(),
        Some(LateStockError::ClockUnavailable)
    );
    assert_eq!(
        recover(&pool, f.time.as_ref(), bad, &f.order.identity.id)
            .await
            .err(),
        Some(LateStockError::EntropyUnavailable)
    );
    for at in [1499, 9_007_199_254_740_991, 9_007_199_254_740_992, u64::MAX] {
        f.time.0.store(at, Ordering::SeqCst);
        assert_eq!(
            recover(&pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
                .await
                .err(),
            Some(LateStockError::ClockUnavailable)
        );
    }
    assert_eq!(all_effects(&pool).await, before);
}
