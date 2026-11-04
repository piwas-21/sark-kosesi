//! Canonical cross-path ownership and bounded stored headers; no detached authority.
use super::*;
#[derive(sqlx::FromRow)]
struct Index {
    attempt_id: Option<String>,
    kind: Option<String>,
    receipt_id: Option<String>,
    review: bool,
    original_id: Option<String>,
    late_id: Option<String>,
    original_event: bool,
    late_event: bool,
    review_event: bool,
    orphan: bool,
}
pub(super) async fn read(
    tx: &mut Transaction<'_, Postgres>,
    order: &str,
) -> Result<Option<Outcome>, LateStockError> {
    // One statement snapshot prevents a normal concurrent commit from looking orphaned.
    let index:Index=sqlx::query_as("SELECT (SELECT left(attempt_id,33) FROM order_stock_allocations WHERE order_id=$1) AS attempt_id,(SELECT left(kind,10) FROM order_stock_allocations WHERE order_id=$1) AS kind,(SELECT left(receipt_id,33) FROM order_stock_allocations WHERE order_id=$1) AS receipt_id,(SELECT left(id,33) FROM order_stock_settlements WHERE order_id=$1) AS original_id,(SELECT left(id,33) FROM late_stock_allocations WHERE order_id=$1) AS late_id,EXISTS(SELECT 1 FROM commerce_outbox WHERE aggregate_id=$1 AND kind='order.stock_settled.v1') AS original_event,EXISTS(SELECT 1 FROM commerce_outbox WHERE aggregate_id=$1 AND kind='order.stock_late_allocated.v1') AS late_event,EXISTS(SELECT 1 FROM commerce_outbox WHERE aggregate_id=$1 AND kind='order.stock_recovery_review.v1') AS review_event,EXISTS(SELECT 1 FROM late_stock_review_cases WHERE order_id=$1) AS review,EXISTS(SELECT 1 FROM late_stock_allocations WHERE order_id=$1) OR EXISTS(SELECT 1 FROM order_stock_settlements WHERE order_id=$1) OR EXISTS(SELECT 1 FROM commerce_outbox WHERE aggregate_id=$1 AND kind IN ('order.stock_settled.v1','order.stock_late_allocated.v1','order.stock_recovery_review.v1')) AS orphan")
        .bind(order).fetch_one(&mut **tx).await?;
    match (index.attempt_id, index.kind, index.receipt_id) {
        (Some(attempt), Some(kind), Some(id)) => {
            if index.review || index.review_event || !identifier(&attempt) || !identifier(&id) {
                return Err(LateStockError::InvalidStoredData);
            }
            if match kind.as_str() {
                "original" => {
                    index.original_id.as_deref() != Some(id.as_str())
                        || index.late_id.is_some()
                        || index.late_event
                }
                "late" => {
                    index.late_id.as_deref() != Some(id.as_str())
                        || index.original_id.is_some()
                        || index.original_event
                }
                _ => true,
            } {
                return Err(LateStockError::InvalidStoredData);
            }
            let outcome = match kind.as_str() {
                "original" => Outcome::Original(
                    crate::order_stock::read_in_transaction(tx, order)
                        .await?
                        .filter(|receipt| receipt.id == id && receipt.attempt_id == attempt)
                        .ok_or(LateStockError::InvalidStoredData)?,
                ),
                "late" => {
                    let receipt = allocation(tx, order)
                        .await?
                        .filter(|receipt| receipt.id == id && receipt.attempt_id == attempt)
                        .ok_or(LateStockError::InvalidStoredData)?;
                    history::allocation(tx, &receipt).await?;
                    Outcome::Late(receipt)
                }
                _ => return Err(LateStockError::InvalidStoredData),
            };
            Ok(Some(outcome))
        }
        (None, None, None) => {
            if index.review {
                if index.original_id.is_some()
                    || index.late_id.is_some()
                    || index.original_event
                    || index.late_event
                {
                    return Err(LateStockError::InvalidStoredData);
                }
                let case = review_case(tx, order)
                    .await?
                    .ok_or(LateStockError::InvalidStoredData)?;
                history::review(tx, &case).await?;
                return Ok(Some(Outcome::NeedsReview(case)));
            }
            if index.orphan {
                return Err(LateStockError::InvalidStoredData);
            }
            Ok(None)
        }
        _ => Err(LateStockError::InvalidStoredData),
    }
}
#[derive(sqlx::FromRow)]
struct AllocationRow {
    id: String,
    order_id: String,
    attempt_id: String,
    recognition_id: String,
    original_group_id: String,
    fresh_group_id: String,
    actor: String,
    allocated_at: i64,
    expires_at: i64,
    fence_key: i32,
    footprint: Option<serde_json::Value>,
    event_present: bool,
}
async fn allocation(
    tx: &mut Transaction<'_, Postgres>,
    order: &str,
) -> Result<Option<LateAllocation>, LateStockError> {
    let row:Option<AllocationRow>=sqlx::query_as(r#"SELECT left(s.id,33) AS id,left(s.order_id,33) AS order_id,left(s.attempt_id,33) AS attempt_id,
      left(s.recognition_id,33) AS recognition_id,left(s.original_group_id,33) AS original_group_id,left(s.fresh_group_id,33) AS fresh_group_id,
      left(s.actor,129) AS actor,s.allocated_at,s.expires_at,s.fence_key,
      CASE WHEN octet_length(s.footprint::text)<=2097152 THEN s.footprint END AS footprint,
      CASE WHEN octet_length(s.footprint::text)<=2097152 THEN EXISTS(SELECT 1 FROM commerce_outbox e WHERE e.aggregate_id=s.order_id
        AND e.kind='order.stock_late_allocated.v1' AND e.created_at=s.allocated_at
        AND e.payload=jsonb_build_object('schema_version',1,'order_id',s.order_id,'allocation_id',s.id,'allocation_sha256',late_stock_digest(s))) ELSE false END AS event_present
      FROM late_stock_allocations s WHERE s.order_id=$1"#).bind(order).fetch_optional(&mut **tx).await?;
    let Some(row) = row else {
        return Ok(None);
    };
    if !row.event_present || row.fence_key <= 0 {
        return Err(LateStockError::InvalidStoredData);
    }
    let footprint = decode::allocation(row.footprint.ok_or(LateStockError::InvalidStoredData)?)?;
    let receipt = LateAllocation {
        id: row.id,
        order_id: row.order_id,
        attempt_id: row.attempt_id,
        recognition_id: row.recognition_id,
        original_group_id: row.original_group_id,
        fresh_group_id: row.fresh_group_id,
        actor: row.actor,
        allocated_at: row.allocated_at,
        expires_at: row.expires_at,
        footprint,
    };
    receipt
        .validate()
        .map_err(|_| LateStockError::InvalidStoredData)?;
    Ok(Some(receipt))
}
#[derive(sqlx::FromRow)]
struct ReviewRow {
    id: String,
    order_id: String,
    attempt_id: String,
    recognition_id: String,
    group_id: String,
    actor: String,
    reason: String,
    assessed_at: i64,
    fence_key: i32,
    footprint: Option<serde_json::Value>,
    event_present: bool,
}
async fn review_case(
    tx: &mut Transaction<'_, Postgres>,
    order: &str,
) -> Result<Option<ReviewCase>, LateStockError> {
    let row:Option<ReviewRow>=sqlx::query_as(r#"SELECT left(s.id,33) AS id,left(s.order_id,33) AS order_id,left(s.attempt_id,33) AS attempt_id,left(s.recognition_id,33) AS recognition_id,
      left(s.group_id,33) AS group_id,left(s.actor,129) AS actor,left(s.reason,41) AS reason,s.assessed_at,s.fence_key,
      CASE WHEN octet_length(s.footprint::text)<=2097152 THEN s.footprint END AS footprint,
      CASE WHEN octet_length(s.footprint::text)<=2097152 THEN EXISTS(SELECT 1 FROM commerce_outbox e WHERE e.aggregate_id=s.order_id
        AND e.kind='order.stock_recovery_review.v1' AND e.created_at=s.assessed_at
        AND e.payload=jsonb_build_object('schema_version',1,'order_id',s.order_id,'case_id',s.id,'case_sha256',late_stock_review_digest(s))) ELSE false END AS event_present
      FROM late_stock_review_cases s WHERE s.order_id=$1"#).bind(order).fetch_optional(&mut **tx).await?;
    let Some(row) = row else {
        return Ok(None);
    };
    if !row.event_present || row.fence_key <= 0 {
        return Err(LateStockError::InvalidStoredData);
    }
    let reason = match row.reason.as_str() {
        "prior_consumption_without_allocation" => ReviewReason::PriorConsumptionWithoutAllocation,
        "release_intent_unresolved" => ReviewReason::ReleaseIntentUnresolved,
        "component_scarcity" => ReviewReason::ComponentScarcity,
        _ => return Err(LateStockError::InvalidStoredData),
    };
    let case = ReviewCase {
        id: row.id,
        order_id: row.order_id,
        attempt_id: row.attempt_id,
        recognition_id: row.recognition_id,
        group_id: row.group_id,
        actor: row.actor,
        reason,
        assessed_at: row.assessed_at,
        footprint: decode::review(row.footprint.ok_or(LateStockError::InvalidStoredData)?)?,
    };
    case.validate()
        .map_err(|_| LateStockError::InvalidStoredData)?;
    Ok(Some(case))
}
pub(super) async fn insert_allocation(
    tx: &mut Transaction<'_, Postgres>,
    s: &LateAllocation,
) -> Result<(), LateStockError> {
    sqlx::query("INSERT INTO late_stock_allocations(id,order_id,attempt_id,recognition_id,original_group_id,fresh_group_id,actor,allocated_at,expires_at,footprint) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)")
        .bind(&s.id).bind(&s.order_id).bind(&s.attempt_id).bind(&s.recognition_id).bind(&s.original_group_id).bind(&s.fresh_group_id).bind(&s.actor).bind(s.allocated_at).bind(s.expires_at).bind(envelope(&s.footprint)?).execute(&mut **tx).await?;
    Ok(())
}
pub(super) async fn insert_review(
    tx: &mut Transaction<'_, Postgres>,
    s: &ReviewCase,
) -> Result<(), LateStockError> {
    sqlx::query("INSERT INTO late_stock_review_cases(id,order_id,attempt_id,recognition_id,group_id,actor,reason,assessed_at,footprint) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)")
        .bind(&s.id).bind(&s.order_id).bind(&s.attempt_id).bind(&s.recognition_id).bind(&s.group_id).bind(&s.actor).bind(s.reason.code()).bind(s.assessed_at).bind(envelope(&s.footprint)?).execute(&mut **tx).await?;
    Ok(())
}
pub(super) async fn allocation_event(
    tx: &mut Transaction<'_, Postgres>,
    receipt: &str,
    event: &str,
) -> Result<(), LateStockError> {
    sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) SELECT $1,s.order_id,'order.stock_late_allocated.v1',jsonb_build_object('schema_version',1,'order_id',s.order_id,'allocation_id',s.id,'allocation_sha256',late_stock_digest(s)),s.allocated_at FROM late_stock_allocations s WHERE s.id=$2")
        .bind(event).bind(receipt).execute(&mut **tx).await?;
    Ok(())
}
pub(super) async fn review_event(
    tx: &mut Transaction<'_, Postgres>,
    receipt: &str,
    event: &str,
) -> Result<(), LateStockError> {
    sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) SELECT $1,s.order_id,'order.stock_recovery_review.v1',jsonb_build_object('schema_version',1,'order_id',s.order_id,'case_id',s.id,'case_sha256',late_stock_review_digest(s)),s.assessed_at FROM late_stock_review_cases s WHERE s.id=$2")
        .bind(event).bind(receipt).execute(&mut **tx).await?;
    Ok(())
}
fn envelope<T: serde::Serialize>(value: &T) -> Result<serde_json::Value, LateStockError> {
    if serde_json::to_vec(value)
        .map_err(|_| LateStockError::InvalidStoredData)?
        .len()
        > MAX_ENVELOPE_BYTES
    {
        return Err(LateStockError::InvalidStoredData);
    }
    serde_json::to_value(value).map_err(|_| LateStockError::InvalidStoredData)
}

#[cfg(test)]
#[path = "../../tests/support/late_stock_rows_unit.rs"]
mod boundary_tests;
