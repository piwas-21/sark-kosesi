//! Genuine frozen paid orders, complete replacement custody, and immutable stops.
use super::order_stock_cases::fixture;
use super::*;
use crate::holds::{HoldCommand, transition_hold};
use crate::payment_attempts::{
    identities::resolve, observations::record_verified, recognition::recognize,
};
use crate::{
    MIGRATOR,
    late_stock::{LateStockError, read_recorded, recover},
};
use commerce_domain::{inventory::WarehouseChange, late_stock::*};
use fixture::*;
use sqlx::{Executor, PgPool};
use std::sync::{Arc, atomic::Ordering};

#[path = "late_stock_fixture.rs"]
mod controls;
use controls::*;

#[sqlx::test(migrator = "MIGRATOR")]
async fn late_stock_deadline_active_original_is_expired_replaced_and_consumed_as_one_full_frozen_group(
    pool: PgPool,
) {
    let f = accepted(&pool, false, true).await;
    let source = original_snapshot(&pool).await;
    f.time.0.store(2000, Ordering::SeqCst);
    assert_eq!(
        balances(&pool).await,
        vec![
            ("SEAT-COVER".into(), 10, 1, 4),
            ("SEAT-FOAM".into(), 4, 1, 2)
        ]
    );
    let receipt = recover(&pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
        .await
        .expect("complete late allocation");
    assert!(!receipt.replayed);
    let Outcome::Late(late) = receipt.outcome else {
        panic!("deadline active source must allocate late")
    };
    assert_eq!(late.actor, "late-payment-allocation");
    assert_eq!(late.allocated_at, 2000);
    assert_eq!(late.expires_at, 2060);
    assert_eq!(late.original_group_id, f.order.identity.stock_group_id);
    assert_ne!(late.fresh_group_id, late.original_group_id);
    assert_eq!(late.order_id, f.order.identity.id);
    assert_eq!(late.attempt_id, f.frozen.snapshot().binding.id);
    assert_eq!(late.footprint.original.len(), 2);
    assert_eq!(late.footprint.fresh.len(), 2);
    assert!(
        late.footprint
            .original
            .iter()
            .all(|m| m.state == OriginalState::Active)
    );
    assert_eq!(
        balances(&pool).await,
        vec![
            ("SEAT-COVER".into(), 6, 1, 0),
            ("SEAT-FOAM".into(), 2, 1, 0)
        ]
    );
    assert_eq!(allocation_counts(&pool).await, (1, 1, 0, 0, 2, 4, 3, 3, 6));
    assert_eq!(
        original_snapshot(&pool).await,
        source,
        "immutable original group and snapshots are retained"
    );
    let originals: Vec<String> =
        sqlx::query_scalar("SELECT state FROM stock_holds WHERE basket_group_id=$1 ORDER BY id")
            .bind(&late.original_group_id)
            .fetch_all(&pool)
            .await
            .expect("original terminal members");
    assert_eq!(originals, vec!["expired", "expired"]);
    let fresh: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT state,created_at,expires_at FROM stock_holds WHERE basket_group_id=$1 ORDER BY id",
    )
    .bind(&late.fresh_group_id)
    .fetch_all(&pool)
    .await
    .expect("fresh bounded server window");
    assert_eq!(
        fresh,
        vec![
            ("consumed".into(), 2000, 2060),
            ("consumed".into(), 2000, 2060)
        ]
    );
    let actors:Vec<(String,String)>=sqlx::query_as("SELECT DISTINCT action,actor FROM stock_movements WHERE action IN ('expire-hold','consume-hold') ORDER BY action,actor").fetch_all(&pool).await.expect("server actors");
    assert_eq!(
        actors,
        vec![
            ("consume-hold".into(), "payment-settlement".into()),
            ("expire-hold".into(), "late-payment-expiry".into())
        ]
    );
    assert!(
        read_recorded(&pool, &f.order.identity.id)
            .await
            .expect("closed complete history")
            == Some(Outcome::Late(late))
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn late_stock_expired_original_has_no_repeated_expiry_and_covers_only_never_allocates_foam(
    pool: PgPool,
) {
    let f = accepted(&pool, true, true).await;
    expire_original(&pool, &f).await;
    let expiry_before: Vec<i64> = sqlx::query_scalar(
        "SELECT sequence FROM stock_movements WHERE action='expire-hold' ORDER BY sequence",
    )
    .fetch_all(&pool)
    .await
    .expect("captured original expiry");
    assert_eq!(expiry_before.len(), 1);
    let receipt = recover(&pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
        .await
        .expect("already expired exact covers demand");
    let Outcome::Late(late) = receipt.outcome else {
        panic!("expired source must allocate late")
    };
    assert_eq!(late.footprint.original[0].state, OriginalState::Expired);
    assert_eq!(late.footprint.components.len(), 1);
    assert_eq!(
        balances(&pool).await,
        vec![
            ("SEAT-COVER".into(), 8, 1, 0),
            ("SEAT-FOAM".into(), 4, 1, 0)
        ]
    );
    let expiry_after: Vec<i64> = sqlx::query_scalar(
        "SELECT sequence FROM stock_movements WHERE action='expire-hold' ORDER BY sequence",
    )
    .fetch_all(&pool)
    .await
    .expect("no duplicate expiry");
    assert_eq!(expiry_after, expiry_before);
    let foam:i64=sqlx::query_scalar("SELECT count(*) FROM stock_movements WHERE component_code='SEAT-FOAM' AND action IN ('reserve','consume-hold','expire-hold')").fetch_one(&pool).await.expect("no invented foam demand");
    assert_eq!(foam, 0);
}

#[path = "late_stock_absence.rs"]
mod absence;
#[path = "late_stock_boundary.rs"]
mod boundary;
#[path = "late_stock_bounds.rs"]
mod bounds;
#[path = "late_stock_claimed_expiry.rs"]
mod claimed_expiry;
#[path = "late_stock_corruption.rs"]
mod corruption;
#[path = "late_stock_disposition.rs"]
mod disposition;
#[path = "late_stock_history.rs"]
mod history;
#[path = "late_stock_inbox.rs"]
mod inbox;
#[path = "late_stock_native.rs"]
mod native;
#[path = "late_stock_states.rs"]
mod states;
#[path = "late_stock_waits.rs"]
mod waits;
