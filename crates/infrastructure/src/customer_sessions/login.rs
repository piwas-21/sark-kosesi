use commerce_application::request_context::{Clock, ObservedClock};
use commerce_domain::email_identity::normalize_email;
use sqlx::PgPool;

use super::{
    CustomerIdentity, CustomerSessionToken, IssuedSession, LIFETIME, SessionError, begin, now,
};
use crate::customer_passwords::{PasswordError, hash::verify_password};

#[derive(sqlx::FromRow)]
struct Credential {
    account_id: i64,
    revision: i32,
    password_hash: String,
    changed_at: i64,
}

/// Verify outside a transaction, then issue only against the same locked credential.
pub async fn login(
    pool: &PgPool,
    email: &str,
    password: &str,
    clock: &dyn Clock,
) -> Result<IssuedSession, SessionError> {
    let observed = ObservedClock::new(clock, 0);
    now(&observed)?;
    let identity = normalize_email(email);
    let mut lookup = begin(pool).await?;
    let initial: Option<Credential> = sqlx::query_as("SELECT a.id AS account_id,c.revision,left(c.password_hash,98) AS password_hash,c.changed_at FROM verified_customer_accounts a JOIN customer_password_credentials c ON c.account_id=a.id WHERE a.email_identity=$1")
        .bind(identity.as_deref()).fetch_optional(&mut *lookup).await.map_err(|_| SessionError::Unavailable)?;
    lookup
        .rollback()
        .await
        .map_err(|_| SessionError::Unavailable)?;
    let verified = verify_password(password, initial.as_ref().map(|c| c.password_hash.as_str()))
        .await
        .map_err(|e| match e {
            PasswordError::Denied => SessionError::Denied,
            PasswordError::Unavailable => SessionError::Unavailable,
        })?;
    let initial = initial.ok_or(SessionError::Denied)?;
    if !verified {
        return Err(SessionError::Denied);
    }
    let mut tx = begin(pool).await?;
    let account: Option<i64> = sqlx::query_scalar(
        "SELECT verified_at FROM verified_customer_accounts WHERE id=$1 AND email_identity=$2 FOR UPDATE",
    )
    .bind(initial.account_id)
    .bind(identity.as_deref())
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| SessionError::Unavailable)?;
    let verified_at = account.ok_or(SessionError::Denied)?;
    let current: Option<Credential> = sqlx::query_as("SELECT account_id,revision,left(password_hash,98) AS password_hash,changed_at FROM customer_password_credentials WHERE account_id=$1")
        .bind(initial.account_id).fetch_optional(&mut *tx).await.map_err(|_| SessionError::Unavailable)?;
    let current = current.ok_or(SessionError::Denied)?;
    let created = now(&observed)?;
    if verified_at < 0
        || verified_at > created
        || current.revision <= 0
        || current.changed_at < 0
        || current.changed_at > created
        || current.revision != initial.revision
        || current.password_hash != initial.password_hash
        || current.changed_at != initial.changed_at
    {
        return Err(SessionError::Denied);
    }
    let expires = created
        .checked_add(LIFETIME)
        .ok_or(SessionError::Unavailable)?;
    let token = CustomerSessionToken::random()?;
    let verifier = token.verifier();
    let count = sqlx::query("INSERT INTO customer_sessions(verifier,account_id,credential_revision,created_at,expires_at,rotated_from,revoked_at,revocation_kind,revoked_xid,created_xid) VALUES ($1,$2,$3,$4,$5,NULL,NULL,NULL,NULL,NULL)")
        .bind(verifier.as_slice()).bind(initial.account_id).bind(current.revision).bind(created).bind(expires)
        .execute(&mut *tx).await.map_err(|_| SessionError::Unavailable)?;
    if count.rows_affected() != 1 {
        return Err(SessionError::Unavailable);
    }
    let stored: Option<(bool,)> = sqlx::query_as("SELECT account_id=$2 AND credential_revision=$3 AND created_at=$4 AND expires_at=$5 AND rotated_from IS NULL AND revoked_at IS NULL AND revocation_kind IS NULL AND revoked_xid IS NULL AND created_xid=pg_current_xact_id() FROM customer_sessions WHERE verifier=$1")
        .bind(verifier.as_slice()).bind(initial.account_id).bind(current.revision).bind(created).bind(expires)
        .fetch_optional(&mut *tx).await.map_err(|_| SessionError::Unavailable)?;
    if stored != Some((true,)) {
        return Err(SessionError::Unavailable);
    }
    if now(&observed)? >= expires {
        return Err(SessionError::Denied);
    }
    tx.commit().await.map_err(|_| SessionError::Unavailable)?;
    Ok(IssuedSession {
        token,
        identity: CustomerIdentity {
            account_id: initial.account_id,
            credential_revision: current.revision,
            expires_at: expires,
        },
    })
}
