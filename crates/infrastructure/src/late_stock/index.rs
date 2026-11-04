//! Atomic bounded index snapshots for recovery custody.
use super::*;
#[derive(sqlx::FromRow)]
pub(super) struct Index {
    pub(super) attempt_id: Option<String>,
    pub(super) kind: Option<String>,
    pub(super) receipt_id: Option<String>,
    pub(super) review: bool,
    pub(super) review_id: Option<String>,
    pub(super) review_attempt_id: Option<String>,
    pub(super) original_id: Option<String>,
    pub(super) late_id: Option<String>,
    pub(super) original_event: bool,
    pub(super) late_event: bool,
    pub(super) review_event: bool,
    pub(super) decisions: i64,
    pub(super) decision_events: i64,
    pub(super) orphan: bool,
}
// One statement snapshot prevents a normal concurrent commit from looking orphaned.
pub(super) async fn load_index(
    tx: &mut Transaction<'_, Postgres>,
    order: &str,
) -> Result<Index, LateStockError> {
    sqlx::query_as::<_, Index>("SELECT (SELECT left(attempt_id,33) FROM order_stock_allocations WHERE order_id=$1) AS attempt_id,(SELECT left(kind,10) FROM order_stock_allocations WHERE order_id=$1) AS kind,(SELECT left(receipt_id,33) FROM order_stock_allocations WHERE order_id=$1) AS receipt_id,(SELECT left(id,33) FROM order_stock_settlements WHERE order_id=$1) AS original_id,(SELECT left(id,33) FROM late_stock_allocations WHERE order_id=$1) AS late_id,EXISTS(SELECT 1 FROM commerce_outbox WHERE aggregate_id=$1 AND kind='order.stock_settled.v1') AS original_event,EXISTS(SELECT 1 FROM commerce_outbox WHERE aggregate_id=$1 AND kind='order.stock_late_allocated.v1') AS late_event,EXISTS(SELECT 1 FROM commerce_outbox WHERE aggregate_id=$1 AND kind='order.stock_recovery_review.v1') AS review_event,EXISTS(SELECT 1 FROM late_stock_review_cases WHERE order_id=$1) AS review,(SELECT left(id,33) FROM late_stock_review_cases WHERE order_id=$1) AS review_id,(SELECT left(attempt_id,33) FROM late_stock_review_cases WHERE order_id=$1) AS review_attempt_id,(SELECT count(*) FROM (SELECT 1 FROM late_stock_dispositions WHERE order_id=$1 LIMIT 4) d) AS decisions,(SELECT count(*) FROM (SELECT 1 FROM commerce_outbox WHERE aggregate_id=$1 AND kind='order.stock_recovery_decided.v1' LIMIT 4) e) AS decision_events,EXISTS(SELECT 1 FROM late_stock_allocations WHERE order_id=$1) OR EXISTS(SELECT 1 FROM order_stock_settlements WHERE order_id=$1) OR EXISTS(SELECT 1 FROM commerce_outbox WHERE aggregate_id=$1 AND kind IN ('order.stock_settled.v1','order.stock_late_allocated.v1','order.stock_recovery_review.v1')) AS orphan")
        .bind(order).fetch_one(&mut **tx).await.map_err(Into::into)
}
