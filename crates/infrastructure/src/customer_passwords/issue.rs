use commerce_application::request_context::{Clock, ObservedClock};
use commerce_domain::email_identity::normalize_email;
use sqlx::PgPool;

use super::{LIFETIME, PasswordChangeToken, PasswordError, begin, current_revision, now};

#[derive(sqlx::FromRow)]
struct Issued {
    verifier: Vec<u8>,
    account_id: i64,
    expected_revision: i32,
    created_at: i64,
    expires_at: i64,
    consumed_at: Option<i64>,
    xid_empty: bool,
}

/// Issues a bearer to a trusted service only, after finding an exact verified identity.
pub async fn issue(
    pool: &PgPool,
    email: &str,
    clock: &dyn Clock,
) -> Result<PasswordChangeToken, PasswordError> {
    let identity = normalize_email(email).ok_or(PasswordError::Denied)?;
    let observed = ObservedClock::new(clock, 0);
    now(&observed)?;
    let mut tx = begin(pool).await?;
    let account: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM verified_customer_accounts WHERE email_identity=$1 FOR UPDATE",
    )
    .bind(&identity)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| PasswordError::Unavailable)?;
    let account = account.ok_or(PasswordError::Denied)?;
    let revision = current_revision(&mut tx, account).await?;
    let created = now(&observed)?;
    let expires = created
        .checked_add(LIFETIME)
        .ok_or(PasswordError::Unavailable)?;
    let mut token = PasswordChangeToken::from_bytes([0; 32]);
    getrandom::fill(token.0.as_mut()).map_err(|_| PasswordError::Unavailable)?;
    let verifier = token.verifier();
    let stored: Issued = sqlx::query_as(
        "INSERT INTO customer_password_challenges(verifier,account_id,expected_revision,created_at,expires_at) VALUES ($1,$2,$3,$4,$5) RETURNING verifier,account_id,expected_revision,created_at,expires_at,consumed_at,consumed_xid IS NULL AS xid_empty"
    ).bind(verifier.as_slice()).bind(account).bind(revision).bind(created).bind(expires)
        .fetch_one(&mut *tx).await.map_err(|_| PasswordError::Unavailable)?;
    if stored.verifier != verifier
        || stored.account_id != account
        || stored.expected_revision != revision
        || stored.created_at != created
        || stored.expires_at != expires
        || stored.consumed_at.is_some()
        || !stored.xid_empty
    {
        return Err(PasswordError::Unavailable);
    }
    if now(&observed)? >= expires {
        return Err(PasswordError::Unavailable);
    }
    tx.commit().await.map_err(|_| PasswordError::Unavailable)?;
    Ok(token)
}
