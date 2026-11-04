use commerce_application::request_context::{Clock, ObservedClock};
use sqlx::{PgPool, Postgres, Transaction};

use super::{
    CustomerIdentity, CustomerSessionToken, IssuedSession, SessionError, SessionRow, begin, now,
};

// Account-first locking serializes password changes and session writes consistently.
async fn claim(
    tx: &mut Transaction<'_, Postgres>,
    verifier: &[u8; 32],
    exclusive: bool,
    clock: &dyn Clock,
) -> Result<SessionRow, SessionError> {
    let account: Option<i64> =
        sqlx::query_scalar("SELECT account_id FROM customer_sessions WHERE verifier=$1")
            .bind(verifier.as_slice())
            .fetch_optional(&mut **tx)
            .await
            .map_err(|_| SessionError::Unavailable)?;
    let account = account.ok_or(SessionError::Denied)?;
    let account_sql = if exclusive {
        "SELECT verified_at FROM verified_customer_accounts WHERE id=$1 FOR UPDATE"
    } else {
        "SELECT verified_at FROM verified_customer_accounts WHERE id=$1 FOR SHARE"
    };
    let locked: Option<i64> = sqlx::query_scalar(account_sql)
        .bind(account)
        .fetch_optional(&mut **tx)
        .await
        .map_err(|_| SessionError::Unavailable)?;
    let verified_at = locked.ok_or(SessionError::Denied)?;
    let session_sql = if exclusive {
        "SELECT account_id,credential_revision,created_at,expires_at,rotated_from,revoked_at,revocation_kind,created_xid::text AS created_xid,revoked_xid IS NULL AS revoked_xid_empty FROM customer_sessions WHERE verifier=$1 AND account_id=$2 FOR UPDATE"
    } else {
        "SELECT account_id,credential_revision,created_at,expires_at,rotated_from,revoked_at,revocation_kind,created_xid::text AS created_xid,revoked_xid IS NULL AS revoked_xid_empty FROM customer_sessions WHERE verifier=$1 AND account_id=$2 FOR SHARE"
    };
    let row: Option<SessionRow> = sqlx::query_as(session_sql)
        .bind(verifier.as_slice())
        .bind(account)
        .fetch_optional(&mut **tx)
        .await
        .map_err(|_| SessionError::Unavailable)?;
    let row = row.ok_or(SessionError::Denied)?;
    let credential: Option<(i32, i64)> = sqlx::query_as(
        "SELECT revision,changed_at FROM customer_password_credentials WHERE account_id=$1",
    )
    .bind(account)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| SessionError::Unavailable)?;
    let valid_credential = credential.is_some_and(|(revision, changed)| {
        revision == row.credential_revision && changed >= 0 && changed <= row.created_at
    });
    if verified_at < 0
        || verified_at > row.created_at
        || !valid_credential
        || !row.active(now(clock)?)
    {
        return Err(SessionError::Denied);
    }
    Ok(row)
}

/// Returns a detached snapshot, not authority held through a later operation.
pub async fn authenticate(
    pool: &PgPool,
    token: &CustomerSessionToken,
    clock: &dyn Clock,
) -> Result<CustomerIdentity, SessionError> {
    let observed = ObservedClock::new(clock, 0);
    now(&observed)?;
    let mut tx = begin(pool).await?;
    let row = claim(&mut tx, &token.verifier(), false, &observed).await?;
    if !row.active(now(&observed)?) {
        return Err(SessionError::Denied);
    }
    tx.commit().await.map_err(|_| SessionError::Unavailable)?;
    Ok(row.identity())
}

/// Atomically revokes the old token and issues one successor without extending expiry.
pub async fn rotate(
    pool: &PgPool,
    token: &CustomerSessionToken,
    clock: &dyn Clock,
) -> Result<IssuedSession, SessionError> {
    let observed = ObservedClock::new(clock, 0);
    now(&observed)?;
    let old = token.verifier();
    let mut tx = begin(pool).await?;
    let row = claim(&mut tx, &old, true, &observed).await?;
    let captured = now(&observed)?;
    if !row.active(captured) {
        return Err(SessionError::Denied);
    }
    let successor = CustomerSessionToken::random()?;
    let new = successor.verifier();
    let changed = sqlx::query("UPDATE customer_sessions SET revoked_at=$2,revocation_kind='rotation',revoked_xid=NULL WHERE verifier=$1 AND revoked_at IS NULL")
        .bind(old.as_slice()).bind(captured).execute(&mut *tx).await.map_err(|_| SessionError::Unavailable)?;
    if changed.rows_affected() != 1 {
        return Err(SessionError::Unavailable);
    }
    let inserted = sqlx::query("INSERT INTO customer_sessions(verifier,account_id,credential_revision,created_at,expires_at,rotated_from,revoked_at,revocation_kind,revoked_xid,created_xid) VALUES ($1,$2,$3,$4,$5,$6,NULL,NULL,NULL,NULL)")
        .bind(new.as_slice()).bind(row.account_id).bind(row.credential_revision).bind(captured).bind(row.expires_at).bind(old.as_slice())
        .execute(&mut *tx).await.map_err(|_| SessionError::Unavailable)?;
    if inserted.rows_affected() != 1 {
        return Err(SessionError::Unavailable);
    }
    sqlx::query("SET CONSTRAINTS require_customer_session_rotation IMMEDIATE")
        .execute(&mut *tx)
        .await
        .map_err(|_| SessionError::Unavailable)?;
    let stored: Option<(bool,)> = sqlx::query_as("SELECT o.account_id=$3 AND o.credential_revision=$4 AND o.created_at=$5 AND o.expires_at=$6 AND o.rotated_from IS NOT DISTINCT FROM $7 AND o.revoked_at=$8 AND o.revocation_kind='rotation' AND o.revoked_xid=pg_current_xact_id() AND o.created_xid::text=$9 AND n.account_id=$3 AND n.credential_revision=$4 AND n.created_at=$8 AND n.expires_at=$6 AND n.rotated_from=$1 AND n.revoked_at IS NULL AND n.revocation_kind IS NULL AND n.revoked_xid IS NULL AND n.created_xid=pg_current_xact_id() FROM customer_sessions o JOIN customer_sessions n ON n.verifier=$2 WHERE o.verifier=$1")
        .bind(old.as_slice()).bind(new.as_slice()).bind(row.account_id).bind(row.credential_revision).bind(row.created_at).bind(row.expires_at).bind(row.rotated_from.as_deref()).bind(captured).bind(&row.created_xid)
        .fetch_optional(&mut *tx).await.map_err(|_| SessionError::Unavailable)?;
    if stored != Some((true,)) {
        return Err(SessionError::Unavailable);
    }
    if !row.active(now(&observed)?) {
        return Err(SessionError::Denied);
    }
    tx.commit().await.map_err(|_| SessionError::Unavailable)?;
    Ok(IssuedSession {
        token: successor,
        identity: row.identity(),
    })
}

/// Revokes an active session, with the account lock held through commit.
pub async fn logout(
    pool: &PgPool,
    token: &CustomerSessionToken,
    clock: &dyn Clock,
) -> Result<(), SessionError> {
    let observed = ObservedClock::new(clock, 0);
    now(&observed)?;
    let verifier = token.verifier();
    let mut tx = begin(pool).await?;
    let row = claim(&mut tx, &verifier, true, &observed).await?;
    let captured = now(&observed)?;
    if !row.active(captured) {
        return Err(SessionError::Denied);
    }
    let changed = sqlx::query("UPDATE customer_sessions SET revoked_at=$2,revocation_kind='logout',revoked_xid=NULL WHERE verifier=$1 AND revoked_at IS NULL")
        .bind(verifier.as_slice()).bind(captured).execute(&mut *tx).await.map_err(|_| SessionError::Unavailable)?;
    if changed.rows_affected() != 1 {
        return Err(SessionError::Unavailable);
    }
    let stored: Option<(bool,)> = sqlx::query_as("SELECT account_id=$2 AND credential_revision=$3 AND created_at=$4 AND expires_at=$5 AND rotated_from IS NOT DISTINCT FROM $6 AND revoked_at=$7 AND revocation_kind='logout' AND revoked_xid=pg_current_xact_id() AND created_xid::text=$8 FROM customer_sessions WHERE verifier=$1")
        .bind(verifier.as_slice()).bind(row.account_id).bind(row.credential_revision).bind(row.created_at).bind(row.expires_at).bind(row.rotated_from.as_deref()).bind(captured).bind(&row.created_xid)
        .fetch_optional(&mut *tx).await.map_err(|_| SessionError::Unavailable)?;
    if stored != Some((true,)) {
        return Err(SessionError::Unavailable);
    }
    if !row.active(now(&observed)?) {
        return Err(SessionError::Denied);
    }
    tx.commit().await.map_err(|_| SessionError::Unavailable)?;
    Ok(())
}
