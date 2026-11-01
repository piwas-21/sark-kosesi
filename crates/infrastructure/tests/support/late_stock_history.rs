//! One cross-path allocation; old history does not acquire fresh financial authority.
use super::*;

#[sqlx::test(migrator = "MIGRATOR")]
async fn late_stock_recorded_original_receipt_wins_before_clock_and_never_reallocates_after_ambiguity(
    pool: PgPool,
) {
    let f = accepted(&pool, false, true).await;
    let original =
        crate::order_stock::settle(&pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
            .await
            .expect("canonical original allocation")
            .settlement;
    assert_eq!(allocation_counts(&pool).await, (1, 0, 0, 1, 1, 2, 3, 0, 3));
    f.time.0.store(2000, Ordering::SeqCst);
    observe(&pool, &f, "tr_HISTORY_AMBIGUITY", "open", "later-ambiguity").await;
    recognize(
        &pool,
        f.time.as_ref(),
        f.ids.as_ref(),
        &f.frozen.snapshot().binding.id,
    )
    .await
    .expect("later needs review");
    receive(
        &pool,
        "warehouse-after-original",
        "SEAT-COVER",
        WarehouseChange::Receive(11),
    )
    .await;
    replay_without_ports(&pool, &f, Outcome::Original(original)).await;
    assert_eq!(allocation_counts(&pool).await, (1, 0, 0, 1, 1, 2, 3, 0, 3));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn late_stock_successful_late_history_survives_new_stock_and_finance_without_reconsume(
    pool: PgPool,
) {
    let f = accepted(&pool, false, true).await;
    expire_original(&pool, &f).await;
    let receipt = recover(&pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
        .await
        .expect("late allocation");
    assert!(matches!(receipt.outcome, Outcome::Late(_)));
    receive(
        &pool,
        "after-late-cover",
        "SEAT-COVER",
        WarehouseChange::Receive(7),
    )
    .await;
    receive(
        &pool,
        "after-late-foam",
        "SEAT-FOAM",
        WarehouseChange::Receive(3),
    )
    .await;
    observe(
        &pool,
        &f,
        "tr_LATE_HISTORY_OTHER",
        "paid",
        "later-paid-ambiguity",
    )
    .await;
    recognize(
        &pool,
        f.time.as_ref(),
        f.ids.as_ref(),
        &f.frozen.snapshot().binding.id,
    )
    .await
    .expect("new ambiguity");
    replay_without_ports(&pool, &f, receipt.outcome).await;
    assert_eq!(
        balances(&pool).await,
        vec![
            ("SEAT-COVER".into(), 13, 1, 0),
            ("SEAT-FOAM".into(), 5, 1, 0)
        ]
    );
    assert_eq!(allocation_counts(&pool).await, (1, 1, 0, 0, 2, 4, 3, 3, 6));
}

#[sqlx::test(migrations = false)]
async fn late_stock_schema32_backfill_retains_original_allocation_after_new_ambiguity_without_rewriting_old_receipt(
    pool: PgPool,
) {
    MIGRATOR
        .run_to(32, &pool)
        .await
        .expect("accepted populated schema32");
    let f = accepted(&pool, false, true).await;
    // New application readers need schema33's shared claim; preserve a genuine
    // schema32 receipt through the existing independent native physical fixture.
    use super::super::order_stock_cases::native as prior;
    let original = prior::candidate(&pool, &f).await;
    let mut tx = pool
        .begin()
        .await
        .expect("actual schema32 settlement transaction");
    prior::insert(&mut tx, &original)
        .await
        .expect("schema32 prepared receipt");
    prior::effects(&mut tx, &original, prior::Fault::Healthy).await;
    prior::event(&mut tx, &original)
        .await
        .expect("schema32 exact digest event");
    tx.commit()
        .await
        .expect("genuine prior physical settlement committed");
    f.time.0.store(2000, Ordering::SeqCst);
    observe(&pool, &f, "tr_POST32_OTHER", "open", "post32-ambiguity").await;
    recognize(
        &pool,
        f.time.as_ref(),
        f.ids.as_ref(),
        &f.frozen.snapshot().binding.id,
    )
    .await
    .expect("current ambiguity before upgrade");
    let old:Value=sqlx::query_scalar("SELECT jsonb_build_object('receipts',(SELECT jsonb_agg(to_jsonb(s) ORDER BY id) FROM order_stock_settlements s),'events',(SELECT jsonb_agg(to_jsonb(e) ORDER BY id) FROM commerce_outbox e),'moves',(SELECT jsonb_agg(to_jsonb(m) ORDER BY sequence) FROM stock_movements m),'balances',(SELECT jsonb_agg(to_jsonb(b) ORDER BY component_code) FROM stock_balances b))").fetch_one(&pool).await.expect("all prior immutable data");
    MIGRATOR
        .run(&pool)
        .await
        .expect("schema33 explicit backfill");
    MIGRATOR.run(&pool).await.expect("idempotent migration");
    let after:Value=sqlx::query_scalar("SELECT jsonb_build_object('receipts',(SELECT jsonb_agg(to_jsonb(s) ORDER BY id) FROM order_stock_settlements s),'events',(SELECT jsonb_agg(to_jsonb(e) ORDER BY id) FROM commerce_outbox e),'moves',(SELECT jsonb_agg(to_jsonb(m) ORDER BY sequence) FROM stock_movements m),'balances',(SELECT jsonb_agg(to_jsonb(b) ORDER BY component_code) FROM stock_balances b))").fetch_one(&pool).await.expect("all prior immutable data");
    assert_eq!(after, old);
    replay_without_ports(&pool, &f, Outcome::Original(original.clone())).await;
    assert!(
        crate::order_stock::read_settlement(&pool, &f.order.identity.id)
            .await
            .expect("old reader validates new claim")
            == Some(original)
    );
    assert_eq!(allocation_counts(&pool).await, (1, 0, 0, 1, 1, 2, 3, 0, 3));
}
