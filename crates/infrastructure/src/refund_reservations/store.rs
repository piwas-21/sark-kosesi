use super::*;
#[derive(sqlx::FromRow)]
struct Row {
    id: String,
    order_id: String,
    attempt_id: String,
    recognition_id: String,
    actor_id: String,
    effect_key: String,
    generation: i16,
    reason: String,
    reserved_at: i64,
    allocation: Option<serde_json::Value>,
}
impl Row {
    fn decode(self) -> Result<RefundReceipt, RefundError> {
        if ![
            &self.id,
            &self.order_id,
            &self.attempt_id,
            &self.recognition_id,
        ]
        .iter()
        .all(|s| identifier(s))
            || !commerce_domain::media::valid_command_id(&self.actor_id)
            || !commerce_domain::media::valid_command_id(&self.effect_key)
            || !(1..=16).contains(&self.generation)
            || self.reason.is_empty()
            || self.reason.len() > 240
            || self.reason.trim() != self.reason
            || self.reason.chars().any(char::is_control)
        {
            return Err(RefundError::InvalidStoredData);
        }
        let allocation: RefundAllocation =
            serde_json::from_value(self.allocation.ok_or(RefundError::InvalidStoredData)?)
                .map_err(|_| RefundError::InvalidStoredData)?;
        if allocation.schema_version != 1 || !(1..=20).contains(&allocation.lines.len()) {
            return Err(RefundError::InvalidStoredData);
        }
        Ok(RefundReceipt {
            id: self.id,
            order_id: self.order_id,
            attempt_id: self.attempt_id,
            recognition_id: self.recognition_id,
            actor_id: self.actor_id,
            effect_key: self.effect_key,
            generation: self.generation,
            reason: self.reason,
            reserved_at: self.reserved_at,
            allocation,
            replayed: false,
        })
    }
}
pub(super) async fn lookup(
    tx: &mut Transaction<'_, Postgres>,
    key: &str,
) -> Result<Option<(String, String)>, RefundError> {
    let result: Option<(String, String)> = sqlx::query_as(
        "SELECT left(order_id,33),left(attempt_id,33) FROM refund_reservations WHERE effect_key=$1",
    )
    .bind(key)
    .fetch_optional(&mut **tx)
    .await?;
    if result
        .as_ref()
        .is_some_and(|(o, a)| !identifier(o) || !identifier(a))
    {
        return Err(RefundError::InvalidStoredData);
    }
    Ok(result)
}
pub(super) async fn require_exact_history(
    tx: &mut Transaction<'_, Postgres>,
    order: &str,
) -> Result<(), RefundError> {
    let valid: bool = sqlx::query_scalar("SELECT refund_reservation_history_exact($1)")
        .bind(order)
        .fetch_one(&mut **tx)
        .await?;
    if !valid {
        return Err(RefundError::InvalidStoredData);
    }
    Ok(())
}
pub(super) async fn history(
    tx: &mut Transaction<'_, Postgres>,
    order: &str,
) -> Result<Vec<RefundReceipt>, RefundError> {
    let rows: Vec<Row> = sqlx::query_as("SELECT left(id,33) id,left(order_id,33) order_id,left(attempt_id,33) attempt_id,left(recognition_id,33) recognition_id,left(actor_id,129) actor_id,left(effect_key,129) effect_key,generation,left(reason,241) reason,reserved_at,CASE WHEN octet_length(allocation::text)<=16384 THEN allocation END AS allocation FROM refund_reservations WHERE order_id=$1 ORDER BY generation LIMIT 17")
        .bind(order).fetch_all(&mut **tx).await?;
    if rows.len() > 16 {
        return Err(RefundError::InvalidStoredData);
    }
    rows.into_iter().map(Row::decode).collect()
}
pub(super) fn take_receipt(
    history: Vec<RefundReceipt>,
    key: &str,
) -> Result<RefundReceipt, RefundError> {
    history
        .into_iter()
        .find(|r| r.effect_key == key)
        .ok_or(RefundError::InvalidStoredData)
}
pub(super) async fn insert(
    tx: &mut Transaction<'_, Postgres>,
    r: &RefundReceipt,
    event: &str,
) -> Result<(), RefundError> {
    let json = serde_json::to_value(&r.allocation).map_err(|_| RefundError::InvalidStoredData)?;
    let inserted = sqlx::query("INSERT INTO refund_reservations(id,order_id,attempt_id,recognition_id,actor_id,effect_key,generation,reason,reserved_at,allocation,event_id) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)")
        .bind(&r.id).bind(&r.order_id).bind(&r.attempt_id).bind(&r.recognition_id).bind(&r.actor_id).bind(&r.effect_key)
        .bind(r.generation).bind(&r.reason).bind(r.reserved_at).bind(json).bind(event).execute(&mut **tx).await?;
    if inserted.rows_affected() != 1 {
        return Err(RefundError::InvalidStoredData);
    }
    let inserted = sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) SELECT s.event_id,s.order_id,'order.refund_reserved.v1',jsonb_build_object('schema_version',1,'order_id',s.order_id,'reservation_id',s.id,'reservation_sha256',refund_reservation_digest(s)),s.reserved_at FROM refund_reservations s WHERE s.id=$1")
        .bind(&r.id).execute(&mut **tx).await?;
    if inserted.rows_affected() != 1 {
        return Err(RefundError::InvalidStoredData);
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/support/refund_reservation_store.rs"]
mod tests;
