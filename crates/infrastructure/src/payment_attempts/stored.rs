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
    account: Option<i64>,
) -> Result<Option<FrozenAttempt>, PaymentStoreError> {
    let row = sqlx::query_as::<_,Row>("SELECT left(id,33) AS id,left(order_id,33) AS order_id,left(basket_id,33) AS basket_id,left(effect_key,129) AS effect_key,left(idempotency_key,65) AS idempotency_key,created_at,dispatch_before,CASE WHEN octet_length(intent::text)<=1024 THEN intent END AS intent,CASE WHEN octet_length(snapshot::text)<=16384 THEN snapshot END AS snapshot,CASE WHEN octet_length(configuration::text)<=8192 THEN configuration END AS configuration,CASE WHEN octet_length(configuration_material)<=8192 THEN configuration_material END AS material,CASE WHEN octet_length(request_body)<=8192 THEN request_body END AS body FROM payment_attempts WHERE basket_id=$1 AND effect_key=$2").bind(owner).bind(key).fetch_optional(&mut **tx).await?;
    match row {
        Some(row) => Ok(Some(
            restore(tx, row, checkout_orders::OrderAccess::Owned(account))
                .await
                .map_err(|error| match error {
                    PaymentStoreError::NotFound => {
                        PaymentStoreError::Boundary(checkout_orders::OrderStoreError::NotFound)
                    }
                    other => other,
                })?,
        )),
        None => Ok(None),
    }
}
pub(super) async fn by_order(
    tx: &mut Transaction<'_, Postgres>,
    owner: &str,
    id: &str,
    account: Option<i64>,
) -> Result<Option<FrozenAttempt>, PaymentStoreError> {
    let row = sqlx::query_as::<_,Row>("SELECT left(id,33) AS id,left(order_id,33) AS order_id,left(basket_id,33) AS basket_id,left(effect_key,129) AS effect_key,left(idempotency_key,65) AS idempotency_key,created_at,dispatch_before,CASE WHEN octet_length(intent::text)<=1024 THEN intent END AS intent,CASE WHEN octet_length(snapshot::text)<=16384 THEN snapshot END AS snapshot,CASE WHEN octet_length(configuration::text)<=8192 THEN configuration END AS configuration,CASE WHEN octet_length(configuration_material)<=8192 THEN configuration_material END AS material,CASE WHEN octet_length(request_body)<=8192 THEN request_body END AS body FROM payment_attempts WHERE basket_id=$1 AND order_id=$2").bind(owner).bind(id).fetch_optional(&mut **tx).await?;
    match row {
        Some(row) => Ok(Some(
            restore(tx, row, checkout_orders::OrderAccess::Owned(account)).await?,
        )),
        None => Ok(None),
    }
}
pub(super) async fn by_id(
    tx: &mut Transaction<'_, Postgres>,
    owner: &str,
    id: &str,
    access: checkout_orders::OrderAccess,
) -> Result<FrozenAttempt, PaymentStoreError> {
    let row=sqlx::query_as::<_,Row>("SELECT left(id,33) AS id,left(order_id,33) AS order_id,left(basket_id,33) AS basket_id,left(effect_key,129) AS effect_key,left(idempotency_key,65) AS idempotency_key,created_at,dispatch_before,CASE WHEN octet_length(intent::text)<=1024 THEN intent END AS intent,CASE WHEN octet_length(snapshot::text)<=16384 THEN snapshot END AS snapshot,CASE WHEN octet_length(configuration::text)<=8192 THEN configuration END AS configuration,CASE WHEN octet_length(configuration_material)<=8192 THEN configuration_material END AS material,CASE WHEN octet_length(request_body)<=8192 THEN request_body END AS body FROM payment_attempts WHERE basket_id=$1 AND id=$2").bind(owner).bind(id).fetch_optional(&mut **tx).await?.ok_or(PaymentStoreError::NotFound)?;
    restore(tx, row, access).await
}

pub(super) async fn insert(
    tx: &mut Transaction<'_, Postgres>,
    frozen: &FrozenAttempt,
    configuration: &Configuration,
    intent: &PreparationIntent,
    effect: &str,
    event_id: &str,
    account: Option<i64>,
) -> Result<(), PaymentStoreError> {
    let snapshot = frozen.snapshot();
    let b = &snapshot.binding;
    let value = serde_json::to_value(snapshot).map_err(|_| PaymentStoreError::InvalidStoredData)?;
    let policy = serde_json::to_value(configuration.snapshot())
        .map_err(|_| PaymentStoreError::InvalidStoredData)?;
    let intent = serde_json::to_value(intent).map_err(|_| PaymentStoreError::InvalidStoredData)?;
    let insert = if account.is_some() {
        "INSERT INTO payment_attempts(id,order_id,basket_id,effect_key,intent,idempotency_key,created_at,dispatch_before,snapshot,configuration,configuration_material,request_body,promotion_account_id) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)"
    } else {
        "INSERT INTO payment_attempts(id,order_id,basket_id,effect_key,intent,idempotency_key,created_at,dispatch_before,snapshot,configuration,configuration_material,request_body) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)"
    };
    let query = sqlx::query(insert)
        .bind(&b.id)
        .bind(&b.order_id)
        .bind(&b.basket_id)
        .bind(effect)
        .bind(intent)
        .bind(snapshot.idempotency_key())
        .bind(b.created_at)
        .bind(b.dispatch_before)
        .bind(value)
        .bind(policy)
        .bind(configuration.fingerprint_material())
        .bind(frozen.body());
    let query = if let Some(account) = account {
        query.bind(account)
    } else {
        query
    };
    query.execute(&mut **tx).await?;
    sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) VALUES($1,$2,'payment.attempt_prepared.v1',$3,$4)").bind(event_id).bind(&b.order_id).bind(serde_json::json!({"schema_version":1,"order_id":b.order_id,"attempt_id":b.id})).bind(b.created_at).execute(&mut **tx).await?;
    Ok(())
}

async fn restore(
    tx: &mut Transaction<'_, Postgres>,
    row: Row,
    access: checkout_orders::OrderAccess,
) -> Result<FrozenAttempt, PaymentStoreError> {
    let (order, order_account) = checkout_orders::snapshot_with_account_in_transaction(
        tx,
        &row.basket_id,
        &row.order_id,
        access,
    )
    .await
    .map_err(|error| match (access, error) {
        (checkout_orders::OrderAccess::Owned(_), checkout_orders::OrderStoreError::NotFound) => {
            PaymentStoreError::NotFound
        }
        (_, other) => other.into(),
    })?;
    let has_account: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_catalog.pg_attribute WHERE attrelid='payment_attempts'::regclass AND attname='promotion_account_id' AND attnum>0 AND NOT attisdropped)").fetch_one(&mut **tx).await?;
    let attempt_account = if has_account {
        sqlx::query_scalar::<_, Option<i64>>(
            "SELECT promotion_account_id FROM payment_attempts WHERE id=$1 AND basket_id=$2",
        )
        .bind(&row.id)
        .bind(&row.basket_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(PaymentStoreError::InvalidStoredData)?
    } else {
        None
    };
    if attempt_account != order_account {
        return Err(PaymentStoreError::InvalidStoredData);
    }
    let frozen = row.decode()?;
    let b = &frozen.snapshot().binding;
    if b.stock_group_id != order.identity.stock_group_id
        || b.amount_minor != order.payable_total().gross_minor
        || b.created_at < order.identity.created_at
        || b.dispatch_before != order.identity.hold_expires_at
    {
        return Err(PaymentStoreError::InvalidStoredData);
    }
    Ok(frozen)
}
