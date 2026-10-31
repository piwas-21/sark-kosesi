use super::*;
use serde_json::Value;
#[derive(sqlx::FromRow)]
struct Row {
    id: String,
    attempt_id: String,
    effect_key: String,
    revision: i64,
    page_number: i64,
    recorded_at: i64,
    intent: Option<Value>,
    snapshot: Option<Value>,
    event_present: bool,
}
async fn one(
    tx: &mut Transaction<'_, Postgres>,
    frozen: &FrozenAttempt,
    started_at: i64,
    effect: Option<&str>,
    id: Option<&str>,
) -> Result<Option<SearchSnapshot>, PaymentStoreError> {
    let row = sqlx::query_as::<_,Row>(r#"SELECT left(id,33) AS id,left(attempt_id,33) AS attempt_id,
        left(effect_key,129) AS effect_key,revision,page_number,recorded_at,
        CASE WHEN octet_length(snapshot::text)<=8192 THEN snapshot END AS snapshot,
        CASE WHEN octet_length(candidates::text)<=8192 THEN jsonb_build_object(
          'previous_page_id',left(previous_page_id,33),'from',left(from_cursor,257),'next',left(next_cursor,257),
          'candidates',candidates,'credential_fingerprint',left(credential_fingerprint,65)) END AS intent,
        EXISTS(SELECT 1 FROM commerce_outbox e JOIN payment_attempts a ON a.id=payment_search_pages.attempt_id
          WHERE e.kind='payment.search_page_recorded.v1' AND e.payload->>'page_id'=payment_search_pages.id
          AND e.aggregate_id=a.order_id AND e.created_at=payment_search_pages.recorded_at
          AND e.payload=jsonb_build_object('schema_version',1,'order_id',a.order_id,'attempt_id',a.id,'page_id',payment_search_pages.id)) AS event_present
        FROM payment_search_pages WHERE attempt_id=$1 AND ($2::text IS NULL OR effect_key=$2)
        AND ($3::text IS NULL OR id=$3) ORDER BY revision DESC LIMIT 1"#)
        .bind(&frozen.snapshot().binding.id).bind(effect).bind(id).fetch_optional(&mut **tx).await?;
    row.map(|v| v.decode(frozen, started_at)).transpose()
}
pub(crate) async fn load(
    tx: &mut Transaction<'_, Postgres>,
    frozen: &FrozenAttempt,
    started_at: i64,
    effect: Option<&str>,
    id: Option<&str>,
) -> Result<Option<SearchSnapshot>, PaymentStoreError> {
    let snapshot = one(tx, frozen, started_at, effect, id).await?;
    if let Some(s) = &snapshot {
        validate_chain(tx, frozen, started_at, s).await?;
    }
    Ok(snapshot)
}
pub(super) async fn validate_chain(
    tx: &mut Transaction<'_, Postgres>,
    frozen: &FrozenAttempt,
    started_at: i64,
    s: &SearchSnapshot,
) -> Result<(), PaymentStoreError> {
    let mut current = s.clone();
    let mut cursors = std::collections::HashSet::new();
    if let Some(next) = &s.intent.next {
        cursors.insert(next.clone());
    }
    for _ in 0..commerce_domain::payment_attempts::searches::MAX_SEARCH_PAGES {
        if let Some(from) = &current.intent.from
            && !cursors.insert(from.clone())
        {
            return Err(PaymentStoreError::InvalidStoredData);
        }
        let Some(id) = &current.intent.previous_page_id else {
            return Ok(());
        };
        let previous = one(tx, frozen, started_at, None, Some(id))
            .await?
            .ok_or(PaymentStoreError::InvalidStoredData)?;
        if !current.follows(&previous) {
            return Err(PaymentStoreError::InvalidStoredData);
        }
        current = previous;
    }
    Err(PaymentStoreError::InvalidStoredData)
}
pub(super) async fn insert(
    tx: &mut Transaction<'_, Postgres>,
    s: &SearchSnapshot,
    order_id: &str,
    event_id: &str,
) -> Result<(), PaymentStoreError> {
    let snapshot = serde_json::to_value(s).map_err(|_| PaymentStoreError::InvalidStoredData)?;
    let candidates = serde_json::to_value(&s.intent.candidates)
        .map_err(|_| PaymentStoreError::InvalidStoredData)?;
    sqlx::query("INSERT INTO payment_search_pages(id,attempt_id,effect_key,revision,page_number,recorded_at,previous_page_id,from_cursor,next_cursor,candidates,credential_fingerprint,snapshot) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)")
        .bind(&s.id).bind(&s.attempt_id).bind(&s.effect_key).bind(s.revision).bind(s.page_number).bind(s.recorded_at)
        .bind(&s.intent.previous_page_id).bind(&s.intent.from).bind(&s.intent.next).bind(candidates).bind(&s.intent.credential_fingerprint).bind(snapshot).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) VALUES($1,$2,'payment.search_page_recorded.v1',$3,$4)")
        .bind(event_id).bind(order_id).bind(serde_json::json!({"schema_version":1,"order_id":order_id,"attempt_id":s.attempt_id,"page_id":s.id})).bind(s.recorded_at).execute(&mut **tx).await?;
    Ok(())
}
impl Row {
    fn decode(
        self,
        frozen: &FrozenAttempt,
        started_at: i64,
    ) -> Result<SearchSnapshot, PaymentStoreError> {
        let value = self.snapshot.ok_or(PaymentStoreError::InvalidStoredData)?;
        if !self.event_present
            || !value.is_object()
            || !value.get("intent").is_some_and(Value::is_object)
        {
            return Err(PaymentStoreError::InvalidStoredData);
        }
        let s: SearchSnapshot =
            serde_json::from_value(value).map_err(|_| PaymentStoreError::InvalidStoredData)?;
        s.validate(frozen.snapshot(), started_at)
            .map_err(|_| PaymentStoreError::InvalidStoredData)?;
        let intent =
            serde_json::to_value(&s.intent).map_err(|_| PaymentStoreError::InvalidStoredData)?;
        if s.id != self.id
            || s.attempt_id != self.attempt_id
            || s.effect_key != self.effect_key
            || s.revision != self.revision
            || s.page_number != self.page_number
            || s.recorded_at != self.recorded_at
            || Some(intent) != self.intent
        {
            return Err(PaymentStoreError::InvalidStoredData);
        }
        Ok(s)
    }
}

#[cfg(test)]
#[path = "../../../tests/support/payment_search_rows.rs"]
mod tests;
