use super::*;
use commerce_domain::checkout_orders::OrderIdentity;

#[derive(sqlx::FromRow)]
struct Row {
    effect: String,
    account: Option<i64>,
    canceled_at: i64,
    event_id: String,
    current_transaction: bool,
    event_present: bool,
    attempt_id: Option<String>,
    attempt_valid: bool,
}

#[derive(PartialEq, Eq)]
pub(super) struct Stored {
    pub effect: String,
    pub canceled_at: i64,
    pub event_id: String,
    pub attempt_id: Option<String>,
    pub current_transaction: bool,
}

pub(super) async fn order_by_effect(
    tx: &mut Transaction<'_, Postgres>,
    owner: &str,
    effect: &str,
) -> Result<Option<String>, OrderStoreError> {
    Ok(sqlx::query_scalar("SELECT left(order_id,33) FROM checkout_order_cancellations WHERE basket_id=$1 AND effect_key=$2")
        .bind(owner).bind(effect).fetch_optional(&mut **tx).await?)
}

pub(super) async fn by_order(
    tx: &mut Transaction<'_, Postgres>,
    order: &OrderIdentity,
    account: Option<i64>,
) -> Result<Option<Stored>, OrderStoreError> {
    let row = sqlx::query_as::<_, Row>("SELECT left(c.effect_key,129) AS effect,c.promotion_account_id AS account,c.canceled_at,left(c.event_id,33) AS event_id,COALESCE(c.transaction_id=pg_current_xact_id_if_assigned(),false) AS current_transaction,EXISTS(SELECT 1 FROM commerce_outbox e WHERE e.id=c.event_id AND e.aggregate_id=c.order_id AND e.kind='checkout.order_canceled.v1' AND e.created_at=c.canceled_at AND e.payload=jsonb_build_object('schema_version',1,'order_id',c.order_id)) AS event_present,left(c.attempt_id,33) AS attempt_id,CASE WHEN c.attempt_id IS NULL THEN NOT EXISTS(SELECT 1 FROM payment_attempts a WHERE a.order_id=c.order_id) ELSE EXISTS(SELECT 1 FROM payment_attempts a JOIN commerce_outbox e ON e.aggregate_id=a.order_id AND e.kind='payment.attempt_prepared.v1' AND e.created_at=a.created_at AND e.payload=jsonb_build_object('schema_version',1,'order_id',a.order_id,'attempt_id',a.id) WHERE a.id=c.attempt_id AND a.order_id=c.order_id AND a.basket_id=c.basket_id AND a.promotion_account_id IS NOT DISTINCT FROM c.promotion_account_id AND a.created_at<=c.canceled_at AND NOT EXISTS(SELECT 1 FROM payment_dispatch_starts s WHERE s.attempt_id=a.id)) END AS attempt_valid FROM checkout_order_cancellations c WHERE c.basket_id=$1 AND c.order_id=$2")
        .bind(&order.basket_id).bind(&order.id).fetch_optional(&mut **tx).await?;
    let Some(row) = row else {
        return Ok(None);
    };
    if row.account != account
        || !identifier(&row.event_id)
        || !valid_command_id(&row.effect)
        || !(order.created_at..=commerce_domain::checkout_quotes::MAX_QUOTE_UNIX_SECONDS)
            .contains(&row.canceled_at)
        || !row.event_present
        || !row.attempt_valid
        || row.attempt_id.as_ref().is_some_and(|id| !identifier(id))
    {
        return Err(OrderStoreError::InvalidStoredData);
    }
    Ok(Some(Stored {
        effect: row.effect,
        canceled_at: row.canceled_at,
        event_id: row.event_id,
        attempt_id: row.attempt_id,
        current_transaction: row.current_transaction,
    }))
}

pub(super) async fn claim_undispatched_attempt(
    tx: &mut Transaction<'_, Postgres>,
    order_id: &str,
) -> Result<Option<(String, i64)>, OrderStoreError> {
    let attempt: Option<(String, i64)> = sqlx::query_as(
        "SELECT left(id,33),created_at FROM payment_attempts WHERE order_id=$1 FOR UPDATE",
    )
    .bind(order_id)
    .fetch_optional(&mut **tx)
    .await?;
    if let Some((id, _)) = &attempt {
        if !identifier(id) {
            return Err(OrderStoreError::InvalidStoredData);
        }
        let started: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM payment_dispatch_starts WHERE attempt_id=$1)",
        )
        .bind(id)
        .fetch_one(&mut **tx)
        .await?;
        if started {
            return Err(OrderStoreError::Conflict);
        }
    }
    Ok(attempt)
}

pub(super) async fn insert(
    tx: &mut Transaction<'_, Postgres>,
    owner: &str,
    order_id: &str,
    effect: &str,
    account: Option<i64>,
    canceled_at: i64,
    event: &str,
    attempt_id: Option<&str>,
) -> Result<(), OrderStoreError> {
    sqlx::query("INSERT INTO checkout_order_cancellations(order_id,basket_id,effect_key,promotion_account_id,canceled_at,event_id,attempt_id) VALUES($1,$2,$3,$4,$5,$6,$7)")
        .bind(order_id).bind(owner).bind(effect).bind(account).bind(canceled_at).bind(event).bind(attempt_id)
        .execute(&mut **tx).await?;
    sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) VALUES($1,$2,'checkout.order_canceled.v1',$3,$4)")
        .bind(event).bind(order_id).bind(serde_json::json!({"schema_version":1,"order_id":order_id})).bind(canceled_at)
        .execute(&mut **tx).await?;
    Ok(())
}
