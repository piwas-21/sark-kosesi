//! Closed journal and pointer decoding, including original resolution custody.
use super::*;
#[derive(sqlx::FromRow)]
struct Row {
    id: String,
    order_id: String,
    attempt_id: String,
    resolution_id: String,
    through_observation: i64,
    recorded_at: i64,
    state: String,
    currency: String,
    amount_minor: i64,
    event_present: bool,
    monotonic: bool,
}
pub(super) async fn load(
    tx: &mut Transaction<'_, Postgres>,
    frozen: &FrozenAttempt,
    started: i64,
    through: Option<i64>,
) -> Result<Option<Recognition>, PaymentStoreError> {
    let row = sqlx::query_as::<_, Row>(r#"SELECT left(r.id,33) AS id,
      left(r.order_id,33) AS order_id,left(r.attempt_id,33) AS attempt_id,
      left(r.resolution_id,33) AS resolution_id,r.through_observation,r.recorded_at,
      left(r.state,16) AS state,left(r.currency,4) AS currency,r.amount_minor,
      EXISTS(SELECT 1 FROM commerce_outbox e WHERE e.aggregate_id=r.order_id
        AND e.kind='payment.recognized.v1' AND e.created_at=r.recorded_at
        AND e.payload=jsonb_build_object('schema_version',1,'order_id',r.order_id,
          'attempt_id',r.attempt_id,'recognition_id',r.id,
          'recognition_sha256',payment_recognition_digest(r))) AS event_present,
      NOT EXISTS(SELECT 1 FROM payment_recognitions previous WHERE previous.order_id=r.order_id
        AND previous.through_observation<r.through_observation AND previous.recorded_at>r.recorded_at) AS monotonic
      FROM payment_recognitions r WHERE r.order_id=$1
        AND ($2::bigint IS NULL OR r.through_observation=$2)
      ORDER BY r.through_observation DESC LIMIT 1"#)
        .bind(&frozen.snapshot().binding.order_id).bind(through).fetch_optional(&mut **tx).await?;
    let Some(row) = row else {
        return Ok(None);
    };
    if !row.event_present
        || !row.monotonic
        || row.currency != "EUR"
        || !identifier(&row.resolution_id)
    {
        return Err(PaymentStoreError::InvalidStoredData);
    }
    let state = match row.state.as_str() {
        "unconfirmed" => State::Unconfirmed,
        "received" => State::Received,
        "needs_review" => State::NeedsReview,
        _ => return Err(PaymentStoreError::InvalidStoredData),
    };
    let resolution = super::super::identities::rows::by_id(tx, frozen, started, &row.resolution_id)
        .await?
        .ok_or(PaymentStoreError::InvalidStoredData)?;
    proof::ownership(tx, frozen, &resolution).await?;
    let recognition = Recognition {
        id: row.id,
        order_id: row.order_id,
        attempt_id: row.attempt_id,
        resolution_id: row.resolution_id,
        through_observation: row.through_observation,
        recorded_at: row.recorded_at,
        state,
        currency: commerce_domain::Currency::EUR,
        amount_minor: row.amount_minor,
    };
    recognition
        .validate(
            frozen.snapshot(),
            &resolution.summary,
            resolution.through_observation,
            resolution.recorded_at,
        )
        .map_err(|_| PaymentStoreError::InvalidStoredData)?;
    Ok(Some(recognition))
}
pub(super) async fn pointer(
    tx: &mut Transaction<'_, Postgres>,
    frozen: &FrozenAttempt,
    started: i64,
) -> Result<Option<Recognition>, PaymentStoreError> {
    let pointer: Option<String> = sqlx::query_scalar(
        "SELECT left(recognition_id,33) FROM payment_recognition_current WHERE order_id=$1",
    )
    .bind(&frozen.snapshot().binding.order_id)
    .fetch_optional(&mut **tx)
    .await?;
    let latest = load(tx, frozen, started, None).await?;
    if pointer.as_deref() != latest.as_ref().map(|recognition| recognition.id.as_str()) {
        return Err(PaymentStoreError::InvalidStoredData);
    }
    Ok(latest)
}
pub(super) async fn insert(
    tx: &mut Transaction<'_, Postgres>,
    r: &Recognition,
    event: &str,
) -> Result<(), PaymentStoreError> {
    sqlx::query("INSERT INTO payment_recognitions(id,order_id,attempt_id,resolution_id,through_observation,recorded_at,state,currency,amount_minor) VALUES($1,$2,$3,$4,$5,$6,$7,'EUR',$8)")
        .bind(&r.id).bind(&r.order_id).bind(&r.attempt_id).bind(&r.resolution_id)
        .bind(r.through_observation).bind(r.recorded_at).bind(r.state.code()).bind(r.amount_minor).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO payment_recognition_current(order_id,recognition_id) VALUES($1,$2) ON CONFLICT(order_id) DO UPDATE SET recognition_id=EXCLUDED.recognition_id")
        .bind(&r.order_id).bind(&r.id).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) SELECT $1,r.order_id,'payment.recognized.v1',jsonb_build_object('schema_version',1,'order_id',r.order_id,'attempt_id',r.attempt_id,'recognition_id',r.id,'recognition_sha256',payment_recognition_digest(r)),r.recorded_at FROM payment_recognitions r WHERE r.id=$2")
        .bind(event).bind(&r.id).execute(&mut **tx).await?;
    Ok(())
}
