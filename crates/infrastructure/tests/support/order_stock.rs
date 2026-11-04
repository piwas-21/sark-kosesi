//! Actual order, paid recognition and original group settlement; no provider calls.
use super::*;
use crate::{
    MIGRATOR,
    order_stock::*,
    payment_attempts::{
        identities::resolve, observations::record_verified, recognition::recognize,
    },
};
use sqlx::{Executor, PgPool};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
#[path = "order_stock_fixture.rs"]
pub(crate) mod fixture;
use fixture::*;

#[sqlx::test(migrator = "MIGRATOR")]
async fn original_two_member_paid_order_consumes_shared_cover_and_foam_atomically_once(
    pool: PgPool,
) {
    let f = accepted(&pool, false, true).await;
    let source = unchanged(&pool).await;
    assert_eq!(f.order.quote.total.gross_minor, 25900);
    assert_eq!(
        balances(&pool).await,
        vec![
            ("SEAT-COVER".into(), 10, 1, 4),
            ("SEAT-FOAM".into(), 4, 1, 2)
        ]
    );
    let receipt = settle(&pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
        .await
        .expect("whole original group");
    assert!(!receipt.replayed);
    assert_eq!(receipt.settlement.actor, "payment-settlement");
    assert_eq!(receipt.settlement.order_id, f.order.identity.id);
    assert_eq!(
        receipt.settlement.attempt_id,
        f.frozen.snapshot().binding.id
    );
    assert_eq!(receipt.settlement.group_id, f.order.identity.stock_group_id);
    assert_eq!(
        balances(&pool).await,
        vec![
            ("SEAT-COVER".into(), 6, 1, 0),
            ("SEAT-FOAM".into(), 2, 1, 0)
        ]
    );
    assert_eq!(counts(&pool).await, (1, 1, 3, 1, 2));
    assert_eq!(unchanged(&pool).await, source);
    let states: Vec<String> = sqlx::query_scalar("SELECT state FROM stock_holds ORDER BY id")
        .fetch_all(&pool)
        .await
        .expect("both original holds");
    assert_eq!(states, vec!["consumed", "consumed"]);
    let moved:Vec<(String,i64,i64,i64,String,String)>=sqlx::query_as("SELECT component_code,quantity,on_hand_delta,reserved_delta,actor,reason FROM stock_movements WHERE action='consume-hold' ORDER BY hold_id,component_code").fetch_all(&pool).await.expect("exact member movements");
    assert_eq!(moved.len(), 3);
    assert_eq!(
        moved
            .iter()
            .map(|m| (m.0.as_str(), m.1, m.2, m.3))
            .collect::<Vec<_>>(),
        vec![
            ("SEAT-COVER", 2, -2, -2),
            ("SEAT-COVER", 2, -2, -2),
            ("SEAT-FOAM", 2, -2, -2)
        ]
    );
    assert!(
        moved
            .iter()
            .all(|m| m.4 == "payment-settlement" && m.5 == "sealed BOM hold transition")
    );
    let replay = settle(
        &pool,
        &super::observation_cases::BadPorts,
        &super::observation_cases::BadPorts,
        &f.order.identity.id,
    )
    .await
    .expect("canonical historical receipt without ports");
    assert!(replay.replayed && replay.settlement == receipt.settlement);
    assert!(
        read_settlement(&pool, &f.order.identity.id)
            .await
            .expect("historical read")
            == Some(receipt.settlement)
    );
    assert_eq!(counts(&pool).await, (1, 1, 3, 1, 2));
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn covers_only_paid_order_never_consumes_foam_and_historical_replay_allows_later_warehouse_changes(
    pool: PgPool,
) {
    let f = accepted(&pool, true, true).await;
    let receipt = settle(&pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
        .await
        .expect("covers only")
        .settlement;
    assert_eq!(
        balances(&pool).await,
        vec![
            ("SEAT-COVER".into(), 8, 1, 0),
            ("SEAT-FOAM".into(), 4, 1, 0)
        ]
    );
    assert_eq!(counts(&pool).await, (1, 1, 1, 1, 1));
    receive(
        &pool,
        "legitimate-later-cover",
        "SEAT-COVER",
        commerce_domain::inventory::WarehouseChange::Receive(7),
    )
    .await;
    receive(
        &pool,
        "legitimate-later-foam",
        "SEAT-FOAM",
        commerce_domain::inventory::WarehouseChange::Receive(3),
    )
    .await;
    observe(&pool, &f, "tr_AMBIGUOUS", "open", "later-ambiguity").await;
    recognize(
        &pool,
        f.time.as_ref(),
        f.ids.as_ref(),
        &f.frozen.snapshot().binding.id,
    )
    .await
    .expect("later current needsreview");
    let after = footprint(&pool).await;
    let replay = settle(
        &pool,
        &super::observation_cases::BadPorts,
        &super::observation_cases::BadPorts,
        &f.order.identity.id,
    )
    .await
    .expect("immutable history despite ambiguity and stockchanges");
    assert!(replay.replayed && replay.settlement == receipt);
    assert!(
        read_settlement(&pool, &f.order.identity.id)
            .await
            .expect("historical source footprint")
            == Some(receipt)
    );
    assert_eq!(footprint(&pool).await, after);
}

#[path = "order_stock_states.rs"]
mod states;

#[path = "order_stock_waits.rs"]
mod waits;

#[path = "order_stock_native.rs"]
pub(crate) mod native;

#[path = "order_stock_corruption.rs"]
mod corruption;

#[sqlx::test(migrations = false)]
async fn schema31_upgrade_preserves_received_original_order_and_reservation_without_implicit_consumption(
    pool: PgPool,
) {
    MIGRATOR.run_to(31, &pool).await.expect("accepted schema31");
    let f = accepted(&pool, false, true).await;
    let source = unchanged(&pool).await;
    let balance = balances(&pool).await;
    let recognition =
        crate::payment_attempts::recognition::current(&pool, &f.frozen.snapshot().binding.id)
            .await
            .expect("prior canonical recognition");
    MIGRATOR.run(&pool).await.expect("upgrade32");
    MIGRATOR.run(&pool).await.expect("idempotent upgrade32");
    assert_eq!(unchanged(&pool).await, source);
    assert_eq!(balances(&pool).await, balance);
    assert_eq!(counts(&pool).await, (0, 0, 0, 1, 2));
    assert!(
        crate::payment_attempts::recognition::current(&pool, &f.frozen.snapshot().binding.id)
            .await
            .expect("preserved paid recognition")
            == recognition
    );
    assert!(
        !settle(&pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
            .await
            .expect("explicit postupgrade settlement")
            .replayed
    );
    assert_eq!(counts(&pool).await, (1, 1, 3, 1, 2));
}

#[path = "order_stock_boundary.rs"]
mod boundary;
