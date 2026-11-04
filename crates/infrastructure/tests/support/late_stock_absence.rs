//! A genuine intervening commit cannot rewrite a captured absence into corruption.
use super::*;
#[sqlx::test(migrator = "MIGRATOR")]
async fn late_stock_original_reader_captured_absence_survives_concurrent_healthy_settlement_commit(
    pool: PgPool,
) {
    let f = accepted(&pool, false, true).await;
    let mut tx = pool
        .begin()
        .await
        .expect("explicit isolated reader transaction");
    tx.execute("SET TRANSACTION ISOLATION LEVEL READ COMMITTED")
        .await
        .expect("defaultRC statement snapshots");
    let gate =
        crate::order_stock::rows::absence_hook::register(&mut tx, &f.order.identity.id).await;
    let order = f.order.identity.id.clone();
    let reader = tokio::spawn(async move {
        let result = crate::order_stock::rows::load(&mut tx, &order).await;
        tx.rollback().await.expect("reader rollback");
        result
    });
    tokio::time::timeout(Duration::from_secs(3), gate.reached.notified())
        .await
        .expect("reader captured absence before writer starts");
    let receipt = crate::order_stock::settle(
        &pool,
        f.time.as_ref(),
        f.ids.as_ref(),
        &f.order.identity.id,
    )
    .await
    .expect("different pool backend commits complete real settlement while capturedreader paused");
    assert!(!receipt.replayed);
    assert_eq!(allocation_counts(&pool).await, (1, 0, 0, 1, 1, 2, 3, 0, 3));
    gate.release.notify_one();
    assert!(
        reader
            .await
            .expect("paused reader task")
            .expect("healthy concurrent commit is not orphan corruption")
            .is_none()
    );
    assert!(
        crate::order_stock::read_settlement(&pool, &f.order.identity.id)
            .await
            .expect("later statement sees healthy immutable receipt")
            == Some(receipt.settlement.clone())
    );
    replay_without_ports(&pool, &f, Outcome::Original(receipt.settlement)).await;
}
