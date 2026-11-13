//! Revocation removes authority; it never issues or refreshes a staff session.
use super::{SessionToken, StaffError};
use commerce_application::request_context::{Clock, ObservedClock};
use sqlx::PgPool;

fn now(clock: &dyn Clock) -> Result<i64, StaffError> {
    clock
        .unix_seconds()
        .ok()
        .and_then(|at| i64::try_from(at).ok())
        .ok_or(StaffError::ClockUnavailable)
}

/// Revoke only the exact supplied bearer. Missing and already-revoked tokens are
/// indistinguishable successful no-ops. MFA freshness and capability are not
/// required to relinquish authority; this does not authenticate a staff actor.
pub async fn revoke_self(
    pool: &PgPool,
    token: &SessionToken,
    clock: &dyn Clock,
) -> Result<(), StaffError> {
    let observed = ObservedClock::new(clock, 0);
    now(&observed)?;
    let digest = token.digest();
    let mut tx = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL READ COMMITTED")
        .execute(&mut *tx)
        .await?;
    sqlx::query("SET LOCAL lock_timeout='1s'")
        .execute(&mut *tx)
        .await?;
    sqlx::query("SET LOCAL statement_timeout='2s'")
        .execute(&mut *tx)
        .await?;
    let row: Option<(i64, Option<i64>)> = sqlx::query_as(
        "SELECT created_at,revoked_at FROM staff_sessions WHERE token_hash=$1 FOR UPDATE",
    )
    .bind(digest.as_slice())
    .fetch_optional(&mut *tx)
    .await?;
    let at = now(&observed)?;
    if let Some((created, revoked)) = row {
        if created > at || revoked.is_some_and(|previous| previous > at) {
            return Err(StaffError::ClockUnavailable);
        }
        if revoked.is_none() {
            let updated = sqlx::query("UPDATE staff_sessions SET revoked_at=$2 WHERE token_hash=$1 AND revoked_at IS NULL")
                .bind(digest.as_slice()).bind(at).execute(&mut *tx).await?;
            if updated.rows_affected() != 1 {
                return Err(StaffError::Denied);
            }
            let stored: Option<(i64, Option<i64>)> = sqlx::query_as(
                "SELECT created_at,revoked_at FROM staff_sessions WHERE token_hash=$1",
            )
            .bind(digest.as_slice())
            .fetch_optional(&mut *tx)
            .await?;
            if stored != Some((created, Some(at))) {
                return Err(StaffError::Denied);
            }
        }
    }
    now(&observed)?;
    tx.commit().await?;
    Ok(())
}
