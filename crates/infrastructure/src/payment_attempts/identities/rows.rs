use super::*;
#[derive(sqlx::FromRow)]
struct Row {
    id: String,
    attempt_id: String,
    effect_key: String,
    revision: i64,
    through_observation: i64,
    recorded_at: i64,
    summary: Option<serde_json::Value>,
    event_present: bool,
}
pub(crate) async fn load(
    tx: &mut Transaction<'_, Postgres>,
    frozen: &FrozenAttempt,
    started_at: i64,
    effect: Option<&str>,
) -> Result<Option<Resolution>, PaymentStoreError> {
    let row = sqlx::query_as::<_, Row>(
        r#"SELECT left(id,33) AS id,left(attempt_id,33) AS attempt_id,
      left(effect_key,129) AS effect_key,revision,through_observation,recorded_at,
      CASE WHEN octet_length(summary::text)<=524288 THEN summary END AS summary,
      EXISTS(SELECT 1 FROM commerce_outbox e JOIN payment_attempts a ON a.id=r.attempt_id
        WHERE e.kind='payment.identity_resolved.v1' AND e.payload->>'resolution_id'=r.id
          AND e.aggregate_id=a.order_id AND e.created_at=r.recorded_at
          AND e.payload=jsonb_build_object('schema_version',1,'order_id',a.order_id,
            'attempt_id',a.id,'resolution_id',r.id)) AS event_present
      FROM payment_identity_resolutions r WHERE attempt_id=$1
        AND ($2::text IS NULL OR effect_key=$2) ORDER BY revision DESC LIMIT 1"#,
    )
    .bind(&frozen.snapshot().binding.id)
    .bind(effect)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(r) = row else {
        return Ok(None);
    };
    let r = r.decode_header(&frozen.snapshot().binding.id, started_at)?;
    let observations =
        super::super::observations::rows::prefix(tx, frozen, started_at, r.through_observation)
            .await?;
    let expected = summarize(frozen.snapshot(), started_at, &observations)
        .map_err(|_| PaymentStoreError::InvalidStoredData)?;
    if observations.last().map_or(0, |v| v.revision) != r.through_observation
        || observations
            .last()
            .is_some_and(|v| v.recorded_at > r.recorded_at)
        || r.summary != expected
    {
        return Err(PaymentStoreError::InvalidStoredData);
    }
    Ok(Some(r))
}
impl Row {
    fn decode_header(
        self,
        attempt_id: &str,
        started_at: i64,
    ) -> Result<Resolution, PaymentStoreError> {
        if !self.event_present
            || !identifier(&self.id)
            || self.attempt_id != attempt_id
            || !valid_command_id(&self.effect_key)
            || !(1..=9_007_199_254_740_991).contains(&self.revision)
            || !(0..=1000).contains(&self.through_observation)
            || !(started_at..=9_007_199_254_740_991).contains(&self.recorded_at)
        {
            return Err(PaymentStoreError::InvalidStoredData);
        }
        let summary: IdentitySummary =
            serde_json::from_value(self.summary.ok_or(PaymentStoreError::InvalidStoredData)?)
                .map_err(|_| PaymentStoreError::InvalidStoredData)?;
        Ok(Resolution {
            id: self.id,
            attempt_id: self.attempt_id,
            effect_key: self.effect_key,
            revision: self.revision,
            through_observation: self.through_observation,
            recorded_at: self.recorded_at,
            summary,
        })
    }
}

pub(super) async fn insert(
    tx: &mut Transaction<'_, Postgres>,
    r: &Resolution,
    order: &str,
    event: &str,
) -> Result<(), PaymentStoreError> {
    let summary =
        serde_json::to_value(&r.summary).map_err(|_| PaymentStoreError::InvalidStoredData)?;
    sqlx::query("INSERT INTO payment_identity_resolutions(id,attempt_id,effect_key,revision,through_observation,recorded_at,summary) VALUES($1,$2,$3,$4,$5,$6,$7)")
        .bind(&r.id).bind(&r.attempt_id).bind(&r.effect_key).bind(r.revision).bind(r.through_observation).bind(r.recorded_at).bind(summary).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) VALUES($1,$2,'payment.identity_resolved.v1',$3,$4)")
        .bind(event).bind(order).bind(serde_json::json!({"schema_version":1,"order_id":order,"attempt_id":r.attempt_id,"resolution_id":r.id})).bind(r.recorded_at).execute(&mut **tx).await?;
    Ok(())
}

#[cfg(test)]
#[path = "../../../tests/support/payment_identity_rows.rs"]
mod tests;
