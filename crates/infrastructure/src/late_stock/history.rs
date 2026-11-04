//! Original received proof and immutable effects; no current warehouse comparison.
use super::*;
async fn context(
    tx: &mut Transaction<'_, Postgres>,
    order: &str,
    attempt: &str,
    receipt: &str,
) -> Result<Context, LateStockError> {
    let (frozen, received) = recognition::historical_in_transaction(tx, attempt, receipt)
        .await
        .map_err(|error| match error {
            PaymentStoreError::Database
            | PaymentStoreError::Boundary(crate::checkout_orders::OrderStoreError::Database) => {
                LateStockError::Database
            }
            _ => LateStockError::InvalidStoredData,
        })?;
    if received.state != recognition::State::Received || frozen.snapshot().binding.order_id != order
    {
        return Err(LateStockError::InvalidStoredData);
    }
    let order = crate::checkout_orders::snapshot_in_transaction(
        tx,
        &frozen.snapshot().binding.basket_id,
        order,
    )
    .await
    .map_err(order_error)?;
    Ok(Context { order, received })
}
pub(super) async fn allocation(
    tx: &mut Transaction<'_, Postgres>,
    receipt: &LateAllocation,
) -> Result<(), LateStockError> {
    let context = context(
        tx,
        &receipt.order_id,
        &receipt.attempt_id,
        &receipt.recognition_id,
    )
    .await?;
    if receipt.original_group_id != context.order.identity.stock_group_id
        || receipt.allocated_at < context.order.identity.hold_expires_at
    {
        return Err(LateStockError::InvalidStoredData);
    }
    check_time(
        receipt.allocated_at,
        context.order.identity.created_at,
        context.received.recorded_at,
    )
    .map_err(|_| LateStockError::InvalidStoredData)?;
    let original = source::original(tx, &context.order, false).await?;
    source::historical(&receipt.footprint.original, &original)?;
    let mut fresh_order = context.order;
    fresh_order.identity.stock_group_id = receipt.fresh_group_id.clone();
    fresh_order.identity.created_at = receipt
        .expires_at
        .checked_sub(WINDOW_SECONDS)
        .ok_or(LateStockError::InvalidStoredData)?;
    fresh_order.identity.hold_expires_at = receipt.expires_at;
    let fresh = crate::order_stock::source::members(tx, &fresh_order, false).await?;
    if fresh.states.iter().any(|state| state != "consumed")
        || fresh.members.len() != receipt.footprint.fresh.len()
        || fresh
            .members
            .iter()
            .any(|member| !receipt.footprint.fresh.contains(member))
    {
        return Err(LateStockError::InvalidStoredData);
    }
    let exact:bool=sqlx::query_scalar("SELECT CASE WHEN octet_length(s.footprint::text)<=2097152 THEN late_stock_allocation_exact(s,false,false) ELSE false END FROM late_stock_allocations s WHERE s.id=$1 AND s.order_id=$2")
        .bind(&receipt.id).bind(&receipt.order_id).fetch_one(&mut **tx).await.map_err(proof_error)?;
    if !exact {
        return Err(LateStockError::InvalidStoredData);
    }
    Ok(())
}
pub(super) async fn review(
    tx: &mut Transaction<'_, Postgres>,
    case: &ReviewCase,
) -> Result<(), LateStockError> {
    let context = context(tx, &case.order_id, &case.attempt_id, &case.recognition_id).await?;
    if case.group_id != context.order.identity.stock_group_id
        || (case.reason == ReviewReason::ComponentScarcity
            && case.assessed_at < context.order.identity.hold_expires_at)
    {
        return Err(LateStockError::InvalidStoredData);
    }
    check_time(
        case.assessed_at,
        context.order.identity.created_at,
        context.received.recorded_at,
    )
    .map_err(|_| LateStockError::InvalidStoredData)?;
    let original = source::original(tx, &context.order, false).await?;
    source::historical(&case.footprint.original, &original)?;
    let exact:bool=sqlx::query_scalar("SELECT CASE WHEN octet_length(s.footprint::text)<=2097152 THEN late_stock_review_exact(s,false) ELSE false END FROM late_stock_review_cases s WHERE s.id=$1 AND s.order_id=$2")
        .bind(&case.id).bind(&case.order_id).fetch_one(&mut **tx).await.map_err(proof_error)?;
    if !exact {
        return Err(LateStockError::InvalidStoredData);
    }
    Ok(())
}
fn order_error(error: crate::checkout_orders::OrderStoreError) -> LateStockError {
    match error {
        crate::checkout_orders::OrderStoreError::Database => LateStockError::Database,
        _ => LateStockError::InvalidStoredData,
    }
}
fn proof_error(error: sqlx::Error) -> LateStockError {
    if matches!(&error,sqlx::Error::Database(database) if database.code().as_deref()==Some("23514"))
    {
        LateStockError::InvalidStoredData
    } else {
        LateStockError::Database
    }
}

#[cfg(test)]
#[path = "../../tests/support/late_stock_history_unit.rs"]
mod boundary_tests;
