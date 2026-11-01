//! The caller-owned expiry phase requires a fresh active source, never adoption.
use super::*;
#[sqlx::test(migrator = "MIGRATOR")]
async fn late_stock_claimed_expiry_rejects_a_replayed_terminal_and_rollback_restores_the_same_live_source(
    pool: PgPool,
) {
    let f = accepted(&pool, false, true).await;
    f.time.0.store(2000, Ordering::SeqCst);
    let before = all_effects(&pool).await;
    let holds = original_holds(&pool, &f).await;
    let mut tx = pool
        .begin()
        .await
        .expect("caller owns all original and component claims");
    sqlx::query("SELECT id FROM stock_holds WHERE basket_group_id=$1 ORDER BY id FOR UPDATE")
        .bind(&f.order.identity.stock_group_id)
        .execute(&mut *tx)
        .await
        .expect("all original hold claims before components");
    tx.execute("SELECT component_code FROM stock_balances ORDER BY component_code FOR UPDATE")
        .await
        .expect("union component claims");
    crate::holds::expire_claimed(&mut tx, &holds[0], 2000)
        .await
        .expect("one fresh active original expires");
    assert!(
        matches!(
            crate::holds::expire_claimed(&mut tx, &holds[0], 2000).await,
            Err(crate::holds::HoldWriteError::InvalidRequest)
        ),
        "claimed phase cannot adopt an earlier terminal replay"
    );
    let inside:Vec<(String,i64,i64,i64)>=sqlx::query_as("SELECT component_code,on_hand,quarantine,reserved FROM stock_balances ORDER BY component_code").fetch_all(&mut *tx).await.expect("exact single member effect");
    assert_eq!(
        inside,
        vec![
            ("SEAT-COVER".into(), 10, 1, 2),
            ("SEAT-FOAM".into(), 4, 1, 2)
        ]
    );
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM stock_movements WHERE action='expire-hold'")
            .fetch_one(&mut *tx)
            .await
            .expect("one canonical expiry movement");
    assert_eq!(count, 1);
    tx.rollback().await.expect("whole caller-owned rollback");
    assert_eq!(all_effects(&pool).await, before);
    assert!(
        !recover(&pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
            .await
            .expect("identical healthy whole recovery after rollback")
            .replayed
    );
}
