use super::*;
#[derive(sqlx::FromRow)]
struct Row {
    intake_id: String,
    source_event_id: String,
    created_at: i64,
    updated_at: i64,
    due_at: i64,
    generation: i64,
    lease_until: Option<i64>,
    state: String,
    review_reason: Option<String>,
    settled: bool,
    source_present: bool,
    generation_present: bool,
    active_exact: bool,
    settlement_exact: bool,
}
pub(super) async fn load(
    tx: &mut Transaction<'_, Postgres>,
    id: &str,
) -> Result<Option<CallbackJob>, PaymentStoreError> {
    let row=sqlx::query_as::<_,Row>(r#"SELECT left(j.intake_id,33) AS intake_id,left(j.source_event_id,33) AS source_event_id,
      j.created_at,j.updated_at,j.due_at,j.generation,j.lease_until,j.settled,left(j.state,32) AS state,left(j.review_reason,32) AS review_reason,
      ((NOT j.settled)=EXISTS(SELECT 1 FROM callback_inbox_active a WHERE a.intake_id=j.intake_id)
       AND NOT EXISTS(SELECT 1 FROM callback_inbox_active a JOIN callback_intakes i ON i.id=a.intake_id
        WHERE a.intake_id=j.intake_id AND (a.profile_id<>i.profile_id OR a.created_at<>i.received_at))) AS active_exact,
      (NOT j.settled OR callback_intake_settleable(j.intake_id)) AS settlement_exact,
      EXISTS(SELECT 1 FROM callback_intake_events e JOIN callback_intakes i ON i.id=e.intake_id
       WHERE i.id=j.intake_id AND i.received_at=j.created_at AND e.id=j.source_event_id AND e.created_at=j.created_at
       AND e.kind='payment.callback_received.v1' AND e.payload=jsonb_build_object('schema_version',1,'intake_id',j.intake_id,'profile_id',i.profile_id,'provider_id',i.provider_id,'received_at',i.received_at)) AS source_present,
      (SELECT count(*)=j.generation AND coalesce(bool_and(coalesce(
       g.generation BETWEEN 1 AND j.generation AND g.generation<=5 AND g.created_at>=i.received_at AND g.created_at<=j.updated_at
       AND o.profile_id=i.profile_id AND o.provider_id=i.provider_id AND o.created_at=g.created_at
       AND s.started_at=g.created_at AND g.lease_until=g.created_at+60
       AND c.intake_id=g.intake_id AND c.generation=g.generation AND c.operation_id=g.operation_id
       AND c.created_at=g.created_at AND c.lease_until=g.lease_until
       AND (g.generation<>j.generation OR j.lease_until IS NULL OR j.lease_until=g.lease_until),false)),true)
       FROM callback_inbox_generations g JOIN callback_intakes i ON i.id=g.intake_id
       LEFT JOIN callback_generation_claims c ON c.intake_id=g.intake_id AND c.generation=g.generation
       LEFT JOIN callback_lookup_operations o ON o.id=g.operation_id LEFT JOIN callback_lookup_fetch_starts s ON s.operation_id=o.id
       WHERE g.intake_id=j.intake_id) AS generation_present
      FROM callback_inbox_jobs j WHERE j.intake_id=$1"#).bind(id).fetch_optional(&mut **tx).await?;
    row.map(Row::decode).transpose()
}
impl Row {
    fn decode(self) -> Result<CallbackJob, PaymentStoreError> {
        if !self.source_present
            || !self.generation_present
            || !self.active_exact
            || !self.settlement_exact
        {
            return Err(PaymentStoreError::InvalidStoredData);
        }
        let state = serde_json::from_value(serde_json::Value::String(self.state))
            .map_err(|_| PaymentStoreError::InvalidStoredData)?;
        let reason = self
            .review_reason
            .map(|v| serde_json::from_value(serde_json::Value::String(v)))
            .transpose()
            .map_err(|_| PaymentStoreError::InvalidStoredData)?;
        let job = CallbackJob {
            intake_id: self.intake_id,
            source_event_id: self.source_event_id,
            created_at: self.created_at,
            updated_at: self.updated_at,
            due_at: self.due_at,
            generation: self.generation,
            lease_until: self.lease_until,
            state,
            review_reason: reason,
            settled: self.settled,
        };
        job.validate()
            .map_err(|_| PaymentStoreError::InvalidStoredData)?;
        Ok(job)
    }
}
pub(super) fn state(state: JobState) -> Result<String, PaymentStoreError> {
    serde_json::to_value(state)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .ok_or(PaymentStoreError::InvalidStoredData)
}
pub(super) fn reason(reason: Option<ReviewReason>) -> Result<Option<String>, PaymentStoreError> {
    reason
        .map(|v| {
            serde_json::to_value(v)
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .ok_or(PaymentStoreError::InvalidStoredData)
        })
        .transpose()
}

/// Bounds apply before walking imported identifier lists. These readers return
/// identifiers only; they confer neither lease nor authenticated-fetch authority.
pub(super) fn bounded_operations(ids: Vec<String>) -> Result<Vec<String>, PaymentStoreError> {
    if ids.len() > 5 || ids.iter().any(|id| !identifier(id)) {
        return Err(PaymentStoreError::InvalidStoredData);
    }
    Ok(ids)
}
pub(super) fn bounded_intakes(ids: Vec<String>) -> Result<Vec<String>, PaymentStoreError> {
    if ids.len() > 200 || ids.iter().any(|id| !identifier(id)) {
        return Err(PaymentStoreError::InvalidStoredData);
    }
    Ok(ids)
}

#[cfg(test)]
#[path = "../../../../tests/support/callback_inbox_rows.rs"]
mod tests;
