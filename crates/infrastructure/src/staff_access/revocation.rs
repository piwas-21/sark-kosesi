//! Revocation removes authority; it never issues or refreshes a staff session.
use super::SessionToken;
use commerce_application::request_context::{Clock, ObservedClock};
use sqlx::PgPool;

/// Closed failure categories; database diagnostics never cross this boundary.
#[derive(Debug, PartialEq, Eq)]
pub enum RevocationError {
    ClockUnavailable,
    Unavailable,
}

fn now(clock: &dyn Clock) -> Result<i64, RevocationError> {
    clock
        .unix_seconds()
        .ok()
        .and_then(|at| i64::try_from(at).ok())
        .ok_or(RevocationError::ClockUnavailable)
}

/// Revoke only the exact supplied bearer. Missing and already-revoked tokens are
/// successful no-ops when the clock is valid. MFA freshness and capability are not
/// required to relinquish authority; this does not authenticate a staff actor.
pub async fn revoke_self(
    pool: &PgPool,
    token: &SessionToken,
    clock: &dyn Clock,
) -> Result<(), RevocationError> {
    let observed = ObservedClock::new(clock, 0);
    now(&observed)?;
    let digest = token.digest();
    let mut tx = pool
        .begin()
        .await
        .map_err(|_| RevocationError::Unavailable)?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL READ COMMITTED")
        .execute(&mut *tx)
        .await
        .map_err(|_| RevocationError::Unavailable)?;
    sqlx::query("SET LOCAL lock_timeout='1s'")
        .execute(&mut *tx)
        .await
        .map_err(|_| RevocationError::Unavailable)?;
    sqlx::query("SET LOCAL statement_timeout='2s'")
        .execute(&mut *tx)
        .await
        .map_err(|_| RevocationError::Unavailable)?;
    let row: Option<(i64, Option<i64>)> = sqlx::query_as(
        "SELECT created_at,revoked_at FROM staff_sessions WHERE token_hash=$1 FOR UPDATE",
    )
    .bind(digest.as_slice())
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| RevocationError::Unavailable)?;
    let at = now(&observed)?;
    if let Some((created, revoked)) = row {
        if created > at || revoked.is_some_and(|previous| previous > at) {
            return Err(RevocationError::ClockUnavailable);
        }
        if revoked.is_none() {
            let updated = sqlx::query("UPDATE staff_sessions SET revoked_at=$2 WHERE token_hash=$1 AND revoked_at IS NULL")
                .bind(digest.as_slice()).bind(at).execute(&mut *tx).await.map_err(|_| RevocationError::Unavailable)?;
            if updated.rows_affected() != 1 {
                return Err(RevocationError::Unavailable);
            }
            let stored: Option<(i64, Option<i64>)> = sqlx::query_as(
                "SELECT created_at,revoked_at FROM staff_sessions WHERE token_hash=$1",
            )
            .bind(digest.as_slice())
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| RevocationError::Unavailable)?;
            if stored != Some((created, Some(at))) {
                return Err(RevocationError::Unavailable);
            }
        }
    }
    now(&observed)?;
    tx.commit()
        .await
        .map_err(|_| RevocationError::Unavailable)?;
    Ok(())
}
