//! Shared private historical-evidence context, never current commercial authority.
use super::*;
use commerce_application::request_context::{Clock, Identifiers};
use commerce_domain::{checkout_quotes::MAX_QUOTE_UNIX_SECONDS, payment_attempts::DispatchState};
pub(super) fn input(attempt_id: &str, effect: &str) -> Result<(), PaymentStoreError> {
    if !identifier(attempt_id) || !valid_command_id(effect) {
        return Err(PaymentStoreError::InvalidRequest);
    }
    Ok(())
}
pub(super) async fn limits(tx: &mut Transaction<'_, Postgres>) -> Result<(), PaymentStoreError> {
    // A claim wait must not retain a connection-default repeatable-read snapshot.
    sqlx::query("SET TRANSACTION ISOLATION LEVEL READ COMMITTED")
        .execute(&mut **tx)
        .await?;
    sqlx::query("SET LOCAL lock_timeout='1s';")
        .execute(&mut **tx)
        .await?;
    sqlx::query("SET LOCAL statement_timeout='2s';")
        .execute(&mut **tx)
        .await?;
    Ok(())
}
pub(crate) async fn context(
    tx: &mut Transaction<'_, Postgres>,
    id: &str,
    claim: bool,
) -> Result<(FrozenAttempt, i64), PaymentStoreError> {
    let owner: Option<String> = if claim {
        sqlx::query_scalar("SELECT left(basket_id,33) FROM payment_attempts WHERE id=$1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut **tx)
            .await?
    } else {
        sqlx::query_scalar("SELECT left(basket_id,33) FROM payment_attempts WHERE id=$1")
            .bind(id)
            .fetch_optional(&mut **tx)
            .await?
    };
    let frozen = stored::by_id(tx, &owner.ok_or(PaymentStoreError::NotFound)?, id).await?;
    let (start_id, started_at): (String, i64) = sqlx::query_as(
        "SELECT left(id,33),started_at FROM payment_dispatch_starts WHERE attempt_id=$1",
    )
    .bind(id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(PaymentStoreError::Unavailable)?;
    if !identifier(&start_id)
        || (DispatchState::Started { started_at })
            .validate(frozen.snapshot())
            .is_err()
    {
        return Err(PaymentStoreError::InvalidStoredData);
    }
    Ok((frozen, started_at))
}
pub(super) fn now(clock: &dyn Clock) -> Result<i64, PaymentStoreError> {
    let at = i64::try_from(
        clock
            .unix_seconds()
            .map_err(|_| PaymentStoreError::ClockUnavailable)?,
    )
    .map_err(|_| PaymentStoreError::ClockUnavailable)?;
    if at > MAX_QUOTE_UNIX_SECONDS {
        return Err(PaymentStoreError::ClockUnavailable);
    }
    Ok(at)
}
pub(super) fn fresh_id(ids: &dyn Identifiers) -> Result<String, PaymentStoreError> {
    ids.next()
        .map(|v| format!("{:032x}", u128::from_be_bytes(v)))
        .map_err(|_| {
            PaymentStoreError::Boundary(checkout_orders::OrderStoreError::EntropyUnavailable)
        })
}
