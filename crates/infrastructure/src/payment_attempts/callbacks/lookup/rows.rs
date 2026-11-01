use super::*;
use serde_json::Value;
use sqlx::{Postgres, Transaction};
#[derive(sqlx::FromRow)]
struct OperationRow {
    id: String,
    profile_id: String,
    provider_id: String,
    created_at: i64,
}
pub(super) async fn operation(
    tx: &mut Transaction<'_, Postgres>,
    id: &str,
    claim: bool,
) -> Result<LookupOperationSnapshot, PaymentStoreError> {
    let query = if claim {
        "SELECT left(id,33) AS id,left(profile_id,257) AS profile_id,left(provider_id,257) AS provider_id,created_at FROM callback_lookup_operations WHERE id=$1 FOR UPDATE"
    } else {
        "SELECT left(id,33) AS id,left(profile_id,257) AS profile_id,left(provider_id,257) AS provider_id,created_at FROM callback_lookup_operations WHERE id=$1"
    };
    let row = sqlx::query_as::<_, OperationRow>(query)
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(PaymentStoreError::NotFound)?;
    row.decode(id)
}
impl OperationRow {
    fn decode(self, id: &str) -> Result<LookupOperationSnapshot, PaymentStoreError> {
        let snapshot = LookupOperationSnapshot {
            schema_version: 1,
            id: self.id,
            profile_id: self.profile_id,
            provider_id: self.provider_id,
            created_at: self.created_at,
        };
        snapshot
            .validate()
            .map_err(|_| PaymentStoreError::InvalidStoredData)?;
        if snapshot.id != id {
            return Err(PaymentStoreError::InvalidStoredData);
        }
        Ok(snapshot)
    }
}
#[derive(sqlx::FromRow)]
struct ReceiptRow {
    id: String,
    operation_id: String,
    recorded_at: i64,
    snapshot: Option<Value>,
    event_present: bool,
}
pub(super) async fn receipt(
    tx: &mut Transaction<'_, Postgres>,
    operation: &LookupOperationSnapshot,
) -> Result<Option<LookupReceipt>, PaymentStoreError> {
    let row = sqlx::query_as::<_, ReceiptRow>(r#"SELECT left(r.id,33) AS id,left(r.operation_id,33) AS operation_id,r.recorded_at,
        CASE WHEN octet_length(r.snapshot::text)<=8192 THEN r.snapshot END AS snapshot,
        CASE WHEN octet_length(r.snapshot::text)<=8192 THEN EXISTS(SELECT 1 FROM callback_processing_events e WHERE e.operation_id=r.operation_id
            AND e.receipt_id=r.id AND e.created_at=r.recorded_at AND e.kind='payment.callback_lookup_recorded.v1'
            AND e.payload=jsonb_build_object('schema_version',1,'operation_id',r.operation_id,'receipt_id',r.id,
              'snapshot_sha256',encode(sha256(convert_to(r.snapshot::text,'UTF8')),'hex'))) ELSE false END AS event_present
        FROM callback_lookup_receipts r WHERE r.operation_id=$1"#).bind(&operation.id).fetch_optional(&mut **tx).await?;
    let receipt = row.map(|row| row.decode(operation)).transpose()?;
    if let Some(receipt) = &receipt {
        validate_start(tx, &receipt.snapshot).await?;
    }
    Ok(receipt)
}
impl ReceiptRow {
    fn decode(
        self,
        operation: &LookupOperationSnapshot,
    ) -> Result<LookupReceipt, PaymentStoreError> {
        let value = self.snapshot.ok_or(PaymentStoreError::InvalidStoredData)?;
        if !self.event_present
            || !value.is_object()
            || !value.get("operation").is_some_and(Value::is_object)
            || !value.get("payment").is_some_and(Value::is_object)
        {
            return Err(PaymentStoreError::InvalidStoredData);
        }
        let snapshot: LookupSnapshot =
            serde_json::from_value(value).map_err(|_| PaymentStoreError::InvalidStoredData)?;
        snapshot
            .validate()
            .map_err(|_| PaymentStoreError::InvalidStoredData)?;
        if snapshot.id != self.id
            || self.operation_id != operation.id
            || snapshot.operation != *operation
            || snapshot.recorded_at != self.recorded_at
        {
            return Err(PaymentStoreError::InvalidStoredData);
        }
        let fetched = CallbackPayment::from_custody(&snapshot)
            .map_err(|_| PaymentStoreError::InvalidStoredData)?;
        Ok(LookupReceipt {
            snapshot,
            replayed: true,
            fetched,
        })
    }
}
pub(super) async fn insert(
    tx: &mut Transaction<'_, Postgres>,
    s: &LookupSnapshot,
    event_id: &str,
) -> Result<(), PaymentStoreError> {
    let snapshot = serde_json::to_value(s).map_err(|_| PaymentStoreError::InvalidStoredData)?;
    sqlx::query("INSERT INTO callback_lookup_receipts(id,operation_id,recorded_at,snapshot) VALUES($1,$2,$3,$4)")
        .bind(&s.id).bind(&s.operation.id).bind(s.recorded_at).bind(snapshot).execute(&mut **tx).await?;
    // Hash the exact persisted canonical JSONB text in PostgreSQL. The immutable
    // event independently binds every minimal fetched field before reconstruction.
    sqlx::query("INSERT INTO callback_processing_events(id,operation_id,receipt_id,created_at,kind,payload) SELECT $1,r.operation_id,r.id,r.recorded_at,'payment.callback_lookup_recorded.v1',jsonb_build_object('schema_version',1,'operation_id',r.operation_id,'receipt_id',r.id,'snapshot_sha256',encode(sha256(convert_to(r.snapshot::text,'UTF8')),'hex')) FROM callback_lookup_receipts r WHERE r.id=$2 AND r.operation_id=$3")
        .bind(event_id).bind(&s.id).bind(&s.operation.id).execute(&mut **tx).await?;
    Ok(())
}

pub(super) async fn validate_start(
    tx: &mut Transaction<'_, Postgres>,
    snapshot: &LookupSnapshot,
) -> Result<(), PaymentStoreError> {
    let started_at: Option<i64> = sqlx::query_scalar(
        "SELECT started_at FROM callback_lookup_fetch_starts WHERE operation_id=$1",
    )
    .bind(&snapshot.operation.id)
    .fetch_optional(&mut **tx)
    .await?;
    if started_at != Some(snapshot.started_at) {
        return Err(PaymentStoreError::InvalidStoredData);
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../tests/support/payment_callback_lookup_rows.rs"]
mod tests;
