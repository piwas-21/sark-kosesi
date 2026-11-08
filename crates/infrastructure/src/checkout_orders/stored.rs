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
    account: Option<i64>,
) -> Result<Option<(OrderSnapshot, OrderIntent)>, OrderStoreError> {
    let row = sqlx::query_as::<_,Row>("SELECT left(o.id,33) AS id,left(o.basket_id,33) AS basket_id,o.basket_revision,left(o.quote_id,33) AS quote_id,left(o.effect_key,129) AS effect_key,left(o.stock_group_id,33) AS stock_group_id,o.created_at,o.hold_expires_at,CASE WHEN octet_length(o.intent::text)<=16384 THEN o.intent END AS intent,CASE WHEN octet_length(o.snapshot::text)<=65536 THEN o.snapshot END AS snapshot,CASE WHEN octet_length(q.snapshot::text)<=32768 THEN q.snapshot END AS original_quote,CASE WHEN octet_length(p.policy::text)<=65536 THEN p.policy END AS original_market FROM checkout_orders o LEFT JOIN guest_quote_snapshots q ON q.id=o.quote_id LEFT JOIN market_policy_drafts p ON p.id=q.policy_id AND p.revision=q.policy_revision WHERE o.basket_id=$1 AND o.effect_key=$2")
 .bind(owner).bind(key).fetch_optional(&mut **tx).await?;
    match row {
        Some(row) => Ok(Some(verified_for(tx, row, account).await?)),
        None => Ok(None),
    }
}
pub(super) async fn by_quote(
    tx: &mut Transaction<'_, Postgres>,
    owner: &str,
    id: &str,
    account: Option<i64>,
) -> Result<Option<(OrderSnapshot, OrderIntent)>, OrderStoreError> {
    let row = sqlx::query_as::<_,Row>("SELECT left(o.id,33) AS id,left(o.basket_id,33) AS basket_id,o.basket_revision,left(o.quote_id,33) AS quote_id,left(o.effect_key,129) AS effect_key,left(o.stock_group_id,33) AS stock_group_id,o.created_at,o.hold_expires_at,CASE WHEN octet_length(o.intent::text)<=16384 THEN o.intent END AS intent,CASE WHEN octet_length(o.snapshot::text)<=65536 THEN o.snapshot END AS snapshot,CASE WHEN octet_length(q.snapshot::text)<=32768 THEN q.snapshot END AS original_quote,CASE WHEN octet_length(p.policy::text)<=65536 THEN p.policy END AS original_market FROM checkout_orders o LEFT JOIN guest_quote_snapshots q ON q.id=o.quote_id LEFT JOIN market_policy_drafts p ON p.id=q.policy_id AND p.revision=q.policy_revision WHERE o.basket_id=$1 AND o.quote_id=$2")
 .bind(owner).bind(id).fetch_optional(&mut **tx).await?;
    match row {
        Some(row) => Ok(Some(verified_for(tx, row, account).await?)),
        None => Ok(None),
    }
}
pub(crate) async fn by_id(
    tx: &mut Transaction<'_, Postgres>,
    owner: &str,
    id: &str,
) -> Result<OrderSnapshot, OrderStoreError> {
    Ok(by_id_with_account(tx, owner, id, OrderAccess::Internal)
        .await?
        .0)
}

pub(crate) async fn guest_by_id(
    tx: &mut Transaction<'_, Postgres>,
    owner: &str,
    id: &str,
) -> Result<OrderSnapshot, OrderStoreError> {
    by_id_for(tx, owner, id, None).await
}

pub(crate) async fn by_id_for(
    tx: &mut Transaction<'_, Postgres>,
    owner: &str,
    id: &str,
    account: Option<i64>,
) -> Result<OrderSnapshot, OrderStoreError> {
    Ok(
        by_id_with_account(tx, owner, id, OrderAccess::Owned(account))
            .await?
            .0,
    )
}

#[derive(Clone, Copy)]
pub(crate) enum OrderAccess {
    Internal,
    Owned(Option<i64>),
}

pub(crate) async fn by_id_with_account(
    tx: &mut Transaction<'_, Postgres>,
    owner: &str,
    id: &str,
    access: OrderAccess,
) -> Result<(OrderSnapshot, Option<i64>), OrderStoreError> {
    let row=sqlx::query_as::<_,Row>("SELECT left(o.id,33) AS id,left(o.basket_id,33) AS basket_id,o.basket_revision,left(o.quote_id,33) AS quote_id,left(o.effect_key,129) AS effect_key,left(o.stock_group_id,33) AS stock_group_id,o.created_at,o.hold_expires_at,CASE WHEN octet_length(o.intent::text)<=16384 THEN o.intent END AS intent,CASE WHEN octet_length(o.snapshot::text)<=65536 THEN o.snapshot END AS snapshot,CASE WHEN octet_length(q.snapshot::text)<=32768 THEN q.snapshot END AS original_quote,CASE WHEN octet_length(p.policy::text)<=65536 THEN p.policy END AS original_market FROM checkout_orders o LEFT JOIN guest_quote_snapshots q ON q.id=o.quote_id LEFT JOIN market_policy_drafts p ON p.id=q.policy_id AND p.revision=q.policy_revision WHERE o.basket_id=$1 AND o.id=$2")
 .bind(owner).bind(id).fetch_optional(&mut **tx).await?.ok_or(OrderStoreError::NotFound)?;
    let bound = quote_account(tx, &row.basket_id, &row.quote_id).await?;
    if let OrderAccess::Owned(account) = access
        && bound != account
    {
        return Err(OrderStoreError::NotFound);
    }
    Ok((verified(tx, row, bound).await?.0, bound))
}

async fn quote_account(
    tx: &mut Transaction<'_, Postgres>,
    owner: &str,
    id: &str,
) -> Result<Option<i64>, OrderStoreError> {
    let present: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_catalog.pg_attribute WHERE attrelid='guest_quote_snapshots'::regclass AND attname='promotion_account_id' AND attnum>0 AND NOT attisdropped)").fetch_one(&mut **tx).await?;
    if !present {
        return Ok(None);
    }
    sqlx::query_scalar::<_, Option<i64>>(
        "SELECT promotion_account_id FROM guest_quote_snapshots WHERE basket_id=$1 AND id=$2",
    )
    .bind(owner)
    .bind(id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(OrderStoreError::InvalidStoredData)
}

async fn verified_for(
    tx: &mut Transaction<'_, Postgres>,
    row: Row,
    account: Option<i64>,
) -> Result<(OrderSnapshot, OrderIntent), OrderStoreError> {
    let bound = quote_account(tx, &row.basket_id, &row.quote_id).await?;
    if bound != account {
        return Err(OrderStoreError::NotFound);
    }
    verified(tx, row, bound).await
}

async fn verified(
    tx: &mut Transaction<'_, Postgres>,
    row: Row,
    account: Option<i64>,
) -> Result<(OrderSnapshot, OrderIntent), OrderStoreError> {
    let (snapshot, intent) = row.decode()?;
    if account.is_some() && snapshot.promoted_quote.is_none() {
        return Err(OrderStoreError::InvalidStoredData);
    }
    if let Some(promoted) = &snapshot.promoted_quote {
        let quote = crate::checkout_quotes::by_id_for(
            tx,
            &snapshot.identity.basket_id,
            &snapshot.identity.quote_id,
            account,
        )
        .await?;
        match quote {
            crate::checkout_quotes::StoredQuote::Promoted(stored, _) if *stored == *promoted => {}
            _ => return Err(OrderStoreError::InvalidStoredData),
        }
        let reciprocal: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM promotion_redemptions r
             JOIN checkout_orders o ON o.id=r.order_id AND o.promotion_redemption_transaction_id=r.transaction_id
             WHERE r.order_id=$1 AND r.quote_id=$2 AND r.promotion_id=$3
             AND r.promotion_revision=$4 AND r.promotion_review_revision=$5
             AND r.discount_minor=$6 AND r.created_at=$7
             AND r.redemption_number BETWEEN 1 AND 1000000
             AND r.lifetime_discount_minor BETWEEN r.discount_minor AND $8)",
        )
            .bind(&snapshot.identity.id).bind(&snapshot.identity.quote_id).bind(&promoted.promotion.id)
            .bind(promoted.promotion.revision).bind(promoted.promotion.review_revision).bind(promoted.discount_minor)
            .bind(snapshot.identity.created_at)
            .bind(commerce_domain::quotes::MAX_MINOR_UNITS)
            .fetch_one(&mut **tx).await?;
        if !reciprocal {
            return Err(OrderStoreError::InvalidStoredData);
        }
        let has_account: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_catalog.pg_attribute WHERE attrelid='promotion_redemptions'::regclass AND attname='promotion_account_id' AND attnum>0 AND NOT attisdropped)").fetch_one(&mut **tx).await?;
        if has_account {
            let matching: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM promotion_redemptions WHERE order_id=$1 AND promotion_account_id IS NOT DISTINCT FROM $2 AND (($2::bigint IS NULL AND account_redemption_number IS NULL) OR ($2::bigint IS NOT NULL AND account_redemption_number BETWEEN 1 AND 1000000)))")
                .bind(&snapshot.identity.id).bind(account).fetch_one(&mut **tx).await?;
            if !matching {
                return Err(OrderStoreError::InvalidStoredData);
            }
        } else if account.is_some() {
            return Err(OrderStoreError::InvalidStoredData);
        }
    }
    Ok((snapshot, intent))
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
    // Values are already valid JSON; Display encodes the same UTF-8 JSON envelope
    // without introducing impossible JSON-Value serialization error branches.
    if s.to_string().len() > 65536 || i.to_string().len() > 16384 {
        return Err(OrderStoreError::InvalidRequest);
    }
    let id = &snapshot.identity;
    sqlx::query("INSERT INTO checkout_orders(id,basket_id,basket_revision,quote_id,effect_key,stock_group_id,created_at,hold_expires_at,intent,snapshot) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)")
 .bind(&id.id).bind(&id.basket_id).bind(intent.expected_basket_revision).bind(&id.quote_id).bind(effect).bind(&id.stock_group_id).bind(id.created_at).bind(id.hold_expires_at).bind(i).bind(s).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) VALUES($1,$2,'checkout.order_created.v1',$3,$4)")
 .bind(event).bind(&id.id).bind(serde_json::json!({"schema_version":1,"order_id":id.id})).bind(id.created_at).execute(&mut **tx).await?;
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/support/order_stored.rs"]
mod tests;
