//! Controlled isolated imports cannot turn malformed custody into recovery/review.
use super::*;
async fn edit(pool: &PgPool, sql: &'static str, values: &[Value]) {
    let mut tx = pool
        .begin()
        .await
        .expect("isolated controlled damaged import");
    tx.execute("ALTER TABLE stock_movements DISABLE TRIGGER USER; ALTER TABLE stock_holds DISABLE TRIGGER USER; ALTER TABLE stock_hold_lines DISABLE TRIGGER USER; ALTER TABLE commerce_outbox DISABLE TRIGGER USER; ALTER TABLE late_stock_allocations DISABLE TRIGGER USER; ALTER TABLE late_stock_review_cases DISABLE TRIGGER USER; ALTER TABLE order_stock_allocations DISABLE TRIGGER USER").await.expect("disable only affected fixture guards");
    for statement in sql
        .split(';')
        .filter(|statement| !statement.trim().is_empty())
    {
        let mut query = sqlx::query(statement);
        if statement.contains("$1") {
            for value in values {
                query = query.bind(value);
            }
        }
        query
            .execute(&mut *tx)
            .await
            .expect("controlled fixture edit");
    }
    tx.execute("SET CONSTRAINTS ALL IMMEDIATE")
        .await
        .expect("flush internal foreign key events before restoring guards");
    tx.execute("ALTER TABLE stock_movements ENABLE TRIGGER USER; ALTER TABLE stock_holds ENABLE TRIGGER USER; ALTER TABLE stock_hold_lines ENABLE TRIGGER USER; ALTER TABLE commerce_outbox ENABLE TRIGGER USER; ALTER TABLE late_stock_allocations ENABLE TRIGGER USER; ALTER TABLE late_stock_review_cases ENABLE TRIGGER USER; ALTER TABLE order_stock_allocations ENABLE TRIGGER USER").await.expect("restore exact fixture guards");
    tx.commit()
        .await
        .expect("make controlled import visible to real reader");
}
async fn reject(pool: &PgPool, f: &Fixture) {
    let bad = &super::super::observation_cases::BadPorts;
    assert_eq!(
        read_recorded(pool, &f.order.identity.id).await.err(),
        Some(LateStockError::InvalidStoredData)
    );
    assert_eq!(
        recover(pool, bad, bad, &f.order.identity.id).await.err(),
        Some(LateStockError::InvalidStoredData)
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn late_stock_historical_claim_source_event_and_member_effect_corruption_fail_closed_with_exact_repairs(
    pool: PgPool,
) {
    let f = accepted(&pool, false, true).await;
    f.time.0.store(2000, Ordering::SeqCst);
    let receipt = recover(&pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
        .await
        .expect("genuine complete late receipt");
    let original = all_effects(&pool).await;
    let claim: Value = sqlx::query_scalar("SELECT to_jsonb(a) FROM order_stock_allocations a")
        .fetch_one(&pool)
        .await
        .expect("exact original claim");
    edit(&pool,"UPDATE order_stock_allocations SET receipt_id=repeat('f',32) WHERE order_id=$1->>'order_id'",std::slice::from_ref(&claim)).await;
    reject(&pool, &f).await;
    edit(&pool,"UPDATE order_stock_allocations SET receipt_id=$1->>'receipt_id' WHERE order_id=$1->>'order_id'",std::slice::from_ref(&claim)).await;
    replay_without_ports(&pool, &f, receipt.outcome.clone()).await;
    edit(
        &pool,
        "DELETE FROM order_stock_allocations WHERE order_id=$1->>'order_id'",
        std::slice::from_ref(&claim),
    )
    .await;
    reject(&pool, &f).await;
    edit(&pool,"INSERT INTO order_stock_allocations SELECT * FROM jsonb_populate_record(NULL::order_stock_allocations,$1)",std::slice::from_ref(&claim)).await;
    replay_without_ports(&pool, &f, receipt.outcome.clone()).await;
    for kind in ["order.stock_late_allocated.v1", "payment.recognized.v1"] {
        let event: Value =
            sqlx::query_scalar("SELECT to_jsonb(e) FROM commerce_outbox e WHERE kind=$1")
                .bind(kind)
                .fetch_one(&pool)
                .await
                .expect("exact historical source event");
        edit(
            &pool,
            "DELETE FROM commerce_outbox WHERE id=$1->>'id'",
            std::slice::from_ref(&event),
        )
        .await;
        reject(&pool, &f).await;
        edit(&pool,"INSERT INTO commerce_outbox SELECT * FROM jsonb_populate_record(NULL::commerce_outbox,$1)",std::slice::from_ref(&event)).await;
        replay_without_ports(&pool, &f, receipt.outcome.clone()).await;
    }
    for action in ["reserve", "consume-hold", "expire-hold"] {
        let movement: Value = sqlx::query_scalar(
            "SELECT to_jsonb(m) FROM stock_movements m WHERE action=$1 ORDER BY sequence LIMIT 1",
        )
        .bind(action)
        .fetch_one(&pool)
        .await
        .expect("actual exact source/effect movement");
        edit(&pool,"UPDATE stock_movements SET actor='forged-import' WHERE sequence=($1->>'sequence')::bigint",std::slice::from_ref(&movement)).await;
        reject(&pool, &f).await;
        edit(&pool,"UPDATE stock_movements SET actor=$1->>'actor' WHERE sequence=($1->>'sequence')::bigint",std::slice::from_ref(&movement)).await;
        replay_without_ports(&pool, &f, receipt.outcome.clone()).await;
    }
    assert_eq!(all_effects(&pool).await, original);
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn late_stock_corrupt_original_reserve_or_missing_frozen_line_is_never_normal_review_or_scarcity(
    pool: PgPool,
) {
    let f = accepted(&pool, false, true).await;
    f.time.0.store(2000, Ordering::SeqCst);
    let original = all_effects(&pool).await;
    let movement:Value=sqlx::query_scalar("SELECT to_jsonb(m) FROM stock_movements m WHERE action='reserve' ORDER BY sequence LIMIT 1").fetch_one(&pool).await.expect("real reserve");
    edit(
        &pool,
        "UPDATE stock_movements SET actor='forged-import' WHERE sequence=($1->>'sequence')::bigint",
        std::slice::from_ref(&movement),
    )
    .await;
    assert_eq!(
        recover(&pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
            .await
            .err(),
        Some(LateStockError::InvalidStoredData)
    );
    edit(
        &pool,
        "UPDATE stock_movements SET actor=$1->>'actor' WHERE sequence=($1->>'sequence')::bigint",
        std::slice::from_ref(&movement),
    )
    .await;
    let candidate = super::native::candidate(&pool, &f).await;
    super::native::healthy_probe(&pool, &candidate).await;
    let line: Value = sqlx::query_scalar(
        "SELECT to_jsonb(l) FROM stock_hold_lines l ORDER BY hold_id,component_code LIMIT 1",
    )
    .fetch_one(&pool)
    .await
    .expect("frozen original line");
    edit(&pool,"DELETE FROM stock_hold_lines WHERE hold_id=$1->>'hold_id' AND component_code=$1->>'component_code'",std::slice::from_ref(&line)).await;
    assert_eq!(
        recover(&pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
            .await
            .err(),
        Some(LateStockError::InvalidStoredData)
    );
    edit(&pool,"INSERT INTO stock_hold_lines SELECT * FROM jsonb_populate_record(NULL::stock_hold_lines,$1)",std::slice::from_ref(&line)).await;
    super::native::healthy_probe(&pool, &candidate).await;
    assert_eq!(all_effects(&pool).await, original);
    assert!(
        !recover(&pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
            .await
            .expect("actual repaired full allocation")
            .replayed
    );
}
#[path = "late_stock_closed_rows.rs"]
mod closed_rows;
#[path = "late_stock_oversize.rs"]
mod oversize;
#[path = "late_stock_rows_postgres.rs"]
mod row_boundaries;

#[sqlx::test(migrator = "MIGRATOR")]
async fn late_stock_missing_physical_balance_is_corruption_not_scarcity_and_exact_repair_allocates(
    pool: PgPool,
) {
    let f = accepted(&pool, false, true).await;
    f.time.0.store(2000, Ordering::SeqCst);
    let before = all_effects(&pool).await;
    let row: Value = sqlx::query_scalar(
        "SELECT to_jsonb(b) FROM stock_balances b WHERE component_code='SEAT-FOAM'",
    )
    .fetch_one(&pool)
    .await
    .expect("genuine physically backed source");
    edit(
        &pool,
        "DELETE FROM stock_balances WHERE component_code=$1->>'component_code'",
        std::slice::from_ref(&row),
    )
    .await;
    assert_eq!(
        recover(&pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
            .await
            .err(),
        Some(LateStockError::InvalidStoredData)
    );
    assert_eq!(allocation_counts(&pool).await, (0, 0, 0, 0, 1, 2, 0, 0, 3));
    edit(
        &pool,
        "INSERT INTO stock_balances SELECT * FROM jsonb_populate_record(NULL::stock_balances,$1)",
        std::slice::from_ref(&row),
    )
    .await;
    assert_eq!(all_effects(&pool).await, before);
    assert!(
        !recover(&pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
            .await
            .expect("identical repaired source allocates")
            .replayed
    );
}
#[path = "late_stock_history_postgres.rs"]
mod history_boundaries;
