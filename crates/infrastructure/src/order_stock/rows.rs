//! Immutable canonical receipt, independently bound event, and historical effects.
use super::*;
#[derive(sqlx::FromRow)]
struct Row {
    id: String,
    order_id: String,
    attempt_id: String,
    recognition_id: String,
    group_id: String,
    actor: String,
    settled_at: i64,
    fence_key: i32,
    footprint: Option<serde_json::Value>,
    event_present: bool,
    allocation_present: bool,
}
pub(crate) async fn load(
    tx: &mut Transaction<'_, Postgres>,
    order_id: &str,
) -> Result<Option<Settlement>, SettlementError> {
    let (present, orphan): (bool, bool) = sqlx::query_as("SELECT EXISTS(SELECT 1 FROM order_stock_settlements WHERE order_id=$1),EXISTS(SELECT 1 FROM commerce_outbox WHERE aggregate_id=$1 AND kind='order.stock_settled.v1') OR EXISTS(SELECT 1 FROM order_stock_allocations WHERE order_id=$1 AND kind='original')")
        .bind(order_id).fetch_one(&mut **tx).await?;
    if !present {
        #[cfg(test)]
        absence_hook::pause(tx, order_id).await?;
        // Keep the captured orphan flag: a later probe would use a different snapshot.
        if orphan {
            return Err(SettlementError::InvalidStoredData);
        }
        return Ok(None);
    }
    let row: Option<Row> = sqlx::query_as(
        r#"SELECT left(s.id,33) AS id,left(s.order_id,33) AS order_id,
      left(s.attempt_id,33) AS attempt_id,left(s.recognition_id,33) AS recognition_id,
      left(s.group_id,33) AS group_id,left(s.actor,129) AS actor,s.settled_at,s.fence_key,
      CASE WHEN octet_length(s.footprint::text)<=1048576 THEN s.footprint END AS footprint,
      CASE WHEN octet_length(s.footprint::text)<=1048576 THEN EXISTS(SELECT 1 FROM commerce_outbox e WHERE e.aggregate_id=s.order_id
        AND e.kind='order.stock_settled.v1' AND e.created_at=s.settled_at
        AND e.payload=jsonb_build_object('schema_version',1,'order_id',s.order_id,
          'settlement_id',s.id,'settlement_sha256',order_stock_digest(s))) ELSE false END AS event_present,
      EXISTS(SELECT 1 FROM order_stock_allocations a WHERE a.order_id=s.order_id AND a.attempt_id=s.attempt_id AND a.kind='original' AND a.receipt_id=s.id)
        AND NOT EXISTS(SELECT 1 FROM late_stock_review_cases WHERE order_id=s.order_id)
        AND NOT EXISTS(SELECT 1 FROM late_stock_allocations WHERE order_id=s.order_id)
        AND NOT EXISTS(SELECT 1 FROM commerce_outbox WHERE aggregate_id=s.order_id AND kind IN ('order.stock_late_allocated.v1','order.stock_recovery_review.v1')) AS allocation_present
      FROM order_stock_settlements s WHERE s.order_id=$1"#,
    )
    .bind(order_id)
    .fetch_optional(&mut **tx)
    .await?;
    let row = row.ok_or(SettlementError::InvalidStoredData)?;
    if !row.event_present
        || !row.allocation_present
        || row.fence_key <= 0
        || !identifier(&row.attempt_id)
        || !identifier(&row.recognition_id)
    {
        return Err(SettlementError::InvalidStoredData);
    }
    let footprint = decode_footprint(row.footprint.ok_or(SettlementError::InvalidStoredData)?)?;
    let settlement = Settlement {
        id: row.id,
        order_id: row.order_id,
        attempt_id: row.attempt_id,
        recognition_id: row.recognition_id,
        group_id: row.group_id,
        actor: row.actor,
        settled_at: row.settled_at,
        footprint,
    };
    settlement.validate()?;
    let (frozen, received) = recognition::historical_in_transaction(
        tx,
        &settlement.attempt_id,
        &settlement.recognition_id,
    )
    .await?;
    let order = crate::checkout_orders::snapshot_in_transaction(
        tx,
        &frozen.snapshot().binding.basket_id,
        order_id,
    )
    .await
    .map_err(|error| match error {
        crate::checkout_orders::OrderStoreError::Database => SettlementError::Database,
        _ => SettlementError::InvalidStoredData,
    })?;
    if settlement.order_id != order_id
        || frozen.snapshot().binding.order_id != order_id
        || settlement.group_id != order.identity.stock_group_id
        || received.state != recognition::State::Received
    {
        return Err(SettlementError::InvalidStoredData);
    }
    check_time(
        settlement.settled_at,
        order.identity.created_at,
        order.identity.hold_expires_at,
        received.recorded_at,
    )
    .map_err(|_| SettlementError::InvalidStoredData)?;
    let members = source::members(tx, &order, false).await?;
    if members.active || members.members != settlement.footprint.members {
        return Err(SettlementError::InvalidStoredData);
    }
    let all_consumed: bool = sqlx::query_scalar("SELECT NOT EXISTS(SELECT 1 FROM stock_holds WHERE basket_group_id=$1 AND state<>'consumed')")
        .bind(&settlement.group_id).fetch_one(&mut **tx).await?;
    if !all_consumed {
        return Err(SettlementError::InvalidStoredData);
    }
    effects::settled(tx, &settlement).await?;
    Ok(Some(settlement))
}
pub(super) async fn insert(
    tx: &mut Transaction<'_, Postgres>,
    s: &Settlement,
) -> Result<(), SettlementError> {
    let footprint =
        serde_json::to_value(&s.footprint).map_err(|_| SettlementError::InvalidStoredData)?;
    if serde_json::to_vec(&s.footprint)
        .map_err(|_| SettlementError::InvalidStoredData)?
        .len()
        > MAX_FOOTPRINT_BYTES
    {
        return Err(SettlementError::InvalidStoredData);
    }
    sqlx::query("INSERT INTO order_stock_settlements(id,order_id,attempt_id,recognition_id,group_id,actor,settled_at,footprint) VALUES($1,$2,$3,$4,$5,$6,$7,$8)")
        .bind(&s.id).bind(&s.order_id).bind(&s.attempt_id).bind(&s.recognition_id).bind(&s.group_id).bind(&s.actor).bind(s.settled_at).bind(footprint).execute(&mut **tx).await?;
    Ok(())
}
pub(super) async fn event(
    tx: &mut Transaction<'_, Postgres>,
    settlement: &str,
    event: &str,
) -> Result<(), SettlementError> {
    sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) SELECT $1,s.order_id,'order.stock_settled.v1',jsonb_build_object('schema_version',1,'order_id',s.order_id,'settlement_id',s.id,'settlement_sha256',order_stock_digest(s)),s.settled_at FROM order_stock_settlements s WHERE s.id=$2")
        .bind(event).bind(settlement).execute(&mut **tx).await?;
    Ok(())
}

fn decode_footprint(value: serde_json::Value) -> Result<Footprint, SettlementError> {
    use serde_json::Value;
    let members = value
        .get("members")
        .and_then(Value::as_array)
        .ok_or(SettlementError::InvalidStoredData)?;
    let components = value
        .get("components")
        .and_then(Value::as_array)
        .ok_or(SettlementError::InvalidStoredData)?;
    if !value.is_object()
        || members.is_empty()
        || members.len() > MAX_MEMBERS
        || components.is_empty()
        || components.len() > MAX_PAIRS
        || members.iter().any(|member| {
            !member.is_object()
                || !member
                    .get("components")
                    .and_then(Value::as_array)
                    .is_some_and(|lines| {
                        !lines.is_empty()
                            && lines.len() <= MAX_LINES
                            && lines.iter().all(Value::is_object)
                    })
        })
        || components.iter().any(|component| {
            !component.is_object()
                || !component.get("before").is_some_and(Value::is_object)
                || !component.get("after").is_some_and(Value::is_object)
        })
    {
        return Err(SettlementError::InvalidStoredData);
    }
    serde_json::from_value(value).map_err(|_| SettlementError::InvalidStoredData)
}

#[cfg(test)]
#[path = "../../tests/support/order_stock_rows_unit.rs"]
mod decoder_tests;

#[cfg(test)]
#[path = "../../tests/support/order_stock_absence_hook.rs"]
pub(crate) mod absence_hook;
