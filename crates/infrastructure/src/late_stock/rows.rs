//! Canonical cross-path ownership and bounded stored headers; no detached authority.
use super::index::load_index;
use super::*;
pub(super) async fn read(
    tx: &mut Transaction<'_, Postgres>,
    order: &str,
) -> Result<Option<Outcome>, LateStockError> {
    // An open case can acquire an allocation while its historical proof is read.
    // Claim its attempt before touching hold/stock sources, then reload the index.
    let mut index = load_index(tx, order).await?;
    if index.review && index.attempt_id.is_none() {
        let case_id = index
            .review_id
            .clone()
            .filter(|id| identifier(id))
            .ok_or(LateStockError::InvalidStoredData)?;
        let attempt = index
            .review_attempt_id
            .clone()
            .filter(|id| identifier(id))
            .ok_or(LateStockError::InvalidStoredData)?;
        crate::order_stock::source::claim_attempt(tx, &attempt).await?;
        index = load_index(tx, order).await?;
        index.require_review_binding(&case_id, &attempt)?;
    }
    match (
        index.attempt_id.as_deref(),
        index.kind.as_deref(),
        index.receipt_id.as_deref(),
    ) {
        (Some(attempt), Some(kind), Some(id)) => {
            if index.review {
                let case = review_case(tx, order)
                    .await?
                    .ok_or(LateStockError::InvalidStoredData)?;
                index.require_review_binding(&case.id, &case.attempt_id)?;
                history::review(tx, &case).await?;
                let decisions = disposition::validate_history(tx, &case.id).await?;
                validate_decisions(&decisions, index.decisions, Some(id))?;
            }
            let outcome = match kind {
                "original" => {
                    let receipt = crate::order_stock::read_in_transaction(tx, order)
                        .await?
                        .ok_or(LateStockError::InvalidStoredData)?;
                    validate_receipt_binding(&receipt.id, &receipt.attempt_id, id, attempt)?;
                    Outcome::Original(receipt)
                }
                "late" => {
                    let receipt = allocation(tx, order)
                        .await?
                        .ok_or(LateStockError::InvalidStoredData)?;
                    validate_receipt_binding(&receipt.id, &receipt.attempt_id, id, attempt)?;
                    history::allocation(tx, &receipt).await?;
                    Outcome::Late(receipt)
                }
                _ => return Err(LateStockError::InvalidStoredData),
            };
            Ok(Some(outcome))
        }
        (None, None, None) => {
            if index.review {
                let case = review_case(tx, order)
                    .await?
                    .ok_or(LateStockError::InvalidStoredData)?;
                index.require_review_binding(&case.id, &case.attempt_id)?;
                history::review(tx, &case).await?;
                let decisions = disposition::validate_history(tx, &case.id).await?;
                validate_decisions(&decisions, index.decisions, None)?;
                return Ok(Some(Outcome::NeedsReview(case)));
            }
            Ok(None)
        }
        _ => Err(LateStockError::InvalidStoredData),
    }
}
// The canonical claim must name the actual receipt and its owning attempt.
fn validate_receipt_binding(
    receipt_id: &str,
    receipt_attempt: &str,
    claimed_id: &str,
    claimed_attempt: &str,
) -> Result<(), LateStockError> {
    if receipt_id != claimed_id || receipt_attempt != claimed_attempt {
        return Err(LateStockError::InvalidStoredData);
    }
    Ok(())
}
// The index count and the complete validated history must agree about whether
// this case remains open or produced the exact canonical allocation.
fn validate_decisions(
    decisions: &[disposition::DecisionReceipt],
    expected_count: i64,
    allocation_id: Option<&str>,
) -> Result<(), LateStockError> {
    use disposition::DecisionOutcome;
    if i64::try_from(decisions.len()).ok() != Some(expected_count) {
        return Err(LateStockError::InvalidStoredData);
    }
    let consistent = match allocation_id {
        Some(id) => {
            matches!(decisions.last().map(|d| &d.outcome), Some(DecisionOutcome::Allocated { allocation_id }) if allocation_id == id)
        }
        None => !decisions
            .iter()
            .any(|d| matches!(d.outcome, DecisionOutcome::Allocated { .. })),
    };
    if !consistent {
        return Err(LateStockError::InvalidStoredData);
    }
    Ok(())
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
pub(super) async fn review_case(
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
pub(super) fn envelope<T: serde::Serialize>(
    value: &T,
) -> Result<serde_json::Value, LateStockError> {
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

#[cfg(test)]
#[path = "../../tests/support/late_stock_rows_outcomes_unit.rs"]
mod outcome_tests;
