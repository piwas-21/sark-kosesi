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
impl Index {
    pub(super) fn require_review_binding(
        &self,
        case_id: &str,
        attempt_id: &str,
    ) -> Result<(), LateStockError> {
        if !self.review
            || self.review_id.as_deref() != Some(case_id)
            || self.review_attempt_id.as_deref() != Some(attempt_id)
        {
            return Err(LateStockError::InvalidStoredData);
        }
        Ok(())
    }

    fn validate(&self) -> Result<(), LateStockError> {
        let valid_review = self.review_id.as_deref().is_some_and(identifier)
            && self.review_attempt_id.as_deref().is_some_and(identifier);
        if (self.review && !valid_review)
            || (!self.review && (self.review_id.is_some() || self.review_attempt_id.is_some()))
            || !(0..=3).contains(&self.decisions)
            || self.decisions != self.decision_events
            || (!self.review && self.decisions != 0)
        {
            return Err(LateStockError::InvalidStoredData);
        }
        match (&self.attempt_id, &self.kind, &self.receipt_id) {
            (Some(attempt), Some(kind), Some(id)) if identifier(attempt) && identifier(id) => {
                let matching_receipt = match kind.as_str() {
                    "original" => {
                        self.original_id.as_ref() == Some(id)
                            && self.late_id.is_none()
                            && !self.late_event
                    }
                    "late" => {
                        self.late_id.as_ref() == Some(id)
                            && self.original_id.is_none()
                            && !self.original_event
                    }
                    _ => false,
                };
                if !matching_receipt
                    || ((self.review || self.review_event)
                        && (kind != "late" || !self.review || !self.review_event))
                {
                    return Err(LateStockError::InvalidStoredData);
                }
            }
            (None, None, None) => {
                if (self.review
                    && (self.original_id.is_some()
                        || self.late_id.is_some()
                        || self.original_event
                        || self.late_event))
                    || (!self.review && self.orphan)
                {
                    return Err(LateStockError::InvalidStoredData);
                }
            }
            _ => return Err(LateStockError::InvalidStoredData),
        }
        Ok(())
    }
}
// One statement snapshot prevents a normal concurrent commit from looking orphaned.
pub(super) async fn load_index(
    tx: &mut Transaction<'_, Postgres>,
    order: &str,
) -> Result<Index, LateStockError> {
    let index = sqlx::query_as::<_, Index>("SELECT (SELECT left(attempt_id,33) FROM order_stock_allocations WHERE order_id=$1) AS attempt_id,(SELECT left(kind,10) FROM order_stock_allocations WHERE order_id=$1) AS kind,(SELECT left(receipt_id,33) FROM order_stock_allocations WHERE order_id=$1) AS receipt_id,(SELECT left(id,33) FROM order_stock_settlements WHERE order_id=$1) AS original_id,(SELECT left(id,33) FROM late_stock_allocations WHERE order_id=$1) AS late_id,EXISTS(SELECT 1 FROM commerce_outbox WHERE aggregate_id=$1 AND kind='order.stock_settled.v1') AS original_event,EXISTS(SELECT 1 FROM commerce_outbox WHERE aggregate_id=$1 AND kind='order.stock_late_allocated.v1') AS late_event,EXISTS(SELECT 1 FROM commerce_outbox WHERE aggregate_id=$1 AND kind='order.stock_recovery_review.v1') AS review_event,EXISTS(SELECT 1 FROM late_stock_review_cases WHERE order_id=$1) AS review,(SELECT left(id,33) FROM late_stock_review_cases WHERE order_id=$1) AS review_id,(SELECT left(attempt_id,33) FROM late_stock_review_cases WHERE order_id=$1) AS review_attempt_id,(SELECT count(*) FROM (SELECT 1 FROM late_stock_dispositions WHERE order_id=$1 LIMIT 4) d) AS decisions,(SELECT count(*) FROM (SELECT 1 FROM commerce_outbox WHERE aggregate_id=$1 AND kind='order.stock_recovery_decided.v1' LIMIT 4) e) AS decision_events,EXISTS(SELECT 1 FROM late_stock_allocations WHERE order_id=$1) OR EXISTS(SELECT 1 FROM order_stock_settlements WHERE order_id=$1) OR EXISTS(SELECT 1 FROM commerce_outbox WHERE aggregate_id=$1 AND kind IN ('order.stock_settled.v1','order.stock_late_allocated.v1','order.stock_recovery_review.v1')) AS orphan")
        .bind(order).fetch_one(&mut **tx).await?;
    index.validate()?;
    Ok(index)
}

#[cfg(test)]
#[path = "../../tests/support/late_stock_index_unit.rs"]
mod tests;
