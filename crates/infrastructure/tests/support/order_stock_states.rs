//! Closed eligibility/recovery outcomes never mint a stock receipt or effects.
use super::super::observation_cases::BadPorts;
use super::*;
use crate::holds::{HoldCommand, transition_hold};
use crate::payment_attempts::recognition::current;

#[sqlx::test(migrator = "MIGRATOR")]
async fn stock_settlement_rejects_unconfirmed_stale_and_ambiguous_recognition_without_effects(
    pool: PgPool,
) {
    let f = accepted(&pool, false, false).await;
    let order = &f.order.identity.id;
    let attempt = &f.frozen.snapshot().binding.id;
    let before = footprint(&pool).await;
    assert!(
        read_settlement(&pool, order)
            .await
            .expect("no receipt")
            .is_none()
    );
    assert_eq!(
        settle(&pool, f.time.as_ref(), f.ids.as_ref(), order)
            .await
            .err(),
        Some(SettlementError::Unavailable)
    );
    resolve(&pool, f.time.as_ref(), f.ids.as_ref(), attempt, "zero")
        .await
        .expect("zero complete resolution");
    recognize(&pool, f.time.as_ref(), f.ids.as_ref(), attempt)
        .await
        .expect("unconfirmed");
    assert_eq!(
        settle(&pool, f.time.as_ref(), f.ids.as_ref(), order)
            .await
            .err(),
        Some(SettlementError::Unavailable)
    );
    observe(&pool, &f, "tr_SETTLEMENT", "paid", "paid").await;
    assert!(current(&pool, attempt).await.is_err());
    assert_eq!(
        settle(&pool, f.time.as_ref(), f.ids.as_ref(), order)
            .await
            .err(),
        Some(SettlementError::Unavailable)
    );
    recognize(&pool, f.time.as_ref(), f.ids.as_ref(), attempt)
        .await
        .expect("received");
    observe(&pool, &f, "tr_OTHER", "open", "other").await;
    recognize(&pool, f.time.as_ref(), f.ids.as_ref(), attempt)
        .await
        .expect("needs review");
    assert_eq!(
        settle(&pool, f.time.as_ref(), f.ids.as_ref(), order)
            .await
            .err(),
        Some(SettlementError::Unavailable)
    );
    assert_eq!(footprint(&pool).await, before);
    assert_eq!(counts(&pool).await, (0, 0, 0, 1, 2));
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn stock_settlement_intact_expired_and_released_mixed_original_groups_require_separate_recovery(
    pool: PgPool,
) {
    let f = accepted(&pool, false, true).await;
    let order = &f.order.identity.id;
    let before = footprint(&pool).await;
    f.time.0.store(2000, Ordering::SeqCst);
    assert_eq!(
        settle(&pool, f.time.as_ref(), f.ids.as_ref(), order)
            .await
            .err(),
        Some(SettlementError::RecoveryRequired)
    );
    assert_eq!(footprint(&pool).await, before);
    let holds: Vec<String> = sqlx::query_scalar("SELECT id FROM stock_holds ORDER BY id")
        .fetch_all(&pool)
        .await
        .expect("complete original members");
    transition_hold(
        &pool,
        &holds[0],
        HoldCommand::Release {
            owner_id: order,
            actor_id: "fixture-release",
        },
        1500,
    )
    .await
    .expect("first complete release");
    let mixed_active = footprint(&pool).await;
    assert_eq!(
        settle(&pool, f.time.as_ref(), f.ids.as_ref(), order)
            .await
            .err(),
        Some(SettlementError::RecoveryRequired)
    );
    assert_eq!(footprint(&pool).await, mixed_active);
    transition_hold(
        &pool,
        &holds[1],
        HoldCommand::Expire {
            actor_id: "fixture-expiry",
        },
        2000,
    )
    .await
    .expect("second complete expiry");
    let mixed_terminal = footprint(&pool).await;
    assert_eq!(
        settle(&pool, f.time.as_ref(), f.ids.as_ref(), order)
            .await
            .err(),
        Some(SettlementError::RecoveryRequired)
    );
    assert_eq!(footprint(&pool).await, mixed_terminal);
    assert_eq!(counts(&pool).await, (0, 0, 0, 1, 2));
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn stock_settlement_request_clock_entropy_and_absent_order_fail_closed(pool: PgPool) {
    let f = accepted(&pool, false, true).await;
    let order = &f.order.identity.id;
    let before = footprint(&pool).await;
    for invalid in ["", "bad"] {
        assert_eq!(
            settle(&pool, f.time.as_ref(), f.ids.as_ref(), invalid)
                .await
                .err(),
            Some(SettlementError::InvalidRequest)
        );
        assert_eq!(
            read_settlement(&pool, invalid).await.err(),
            Some(SettlementError::InvalidRequest)
        );
    }
    assert_eq!(
        settle(&pool, f.time.as_ref(), f.ids.as_ref(), &"f".repeat(32))
            .await
            .err(),
        Some(SettlementError::NotFound)
    );
    assert_eq!(
        settle(&pool, &BadPorts, f.ids.as_ref(), order).await.err(),
        Some(SettlementError::ClockUnavailable)
    );
    assert_eq!(
        settle(&pool, f.time.as_ref(), &BadPorts, order).await.err(),
        Some(SettlementError::EntropyUnavailable)
    );
    for at in [1499, 9_007_199_254_740_992, u64::MAX] {
        f.time.0.store(at, Ordering::SeqCst);
        assert_eq!(
            settle(&pool, f.time.as_ref(), f.ids.as_ref(), order)
                .await
                .err(),
            Some(SettlementError::ClockUnavailable)
        );
    }
    assert_eq!(footprint(&pool).await, before);
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn concurrent_stock_settlement_has_one_canonical_receipt_and_one_complete_consumption(
    pool: PgPool,
) {
    let f = accepted(&pool, false, true).await;
    let mut tasks = vec![];
    for _ in 0..4 {
        let pool = pool.clone();
        let time = f.time.clone();
        let ids = f.ids.clone();
        let order = f.order.identity.id.clone();
        tasks.push(tokio::spawn(async move {
            settle(&pool, time.as_ref(), ids.as_ref(), &order).await
        }));
    }
    let mut original = None;
    let mut replays = 0;
    for task in tasks {
        let receipt = task
            .await
            .expect("settlement task")
            .expect("canonical serialized settlement");
        replays += usize::from(receipt.replayed);
        if let Some(ref value) = original {
            assert!(value == &receipt.settlement);
        }
        original = Some(receipt.settlement);
    }
    assert_eq!(replays, 3);
    assert_eq!(counts(&pool).await, (1, 1, 3, 1, 2));
    assert_eq!(
        balances(&pool).await,
        vec![
            ("SEAT-COVER".into(), 6, 1, 0),
            ("SEAT-FOAM".into(), 2, 1, 0)
        ]
    );
}
