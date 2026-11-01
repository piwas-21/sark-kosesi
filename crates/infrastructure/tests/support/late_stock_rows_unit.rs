//! Private stored-body readers and typed envelope limits retain literal boundaries.
use super::*;
use crate::{MIGRATOR, mollie::tests::order_stock_cases::fixture as orders};
use std::sync::atomic::Ordering;
#[sqlx::test(migrator = "MIGRATOR")]
async fn late_allocation_private_absence_then_real_body_and_typed_two_mib_bound(pool: PgPool) {
    let f = orders::accepted(&pool, false, true).await;
    let mut tx = pool.begin().await.expect("private body absence");
    assert!(
        allocation(&mut tx, &f.order.identity.id)
            .await
            .expect("no stored body")
            .is_none()
    );
    tx.rollback().await.expect("absence rollback");
    f.time.0.store(2000, Ordering::SeqCst);
    let Outcome::Late(late) =
        crate::late_stock::recover(&pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
            .await
            .expect("genuine complete allocation")
            .outcome
    else {
        panic!("intact expired-window source allocates")
    };
    let mut tx = pool.begin().await.expect("same private body positive");
    assert!(
        allocation(&mut tx, &f.order.identity.id)
            .await
            .expect("valid stored body")
            == Some(late.clone())
    );
    tx.rollback().await.expect("positive rollback");
    let healthy = envelope(&late.footprint).expect("valid typed envelope");
    assert_eq!(healthy["schema_version"], 1);
    assert_eq!(
        healthy["fresh"]
            .as_array()
            .expect("complete fresh membership")
            .len(),
        2
    );
    let mut oversized = late.footprint.clone();
    oversized.fresh[0].sku = "X".repeat(2_097_153);
    assert_eq!(
        envelope(&oversized).err(),
        Some(LateStockError::InvalidStoredData)
    );
    assert!(
        envelope(&late.footprint).is_ok(),
        "same typed healthy source remains admitted"
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn late_review_private_absence_then_real_body_and_typed_two_mib_bound(pool: PgPool) {
    let f = orders::accepted(&pool, false, true).await;
    let mut tx = pool.begin().await.expect("private case absence");
    assert!(
        review_case(&mut tx, &f.order.identity.id)
            .await
            .expect("no case body")
            .is_none()
    );
    tx.rollback().await.expect("absence rollback");
    let hold: String =
        sqlx::query_scalar("SELECT min(id) FROM stock_holds WHERE basket_group_id=$1")
            .bind(&f.order.identity.stock_group_id)
            .fetch_one(&pool)
            .await
            .expect("original first hold");
    crate::holds::transition_hold(
        &pool,
        &hold,
        crate::holds::HoldCommand::Release {
            owner_id: &f.order.identity.id,
            actor_id: "fixture-release",
        },
        1500,
    )
    .await
    .expect("actual release intent");
    f.time.0.store(2000, Ordering::SeqCst);
    let Outcome::NeedsReview(case) =
        crate::late_stock::recover(&pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
            .await
            .expect("genuine immutable stop")
            .outcome
    else {
        panic!("release grants no allocation")
    };
    let mut tx = pool.begin().await.expect("same private case positive");
    assert!(
        review_case(&mut tx, &f.order.identity.id)
            .await
            .expect("valid case body")
            == Some(case.clone())
    );
    tx.rollback().await.expect("positive rollback");
    let healthy = envelope(&case.footprint).expect("valid typed case envelope");
    assert_eq!(healthy["schema_version"], 1);
    assert_eq!(
        healthy["original"]
            .as_array()
            .expect("original membership")
            .len(),
        2
    );
    let mut oversized = case.footprint.clone();
    oversized.original[0].member.sku = "X".repeat(2_097_153);
    assert_eq!(
        envelope(&oversized).err(),
        Some(LateStockError::InvalidStoredData)
    );
    assert!(
        envelope(&case.footprint).is_ok(),
        "same healthy typed case remains readable"
    );
}
