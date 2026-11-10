use super::*;

#[derive(sqlx::FromRow)]
struct OperationRow {
    id: String,
    order_id: String,
    attempt_id: String,
    effect_key: String,
    recognition_id: String,
    through_observation: i64,
    profile_id: String,
    payment_id: String,
    credential_fingerprint: String,
    started_at: i64,
    fetch_before: i64,
    current_transaction: bool,
    valid_shape: bool,
}
impl OperationRow {
    fn decode(self) -> Result<(Operation, bool), PaymentStoreError> {
        let op = Operation {
            id: self.id,
            order_id: self.order_id,
            attempt_id: self.attempt_id,
            effect_key: self.effect_key,
            recognition_id: self.recognition_id,
            through_observation: self.through_observation,
            profile_id: self.profile_id,
            payment_id: self.payment_id,
            credential_fingerprint: self.credential_fingerprint,
            started_at: self.started_at,
            fetch_before: self.fetch_before,
        };
        if !self.valid_shape {
            return Err(PaymentStoreError::InvalidStoredData);
        }
        Ok((op, self.current_transaction))
    }
}
pub(super) async fn by_effect(
    tx: &mut Transaction<'_, Postgres>,
    attempt: &str,
    effect: &str,
) -> Result<Option<Operation>, PaymentStoreError> {
    Ok(by_effect_with_xid(tx, attempt, effect).await?.map(|v| v.0))
}
pub(super) async fn by_effect_with_xid(
    tx: &mut Transaction<'_, Postgres>,
    attempt: &str,
    effect: &str,
) -> Result<Option<(Operation, bool)>, PaymentStoreError> {
    let row = sqlx::query_as::<_, OperationRow>("SELECT left(o.id,33) AS id,left(o.order_id,33) AS order_id,left(o.attempt_id,33) AS attempt_id,left(o.effect_key,129) AS effect_key,left(o.recognition_id,33) AS recognition_id,o.through_observation,left(o.profile_id,257) AS profile_id,left(o.payment_id,257) AS payment_id,left(o.credential_fingerprint,65) AS credential_fingerprint,o.started_at,o.fetch_before,COALESCE(o.transaction_id=pg_current_xact_id_if_assigned(),false) AS current_transaction,COALESCE(paid_parent_total_operation_shape(o),false) AS valid_shape FROM paid_parent_total_operations o WHERE o.attempt_id=$1 AND o.effect_key=$2")
        .bind(attempt).bind(effect).fetch_optional(&mut **tx).await?;
    row.map(OperationRow::decode).transpose()
}
pub(super) async fn history(
    tx: &mut Transaction<'_, Postgres>,
    attempt: &str,
) -> Result<(i64, Option<i64>), PaymentStoreError> {
    Ok(sqlx::query_as("SELECT count(*),max(started_at) FROM (SELECT started_at FROM paid_parent_total_operations WHERE attempt_id=$1 ORDER BY started_at,id LIMIT 129) bounded")
        .bind(attempt).fetch_one(&mut **tx).await?)
}
pub(super) async fn insert_operation(
    tx: &mut Transaction<'_, Postgres>,
    o: &Operation,
) -> Result<(), PaymentStoreError> {
    sqlx::query("INSERT INTO paid_parent_total_operations(id,order_id,attempt_id,effect_key,recognition_id,through_observation,profile_id,payment_id,credential_fingerprint,started_at,fetch_before) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)")
        .bind(&o.id).bind(&o.order_id).bind(&o.attempt_id).bind(&o.effect_key).bind(&o.recognition_id).bind(o.through_observation).bind(&o.profile_id).bind(&o.payment_id).bind(&o.credential_fingerprint).bind(o.started_at).bind(o.fetch_before).execute(&mut **tx).await?;
    Ok(())
}
#[derive(sqlx::FromRow)]
struct ReceiptRow {
    refunded_minor: Option<i64>,
    remaining_minor: Option<i64>,
    captured_minor: Option<i64>,
    charged_back_minor: Option<i64>,
    fetched_at: i64,
    recorded_at: i64,
    event_id: String,
    event_present: bool,
    valid_shape: bool,
}
pub(super) async fn receipt(
    tx: &mut Transaction<'_, Postgres>,
    op: &Operation,
) -> Result<Option<Receipt>, PaymentStoreError> {
    let row=sqlx::query_as::<_,ReceiptRow>("SELECT r.refunded_minor,r.remaining_minor,r.captured_minor,r.charged_back_minor,r.fetched_at,r.recorded_at,left(r.event_id,33) AS event_id,EXISTS(SELECT 1 FROM commerce_outbox e WHERE e.id=r.event_id AND e.aggregate_id=o.order_id AND e.kind='payment.paid_parent_totals_observed.v1' AND e.created_at=r.recorded_at AND e.payload=paid_parent_total_receipt_event(r,o)) AS event_present,COALESCE(paid_parent_total_receipt_shape(r,o),false) AS valid_shape FROM paid_parent_total_receipts r JOIN paid_parent_total_operations o ON o.id=r.operation_id WHERE r.operation_id=$1")
        .bind(&op.id).fetch_optional(&mut **tx).await?;
    let Some(r) = row else {
        return Ok(None);
    };
    let totals = ReversalTotals {
        refunded_minor: r.refunded_minor,
        remaining_minor: r.remaining_minor,
        captured_minor: r.captured_minor,
        charged_back_minor: r.charged_back_minor,
    };
    if !r.valid_shape || !r.event_present {
        return Err(PaymentStoreError::InvalidStoredData);
    }
    Ok(Some(Receipt {
        operation: op.clone(),
        totals,
        fetched_at: r.fetched_at,
        recorded_at: r.recorded_at,
        event_id: r.event_id,
        replayed: true,
    }))
}
pub(super) async fn receipt_current_transaction(
    tx: &mut Transaction<'_, Postgres>,
    id: &str,
) -> Result<bool, PaymentStoreError> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM paid_parent_total_receipts WHERE operation_id=$1 AND transaction_id=pg_current_xact_id())")
        .bind(id).fetch_one(&mut **tx).await?)
}
pub(super) async fn insert_receipt(
    tx: &mut Transaction<'_, Postgres>,
    r: &Receipt,
) -> Result<(), PaymentStoreError> {
    sqlx::query("INSERT INTO paid_parent_total_receipts(operation_id,refunded_minor,remaining_minor,captured_minor,charged_back_minor,fetched_at,recorded_at,event_id) VALUES($1,$2,$3,$4,$5,$6,$7,$8)")
        .bind(&r.operation.id).bind(r.totals.refunded_minor).bind(r.totals.remaining_minor).bind(r.totals.captured_minor).bind(r.totals.charged_back_minor).bind(r.fetched_at).bind(r.recorded_at).bind(&r.event_id).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) SELECT r.event_id,o.order_id,'payment.paid_parent_totals_observed.v1',paid_parent_total_receipt_event(r,o),r.recorded_at FROM paid_parent_total_receipts r JOIN paid_parent_total_operations o ON o.id=r.operation_id WHERE r.operation_id=$1")
        .bind(&r.operation.id).execute(&mut **tx).await?;
    Ok(())
}
