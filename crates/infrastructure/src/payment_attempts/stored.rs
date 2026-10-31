use super::*;
mod decode;
#[derive(sqlx::FromRow)]
pub(super) struct Row {
    id: String,
    order_id: String,
    basket_id: String,
    effect_key: String,
    idempotency_key: String,
    created_at: i64,
    dispatch_before: i64,
    intent: Option<serde_json::Value>,
    snapshot: Option<serde_json::Value>,
    configuration: Option<serde_json::Value>,
    material: Option<String>,
    body: Option<String>,
}
pub(super) async fn by_effect(
    tx: &mut Transaction<'_, Postgres>,
    owner: &str,
    key: &str,
) -> Result<Option<FrozenAttempt>, PaymentStoreError> {
    let row = sqlx::query_as::<_,Row>("SELECT left(id,33) AS id,left(order_id,33) AS order_id,left(basket_id,33) AS basket_id,left(effect_key,129) AS effect_key,left(idempotency_key,65) AS idempotency_key,created_at,dispatch_before,CASE WHEN octet_length(intent::text)<=1024 THEN intent END AS intent,CASE WHEN octet_length(snapshot::text)<=16384 THEN snapshot END AS snapshot,CASE WHEN octet_length(configuration::text)<=8192 THEN configuration END AS configuration,CASE WHEN octet_length(configuration_material)<=8192 THEN configuration_material END AS material,CASE WHEN octet_length(request_body)<=8192 THEN request_body END AS body FROM payment_attempts WHERE basket_id=$1 AND effect_key=$2").bind(owner).bind(key).fetch_optional(&mut **tx).await?;
    match row {
        Some(row) => Ok(Some(restore(tx, row).await?)),
        None => Ok(None),
    }
}
pub(super) async fn by_order(
    tx: &mut Transaction<'_, Postgres>,
    owner: &str,
    id: &str,
) -> Result<Option<FrozenAttempt>, PaymentStoreError> {
    let row = sqlx::query_as::<_,Row>("SELECT left(id,33) AS id,left(order_id,33) AS order_id,left(basket_id,33) AS basket_id,left(effect_key,129) AS effect_key,left(idempotency_key,65) AS idempotency_key,created_at,dispatch_before,CASE WHEN octet_length(intent::text)<=1024 THEN intent END AS intent,CASE WHEN octet_length(snapshot::text)<=16384 THEN snapshot END AS snapshot,CASE WHEN octet_length(configuration::text)<=8192 THEN configuration END AS configuration,CASE WHEN octet_length(configuration_material)<=8192 THEN configuration_material END AS material,CASE WHEN octet_length(request_body)<=8192 THEN request_body END AS body FROM payment_attempts WHERE basket_id=$1 AND order_id=$2").bind(owner).bind(id).fetch_optional(&mut **tx).await?;
    match row {
        Some(row) => Ok(Some(restore(tx, row).await?)),
        None => Ok(None),
    }
}
pub(super) async fn by_id(
    tx: &mut Transaction<'_, Postgres>,
    owner: &str,
    id: &str,
) -> Result<FrozenAttempt, PaymentStoreError> {
    let row=sqlx::query_as::<_,Row>("SELECT left(id,33) AS id,left(order_id,33) AS order_id,left(basket_id,33) AS basket_id,left(effect_key,129) AS effect_key,left(idempotency_key,65) AS idempotency_key,created_at,dispatch_before,CASE WHEN octet_length(intent::text)<=1024 THEN intent END AS intent,CASE WHEN octet_length(snapshot::text)<=16384 THEN snapshot END AS snapshot,CASE WHEN octet_length(configuration::text)<=8192 THEN configuration END AS configuration,CASE WHEN octet_length(configuration_material)<=8192 THEN configuration_material END AS material,CASE WHEN octet_length(request_body)<=8192 THEN request_body END AS body FROM payment_attempts WHERE basket_id=$1 AND id=$2").bind(owner).bind(id).fetch_optional(&mut **tx).await?.ok_or(PaymentStoreError::NotFound)?;
    restore(tx, row).await
}
pub(super) async fn insert(
    tx: &mut Transaction<'_, Postgres>,
    frozen: &FrozenAttempt,
    configuration: &Configuration,
    intent: &PreparationIntent,
    effect: &str,
    event_id: &str,
) -> Result<(), PaymentStoreError> {
    let snapshot = frozen.snapshot();
    let b = &snapshot.binding;
    let value = serde_json::to_value(snapshot).map_err(|_| PaymentStoreError::InvalidStoredData)?;
    let policy = serde_json::to_value(configuration.snapshot())
        .map_err(|_| PaymentStoreError::InvalidStoredData)?;
    let intent = serde_json::to_value(intent).map_err(|_| PaymentStoreError::InvalidStoredData)?;
    sqlx::query("INSERT INTO payment_attempts(id,order_id,basket_id,effect_key,intent,idempotency_key,created_at,dispatch_before,snapshot,configuration,configuration_material,request_body) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)").bind(&b.id).bind(&b.order_id).bind(&b.basket_id).bind(effect).bind(intent).bind(snapshot.idempotency_key()).bind(b.created_at).bind(b.dispatch_before).bind(value).bind(policy).bind(configuration.fingerprint_material()).bind(frozen.body()).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) VALUES($1,$2,'payment.attempt_prepared.v1',$3,$4)").bind(event_id).bind(&b.order_id).bind(serde_json::json!({"schema_version":1,"order_id":b.order_id,"attempt_id":b.id})).bind(b.created_at).execute(&mut **tx).await?;
    Ok(())
}

async fn restore(
    tx: &mut Transaction<'_, Postgres>,
    row: Row,
) -> Result<FrozenAttempt, PaymentStoreError> {
    let frozen = row.decode()?;
    let b = &frozen.snapshot().binding;
    let order = checkout_orders::snapshot_in_transaction(tx, &b.basket_id, &b.order_id).await?;
    if b.stock_group_id != order.identity.stock_group_id
        || b.amount_minor != order.quote.total.gross_minor
        || b.created_at < order.identity.created_at
        || b.dispatch_before != order.identity.hold_expires_at
    {
        return Err(PaymentStoreError::InvalidStoredData);
    }
    Ok(frozen)
}
