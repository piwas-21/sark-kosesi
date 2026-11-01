//! Source-only proof for redacted exceptions; never decodes fetched payment JSON.
use super::*;
#[derive(sqlx::FromRow)]
struct SourceRow {
    intake_id: String,
    profile_id: String,
    provider_id: String,
    received_at: i64,
    created_at: i64,
    event_id: Option<String>,
    source_present: bool,
    receipt_present: bool,
}
pub(super) struct Source {
    pub event_id: Option<String>,
    pub created_at: i64,
}
pub(super) async fn read(pool: &PgPool, operation_id: &str) -> Result<Source, PaymentStoreError> {
    read_source(pool, operation_id, true).await
}
/// A quarantine proves only original intake/generation identity; a damaged
/// callback processing event is not part of that source-only proof.
pub(super) async fn identity(pool: &PgPool, operation_id: &str) -> Result<i64, PaymentStoreError> {
    read_source(pool, operation_id, false)
        .await
        .map(|v| v.created_at)
}
async fn read_source(
    pool: &PgPool,
    operation_id: &str,
    event_header: bool,
) -> Result<Source, PaymentStoreError> {
    let row=sqlx::query_as::<_,SourceRow>(r#"SELECT left(i.id,33) AS intake_id,left(i.profile_id,257) AS profile_id,
      left(i.provider_id,257) AS provider_id,i.received_at,g.created_at,CASE WHEN $2::boolean THEN left(e.id,33) END AS event_id,
      EXISTS(SELECT 1 FROM callback_lookup_receipts r WHERE r.operation_id=g.operation_id) AS receipt_present,
      (g.generation BETWEEN 1 AND 5 AND g.generation<=j.generation AND g.created_at>=i.received_at
       AND g.created_at<=j.updated_at AND g.lease_until=g.created_at+60 AND o.created_at=g.created_at
       AND o.profile_id=i.profile_id AND o.provider_id=i.provider_id AND s.started_at=g.created_at
       AND c.operation_id=g.operation_id AND c.created_at=g.created_at AND c.lease_until=g.lease_until
       AND j.created_at=i.received_at AND source.created_at=i.received_at AND source.kind='payment.callback_received.v1'
       AND source.payload=jsonb_build_object('schema_version',1,'intake_id',i.id,'profile_id',i.profile_id,'provider_id',i.provider_id,'received_at',i.received_at)) AS source_present
      FROM callback_inbox_generations g JOIN callback_intakes i ON i.id=g.intake_id
      JOIN callback_generation_claims c ON c.intake_id=g.intake_id AND c.generation=g.generation
      JOIN callback_inbox_jobs j ON j.intake_id=i.id JOIN callback_intake_events source ON source.id=j.source_event_id AND source.intake_id=i.id
      JOIN callback_lookup_operations o ON o.id=g.operation_id JOIN callback_lookup_fetch_starts s ON s.operation_id=o.id
      LEFT JOIN callback_processing_events e ON e.operation_id=g.operation_id
      WHERE g.operation_id=$1"#).bind(operation_id).bind(event_header).fetch_optional(pool).await?.ok_or(PaymentStoreError::InvalidStoredData)?;
    decode(row)
}
fn decode(row: SourceRow) -> Result<Source, PaymentStoreError> {
    IntakeSnapshot {
        schema_version: 1,
        id: row.intake_id,
        profile_id: row.profile_id,
        provider_id: row.provider_id,
        received_at: row.received_at,
    }
    .validate()
    .map_err(|_| PaymentStoreError::InvalidStoredData)?;
    if !row.source_present
        || !row.receipt_present
        || !(row.received_at..=9_007_199_254_740_991).contains(&row.created_at)
        || row.event_id.as_ref().is_some_and(|v| !identifier(v))
    {
        return Err(PaymentStoreError::InvalidStoredData);
    }
    Ok(Source {
        event_id: row.event_id,
        created_at: row.created_at,
    })
}

#[cfg(test)]
#[path = "../../../../tests/support/callback_inbox_source_rows_processing.rs"]
mod tests;
