use super::{CAPACITY, RestockError};
use crate::customer_sessions::SessionError;
use commerce_application::request_context::Clock;
use commerce_domain::catalogue_authoring::merchant_code;
use sqlx::{Postgres, Transaction};

#[derive(Clone, Debug, PartialEq, Eq, sqlx::FromRow)]
pub(super) struct Membership {
    pub(super) sku: String,
    pub(super) generation: i64,
    pub(super) opted_in_at: i64,
}

pub(super) fn failure(_: impl Sized) -> RestockError {
    RestockError::Unavailable
}
pub(super) fn session_failure(error: SessionError) -> RestockError {
    match error {
        SessionError::Denied => RestockError::Denied,
        SessionError::Unavailable => RestockError::Unavailable,
    }
}
pub(super) fn now(clock: &dyn Clock) -> Result<i64, RestockError> {
    clock
        .unix_seconds()
        .ok()
        .and_then(|v| i64::try_from(v).ok())
        .ok_or(RestockError::Unavailable)
}
pub(super) async fn bucket(
    tx: &mut Transaction<'_, Postgres>,
    account: i64,
) -> Result<i64, RestockError> {
    sqlx::query(
        "INSERT INTO customer_restock_buckets(account_id) VALUES ($1) ON CONFLICT DO NOTHING",
    )
    .bind(account)
    .execute(&mut **tx)
    .await
    .map_err(failure)?;
    sqlx::query_scalar(
        "SELECT revision FROM customer_restock_buckets WHERE account_id=$1 FOR UPDATE",
    )
    .bind(account)
    .fetch_one(&mut **tx)
    .await
    .map_err(failure)
}
pub(super) async fn saved(
    tx: &mut Transaction<'_, Postgres>,
    account: i64,
    revision: i64,
) -> Result<Vec<Membership>, RestockError> {
    let entries: Vec<Membership> = sqlx::query_as("SELECT sku,generation,opted_in_at FROM customer_restock_subscriptions WHERE account_id=$1 ORDER BY sku COLLATE \"C\" LIMIT 26")
        .bind(account).fetch_all(&mut **tx).await.map_err(failure)?;
    if revision < 0
        || entries.len() > CAPACITY
        || entries.iter().any(|e| {
            !merchant_code(&e.sku)
                || e.generation <= 0
                || e.generation > revision
                || e.opted_in_at < 0
        })
    {
        return Err(RestockError::Unavailable);
    }
    Ok(entries)
}
