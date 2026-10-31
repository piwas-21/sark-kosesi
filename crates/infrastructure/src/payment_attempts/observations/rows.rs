use super::*;
use serde_json::Value;
#[derive(sqlx::FromRow)]
struct Row {
    id: String,
    attempt_id: String,
    effect_key: String,
    revision: i64,
    recorded_at: i64,
    provider_id: String,
    credential_fingerprint: String,
    status: String,
    snapshot: Option<Value>,
    event_present: bool,
}
pub(super) async fn by_effect(
    tx: &mut Transaction<'_, Postgres>,
    frozen: &FrozenAttempt,
    started_at: i64,
    effect: &str,
) -> Result<Option<ObservationSnapshot>, PaymentStoreError> {
    let row = sqlx::query_as::<_, Row>(
        r#"SELECT left(id,33) AS id,left(attempt_id,33) AS attempt_id,
        left(effect_key,129) AS effect_key,revision,recorded_at,
        left(provider_id,257) AS provider_id,
        left(credential_fingerprint,65) AS credential_fingerprint,left(status,16) AS status,
        CASE WHEN octet_length(snapshot::text)<=8192 THEN snapshot END AS snapshot,
        EXISTS(SELECT 1 FROM commerce_outbox e
          JOIN payment_attempts a ON a.id=payment_observations.attempt_id
          WHERE e.kind='payment.observation_recorded.v1'
            AND e.payload->>'observation_id'=payment_observations.id
            AND e.aggregate_id=a.order_id AND e.created_at=payment_observations.recorded_at
            AND e.payload=jsonb_build_object('schema_version',1,'order_id',a.order_id,
              'attempt_id',a.id,'observation_id',payment_observations.id)) AS event_present
        FROM payment_observations
        WHERE attempt_id=$1 AND effect_key=$2"#,
    )
    .bind(&frozen.snapshot().binding.id)
    .bind(effect)
    .fetch_optional(&mut **tx)
    .await?;
    row.map(|r| r.decode(frozen, started_at)).transpose()
}
pub(crate) async fn by_id(
    tx: &mut Transaction<'_, Postgres>,
    frozen: &FrozenAttempt,
    started_at: i64,
    id: &str,
) -> Result<ObservationSnapshot, PaymentStoreError> {
    let row = sqlx::query_as::<_, Row>(
        r#"SELECT left(id,33) AS id,left(attempt_id,33) AS attempt_id,
        left(effect_key,129) AS effect_key,revision,recorded_at,
        left(provider_id,257) AS provider_id,
        left(credential_fingerprint,65) AS credential_fingerprint,left(status,16) AS status,
        CASE WHEN octet_length(snapshot::text)<=8192 THEN snapshot END AS snapshot,
        EXISTS(SELECT 1 FROM commerce_outbox e
          JOIN payment_attempts a ON a.id=payment_observations.attempt_id
          WHERE e.kind='payment.observation_recorded.v1'
            AND e.payload->>'observation_id'=payment_observations.id
            AND e.aggregate_id=a.order_id AND e.created_at=payment_observations.recorded_at
            AND e.payload=jsonb_build_object('schema_version',1,'order_id',a.order_id,
              'attempt_id',a.id,'observation_id',payment_observations.id)) AS event_present
        FROM payment_observations
        WHERE attempt_id=$1 AND id=$2"#,
    )
    .bind(&frozen.snapshot().binding.id)
    .bind(id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(PaymentStoreError::NotFound)?;
    row.decode(frozen, started_at)
}
pub(super) async fn latest(
    tx: &mut Transaction<'_, Postgres>,
    frozen: &FrozenAttempt,
    started_at: i64,
) -> Result<Option<ObservationSnapshot>, PaymentStoreError> {
    let row = sqlx::query_as::<_, Row>(
        r#"SELECT left(id,33) AS id,left(attempt_id,33) AS attempt_id,
        left(effect_key,129) AS effect_key,revision,recorded_at,
        left(provider_id,257) AS provider_id,
        left(credential_fingerprint,65) AS credential_fingerprint,left(status,16) AS status,
        CASE WHEN octet_length(snapshot::text)<=8192 THEN snapshot END AS snapshot,
        EXISTS(SELECT 1 FROM commerce_outbox e
          JOIN payment_attempts a ON a.id=payment_observations.attempt_id
          WHERE e.kind='payment.observation_recorded.v1'
            AND e.payload->>'observation_id'=payment_observations.id
            AND e.aggregate_id=a.order_id AND e.created_at=payment_observations.recorded_at
            AND e.payload=jsonb_build_object('schema_version',1,'order_id',a.order_id,
              'attempt_id',a.id,'observation_id',payment_observations.id)) AS event_present
        FROM payment_observations
        WHERE attempt_id=$1 ORDER BY revision DESC LIMIT 1"#,
    )
    .bind(&frozen.snapshot().binding.id)
    .fetch_optional(&mut **tx)
    .await?;
    row.map(|r| r.decode(frozen, started_at)).transpose()
}
pub(crate) async fn prefix(
    tx: &mut Transaction<'_, Postgres>,
    frozen: &FrozenAttempt,
    started_at: i64,
    through: i64,
) -> Result<Vec<ObservationSnapshot>, PaymentStoreError> {
    let rows = sqlx::query_as::<_, Row>(
        r#"SELECT left(id,33) AS id,left(attempt_id,33) AS attempt_id,
        left(effect_key,129) AS effect_key,revision,recorded_at,
        left(provider_id,257) AS provider_id,
        left(credential_fingerprint,65) AS credential_fingerprint,left(status,16) AS status,
        CASE WHEN octet_length(snapshot::text)<=8192 THEN snapshot END AS snapshot,
        EXISTS(SELECT 1 FROM commerce_outbox e
          JOIN payment_attempts a ON a.id=payment_observations.attempt_id
          WHERE e.kind='payment.observation_recorded.v1'
            AND e.payload->>'observation_id'=payment_observations.id
            AND e.aggregate_id=a.order_id AND e.created_at=payment_observations.recorded_at
            AND e.payload=jsonb_build_object('schema_version',1,'order_id',a.order_id,
              'attempt_id',a.id,'observation_id',payment_observations.id)) AS event_present
        FROM payment_observations
        WHERE attempt_id=$1 AND revision<=$2 ORDER BY revision LIMIT 1001"#,
    )
    .bind(&frozen.snapshot().binding.id)
    .bind(through)
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter()
        .map(|r| r.decode(frozen, started_at))
        .collect()
}

pub(super) async fn insert(
    tx: &mut Transaction<'_, Postgres>,
    s: &ObservationSnapshot,
    order_id: &str,
    event_id: &str,
) -> Result<(), PaymentStoreError> {
    let snapshot = serde_json::to_value(s).map_err(|_| PaymentStoreError::InvalidStoredData)?;
    let status = serde_json::to_value(s.intent.payment.status)
        .map_err(|_| PaymentStoreError::InvalidStoredData)?;
    let status = status
        .as_str()
        .ok_or(PaymentStoreError::InvalidStoredData)?;
    sqlx::query("INSERT INTO payment_observations(id,attempt_id,effect_key,revision,recorded_at,provider_id,credential_fingerprint,status,snapshot) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)")
        .bind(&s.id).bind(&s.attempt_id).bind(&s.effect_key).bind(s.revision).bind(s.recorded_at)
        .bind(&s.intent.payment.id).bind(&s.intent.credential_fingerprint).bind(status).bind(snapshot).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) VALUES($1,$2,'payment.observation_recorded.v1',$3,$4)")
        .bind(event_id).bind(order_id).bind(serde_json::json!({"schema_version":1,"order_id":order_id,"attempt_id":s.attempt_id,"observation_id":s.id})).bind(s.recorded_at).execute(&mut **tx).await?;
    Ok(())
}

impl Row {
    pub(super) fn decode(
        self,
        frozen: &FrozenAttempt,
        started_at: i64,
    ) -> Result<ObservationSnapshot, PaymentStoreError> {
        if !self.event_present {
            return Err(PaymentStoreError::InvalidStoredData);
        }
        let value = self.snapshot.ok_or(PaymentStoreError::InvalidStoredData)?;
        if !value.is_object()
            || !value.get("intent").is_some_and(Value::is_object)
            || !value
                .get("intent")
                .and_then(|v| v.get("payment"))
                .is_some_and(Value::is_object)
        {
            return Err(PaymentStoreError::InvalidStoredData);
        }
        let s: ObservationSnapshot =
            serde_json::from_value(value).map_err(|_| PaymentStoreError::InvalidStoredData)?;
        s.validate(frozen.snapshot(), started_at)
            .map_err(|_| PaymentStoreError::InvalidStoredData)?;
        let status = serde_json::to_value(s.intent.payment.status)
            .map_err(|_| PaymentStoreError::InvalidStoredData)?;
        if s.id != self.id
            || s.attempt_id != self.attempt_id
            || s.effect_key != self.effect_key
            || s.revision != self.revision
            || s.recorded_at != self.recorded_at
            || s.intent.payment.id != self.provider_id
            || s.intent.credential_fingerprint != self.credential_fingerprint
            || status.as_str() != Some(self.status.as_str())
        {
            return Err(PaymentStoreError::InvalidStoredData);
        }
        Ok(s)
    }
}

#[cfg(test)]
#[path = "../../../tests/support/payment_observation_rows.rs"]
mod tests;
