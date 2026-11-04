//! Independent literal database oracles; never derive expected values from recovery.
use super::*;
pub async fn original_snapshot(pool: &PgPool) -> Value {
    sqlx::query_scalar("SELECT jsonb_build_object('orders',(SELECT jsonb_agg(to_jsonb(o) ORDER BY id) FROM checkout_orders o),'attempts',(SELECT jsonb_agg(to_jsonb(a) ORDER BY id) FROM payment_attempts a),'groups',(SELECT jsonb_agg(to_jsonb(g) ORDER BY id) FROM basket_hold_groups g WHERE id IN(SELECT stock_group_id FROM checkout_orders)),'members',(SELECT jsonb_agg(to_jsonb(m) ORDER BY hold_id) FROM basket_hold_members m WHERE group_id IN(SELECT stock_group_id FROM checkout_orders)),'lines',(SELECT jsonb_agg(to_jsonb(l) ORDER BY hold_id,component_code) FROM stock_hold_lines l WHERE hold_id IN(SELECT id FROM stock_holds WHERE basket_group_id IN(SELECT stock_group_id FROM checkout_orders))))").fetch_one(pool).await.expect("immutable original custody")
}
pub async fn all_effects(pool: &PgPool) -> Value {
    sqlx::query_scalar("SELECT jsonb_build_object('balances',(SELECT jsonb_agg(to_jsonb(b) ORDER BY component_code) FROM stock_balances b),'holds',(SELECT jsonb_agg(to_jsonb(h) ORDER BY id) FROM stock_holds h),'groups',(SELECT jsonb_agg(to_jsonb(g) ORDER BY id) FROM basket_hold_groups g),'members',(SELECT jsonb_agg(to_jsonb(m) ORDER BY hold_id) FROM basket_hold_members m),'lines',(SELECT jsonb_agg(to_jsonb(l) ORDER BY hold_id,component_code) FROM stock_hold_lines l),'moves',(SELECT jsonb_agg(to_jsonb(m) ORDER BY sequence) FROM stock_movements m),'claims',(SELECT jsonb_agg(to_jsonb(a) ORDER BY order_id) FROM order_stock_allocations a),'late',(SELECT jsonb_agg(to_jsonb(a) ORDER BY id) FROM late_stock_allocations a),'cases',(SELECT jsonb_agg(to_jsonb(a) ORDER BY id) FROM late_stock_review_cases a),'original',(SELECT jsonb_agg(to_jsonb(a) ORDER BY id) FROM order_stock_settlements a),'events',(SELECT jsonb_agg(to_jsonb(e) ORDER BY id) FROM commerce_outbox e))").fetch_one(pool).await.expect("entire physical and durable journal state")
}
pub async fn allocation_counts(pool: &PgPool) -> (i64, i64, i64, i64, i64, i64, i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM order_stock_allocations),(SELECT count(*) FROM late_stock_allocations),(SELECT count(*) FROM late_stock_review_cases),(SELECT count(*) FROM order_stock_settlements),(SELECT count(*) FROM basket_hold_groups),(SELECT count(*) FROM stock_holds),(SELECT count(*) FROM stock_movements WHERE action='consume-hold'),(SELECT count(*) FROM stock_movements WHERE action='expire-hold'),(SELECT count(*) FROM stock_movements WHERE action='reserve')").fetch_one(pool).await.expect("literal canonical ownership and movement counts")
}
pub async fn original_holds(pool: &PgPool, f: &Fixture) -> Vec<String> {
    sqlx::query_scalar("SELECT id FROM stock_holds WHERE basket_group_id=$1 ORDER BY id")
        .bind(&f.order.identity.stock_group_id)
        .fetch_all(pool)
        .await
        .expect("original member ordering")
}
pub async fn expire_original(pool: &PgPool, f: &Fixture) {
    f.time.0.store(2000, Ordering::SeqCst);
    for hold in original_holds(pool, f).await {
        transition_hold(
            pool,
            &hold,
            HoldCommand::Expire {
                actor_id: "fixture-original-expiry",
            },
            2000,
        )
        .await
        .expect("genuine complete original expiry");
    }
}
pub async fn replay_without_ports(pool: &PgPool, f: &Fixture, expected: Outcome) {
    let before = all_effects(pool).await;
    let bad = &super::super::observation_cases::BadPorts;
    let replay = recover(pool, bad, bad, &f.order.identity.id)
        .await
        .expect("historical replay needs no time or entropy");
    assert!(replay.replayed);
    assert!(replay.outcome == expected);
    assert!(
        read_recorded(pool, &f.order.identity.id)
            .await
            .expect("independent history reader")
            == Some(expected)
    );
    assert_eq!(all_effects(pool).await, before);
}
