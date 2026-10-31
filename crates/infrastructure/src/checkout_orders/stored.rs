use super::*;
mod decode;
#[derive(sqlx::FromRow)]
pub(super) struct Row {
    id: String,
    basket_id: String,
    basket_revision: i32,
    quote_id: String,
    effect_key: String,
    stock_group_id: String,
    created_at: i64,
    hold_expires_at: i64,
    intent: Option<serde_json::Value>,
    snapshot: Option<serde_json::Value>,
    original_quote: Option<serde_json::Value>,
    original_market: Option<serde_json::Value>,
}
pub(super) async fn by_effect(
    tx: &mut Transaction<'_, Postgres>,
    owner: &str,
    key: &str,
) -> Result<Option<(OrderSnapshot, OrderIntent)>, OrderStoreError> {
    sqlx::query_as::<_,Row>("SELECT left(o.id,33) AS id,left(o.basket_id,33) AS basket_id,o.basket_revision,left(o.quote_id,33) AS quote_id,left(o.effect_key,129) AS effect_key,left(o.stock_group_id,33) AS stock_group_id,o.created_at,o.hold_expires_at,CASE WHEN octet_length(o.intent::text)<=16384 THEN o.intent END AS intent,CASE WHEN octet_length(o.snapshot::text)<=65536 THEN o.snapshot END AS snapshot,CASE WHEN octet_length(q.snapshot::text)<=32768 THEN q.snapshot END AS original_quote,CASE WHEN octet_length(p.policy::text)<=65536 THEN p.policy END AS original_market FROM checkout_orders o LEFT JOIN guest_quote_snapshots q ON q.id=o.quote_id LEFT JOIN market_policy_drafts p ON p.id=q.policy_id AND p.revision=q.policy_revision WHERE o.basket_id=$1 AND o.effect_key=$2")
 .bind(owner).bind(key).fetch_optional(&mut **tx).await?.map(Row::decode).transpose()
}
pub(super) async fn by_quote(
    tx: &mut Transaction<'_, Postgres>,
    owner: &str,
    id: &str,
) -> Result<Option<(OrderSnapshot, OrderIntent)>, OrderStoreError> {
    sqlx::query_as::<_,Row>("SELECT left(o.id,33) AS id,left(o.basket_id,33) AS basket_id,o.basket_revision,left(o.quote_id,33) AS quote_id,left(o.effect_key,129) AS effect_key,left(o.stock_group_id,33) AS stock_group_id,o.created_at,o.hold_expires_at,CASE WHEN octet_length(o.intent::text)<=16384 THEN o.intent END AS intent,CASE WHEN octet_length(o.snapshot::text)<=65536 THEN o.snapshot END AS snapshot,CASE WHEN octet_length(q.snapshot::text)<=32768 THEN q.snapshot END AS original_quote,CASE WHEN octet_length(p.policy::text)<=65536 THEN p.policy END AS original_market FROM checkout_orders o LEFT JOIN guest_quote_snapshots q ON q.id=o.quote_id LEFT JOIN market_policy_drafts p ON p.id=q.policy_id AND p.revision=q.policy_revision WHERE o.basket_id=$1 AND o.quote_id=$2")
 .bind(owner).bind(id).fetch_optional(&mut **tx).await?.map(Row::decode).transpose()
}
pub(super) async fn by_id(
    tx: &mut Transaction<'_, Postgres>,
    owner: &str,
    id: &str,
) -> Result<OrderSnapshot, OrderStoreError> {
    let row=sqlx::query_as::<_,Row>("SELECT left(o.id,33) AS id,left(o.basket_id,33) AS basket_id,o.basket_revision,left(o.quote_id,33) AS quote_id,left(o.effect_key,129) AS effect_key,left(o.stock_group_id,33) AS stock_group_id,o.created_at,o.hold_expires_at,CASE WHEN octet_length(o.intent::text)<=16384 THEN o.intent END AS intent,CASE WHEN octet_length(o.snapshot::text)<=65536 THEN o.snapshot END AS snapshot,CASE WHEN octet_length(q.snapshot::text)<=32768 THEN q.snapshot END AS original_quote,CASE WHEN octet_length(p.policy::text)<=65536 THEN p.policy END AS original_market FROM checkout_orders o LEFT JOIN guest_quote_snapshots q ON q.id=o.quote_id LEFT JOIN market_policy_drafts p ON p.id=q.policy_id AND p.revision=q.policy_revision WHERE o.basket_id=$1 AND o.id=$2")
 .bind(owner).bind(id).fetch_optional(&mut **tx).await?.ok_or(OrderStoreError::NotFound)?;
    Ok(row.decode()?.0)
}
pub(super) async fn insert(
    tx: &mut Transaction<'_, Postgres>,
    snapshot: &OrderSnapshot,
    intent: &OrderIntent,
    effect: &str,
    event: &str,
) -> Result<(), OrderStoreError> {
    let s = serde_json::to_value(snapshot).map_err(|_| OrderStoreError::InvalidStoredData)?;
    let i = serde_json::to_value(intent).map_err(|_| OrderStoreError::InvalidStoredData)?;
    if serde_json::to_vec(&s)
        .map_err(|_| OrderStoreError::InvalidStoredData)?
        .len()
        > 65536
        || serde_json::to_vec(&i)
            .map_err(|_| OrderStoreError::InvalidStoredData)?
            .len()
            > 16384
    {
        return Err(OrderStoreError::InvalidRequest);
    }
    let id = &snapshot.identity;
    sqlx::query("INSERT INTO checkout_orders(id,basket_id,basket_revision,quote_id,effect_key,stock_group_id,created_at,hold_expires_at,intent,snapshot) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)")
 .bind(&id.id).bind(&id.basket_id).bind(intent.expected_basket_revision).bind(&id.quote_id).bind(effect).bind(&id.stock_group_id).bind(id.created_at).bind(id.hold_expires_at).bind(i).bind(s).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) VALUES($1,$2,'checkout.order_created.v1',$3,$4)")
 .bind(event).bind(&id.id).bind(serde_json::json!({"schema_version":1,"order_id":id.id})).bind(id.created_at).execute(&mut **tx).await?;
    Ok(())
}
