//! Bounded independent admission custody; mutable job flags cannot hide work.
use super::*;
#[derive(sqlx::FromRow)]
struct Header {
    id: String,
    profile_id: String,
    provider_id: String,
    received_at: i64,
    exact: bool,
}
pub(super) async fn audit(pool: &PgPool) -> Result<(), PaymentStoreError> {
    // Audit at most the global capacity plus one before filtering by profile. A
    // corrupted profile copy must not make the original profile appear idle.
    let headers=sqlx::query_as::<_,Header>(r#"SELECT left(a.intake_id,33) AS id,coalesce(left(i.profile_id,257),'') AS profile_id,
      coalesce(left(i.provider_id,257),'') AS provider_id,coalesce(i.received_at,-1) AS received_at,
      coalesce((NOT j.settled AND a.profile_id=i.profile_id AND a.created_at=i.received_at AND j.created_at=i.received_at
       AND e.intake_id=i.id AND e.created_at=i.received_at AND e.kind='payment.callback_received.v1'
       AND e.payload=jsonb_build_object('schema_version',1,'intake_id',i.id,'profile_id',i.profile_id,'provider_id',i.provider_id,'received_at',i.received_at)),false) AS exact
      FROM (SELECT * FROM callback_inbox_active ORDER BY intake_id LIMIT 201) a LEFT JOIN callback_intakes i ON i.id=a.intake_id
      LEFT JOIN callback_inbox_jobs j ON j.intake_id=a.intake_id LEFT JOIN callback_intake_events e ON e.id=j.source_event_id
      ORDER BY a.intake_id LIMIT 201"#).fetch_all(pool).await?;
    validate(headers)
}
fn validate(headers: Vec<Header>) -> Result<(), PaymentStoreError> {
    if headers.len() > 200 {
        return Err(PaymentStoreError::InvalidStoredData);
    }
    for header in headers {
        IntakeSnapshot {
            schema_version: 1,
            id: header.id,
            profile_id: header.profile_id,
            provider_id: header.provider_id,
            received_at: header.received_at,
        }
        .validate()
        .map_err(|_| PaymentStoreError::InvalidStoredData)?;
        if !header.exact {
            return Err(PaymentStoreError::InvalidStoredData);
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../tests/support/callback_inbox_source_rows_active.rs"]
mod tests;
