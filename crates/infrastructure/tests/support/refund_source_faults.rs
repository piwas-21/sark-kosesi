//! Exercise the refund adapter against genuine payment evidence and PostgreSQL faults.
use super::*;
use crate::{MIGRATOR, mollie::tests::order_stock_cases::fixture as orders};
use sqlx::Executor;

#[sqlx::test(migrator = "MIGRATOR")]
async fn refund_source_rejects_missing_or_broken_received_evidence(pool: PgPool) {
    let fixture = orders::accepted(&pool, false, true).await;
    let attempt = &fixture.frozen.snapshot().binding.id;
    let order = &fixture.order.identity.id;
    let stock = orders::footprint(&pool).await;
    let mut tx = pool.begin().await.expect("initial source read");
    let (_, received) = source(&mut tx, attempt, None).await.expect("current proof");
    assert_eq!(&received.order_id, order);
    assert_eq!(received.amount_minor, 25900);
    let (_, historical) = source(&mut tx, attempt, Some(&received.id))
        .await
        .expect("exact historical proof");
    assert!(historical == received);
    let missing = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
    assert_ne!(received.id, missing);
    assert_ne!(attempt, missing);
    assert!(matches!(
        source(&mut tx, attempt, Some(missing)).await,
        Err(RefundError::InvalidStoredData)
    ));
    assert!(matches!(
        source(&mut tx, missing, None).await,
        Err(RefundError::InvalidStoredData)
    ));
    tx.rollback().await.expect("end healthy read transaction");

    pool.execute(
        "ALTER TABLE payment_recognition_current RENAME TO unavailable_refund_recognition",
    )
    .await
    .expect("interrupt current recognition storage");
    let mut tx = pool.begin().await.expect("failed source read");
    assert!(matches!(
        source(&mut tx, attempt, None).await,
        Err(RefundError::Database)
    ));
    tx.rollback().await.expect("rollback after SQL failure");
    pool.execute(
        "ALTER TABLE unavailable_refund_recognition RENAME TO payment_recognition_current",
    )
    .await
    .expect("restore current recognition storage");
    let mut tx = pool.begin().await.expect("verify recovery");
    let (_, restored) = source(&mut tx, attempt, None)
        .await
        .expect("healthy control");
    assert!(restored == received);
    tx.rollback().await.expect("read-only rollback");
    assert_eq!(orders::footprint(&pool).await, stock);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn refund_source_requires_current_observation_watermark(pool: PgPool) {
    let fixture = orders::accepted(&pool, false, true).await;
    let attempt = &fixture.frozen.snapshot().binding.id;
    let mut tx = pool.begin().await.expect("original source");
    let (_, received) = source(&mut tx, attempt, None).await.expect("current proof");
    tx.rollback().await.expect("finish original read");

    let observation = orders::verified(&fixture.frozen, "tr_SETTLEMENT", "paid").await;
    crate::payment_attempts::observations::record_verified(
        &pool,
        fixture.time.as_ref(),
        fixture.ids.as_ref(),
        attempt,
        "refund-watermark-changed",
        &observation,
    )
    .await
    .expect("new verified observation");
    let mut tx = pool.begin().await.expect("stale source");
    assert!(matches!(
        source(&mut tx, attempt, None).await,
        Err(RefundError::Unavailable)
    ));
    let (_, historical) = source(&mut tx, attempt, Some(&received.id))
        .await
        .expect("prior exact proof remains historical");
    assert!(historical == received);
    tx.rollback().await.expect("read-only rollback");
}
