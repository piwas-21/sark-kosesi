use commerce_application::request_context::{Clock, ObservedClock};
use commerce_domain::email_identity::normalize_email;
use sqlx::PgPool;

use super::{
    AdmissionKey, AdmissionLimits, CredentialPurpose, LIFETIME, PasswordChangeToken, PasswordError,
    admit_password, begin, current_revision, hash, now,
};

#[derive(sqlx::FromRow)]
struct Proof {
    account_id: i64,
    expected_revision: i32,
    created_at: i64,
    expires_at: i64,
    consumed_at: Option<i64>,
    xid_empty: bool,
}
impl Proof {
    fn valid(&self, at: i64) -> bool {
        self.expected_revision >= 0
            && self.created_at >= 0
            && self.created_at.checked_add(LIFETIME) == Some(self.expires_at)
            && at >= self.created_at
            && at < self.expires_at
            && self.consumed_at.is_none()
            && self.xid_empty
    }
    fn same_source(&self, other: &Self) -> bool {
        self.account_id == other.account_id
            && self.expected_revision == other.expected_revision
            && self.created_at == other.created_at
            && self.expires_at == other.expires_at
    }
}

#[derive(sqlx::FromRow)]
struct Stored {
    revision: i32,
    password_hash: String,
    changed_at: i64,
    challenge_verifier: Vec<u8>,
    credential_xid_matches: bool,
    account_id: i64,
    expected_revision: i32,
    created_at: i64,
    expires_at: i64,
    consumed_at: Option<i64>,
    proof_xid_matches: bool,
}

impl Stored {
    fn matches(
        &self,
        proof: &Proof,
        next: i32,
        password_hash: &str,
        verifier: &[u8],
        captured: i64,
    ) -> bool {
        self.revision == next
            && self.password_hash == password_hash
            && self.changed_at == captured
            && self.challenge_verifier.as_slice() == verifier
            && self.credential_xid_matches
            && self.account_id == proof.account_id
            && self.expected_revision == proof.expected_revision
            && self.created_at == proof.created_at
            && self.expires_at == proof.expires_at
            && self.consumed_at == Some(captured)
            && self.proof_xid_matches
    }
}

/// Consumes exact proof and changes the credential atomically; it grants no session.
pub async fn replace(
    pool: &PgPool,
    email: &str,
    token: &PasswordChangeToken,
    password: &str,
    clock: &dyn Clock,
    key: &AdmissionKey,
    limits: AdmissionLimits,
) -> Result<i32, PasswordError> {
    let observed = ObservedClock::new(clock, 0);
    admit_password(
        pool,
        CredentialPurpose::PasswordReplace,
        email,
        key,
        limits,
        &observed,
    )
    .await?;
    let identity = normalize_email(email).ok_or(PasswordError::Denied)?;
    let verifier = token.verifier();
    now(&observed)?;
    // The cheap precheck rejects absent, consumed, expired and stale proof before Argon2.
    let mut precheck = begin(pool).await?;
    let initial: Option<Proof> = sqlx::query_as(
        "SELECT p.account_id,p.expected_revision,p.created_at,p.expires_at,p.consumed_at,p.consumed_xid IS NULL AS xid_empty FROM customer_password_challenges p JOIN verified_customer_accounts a ON a.id=p.account_id LEFT JOIN customer_password_credentials c ON c.account_id=a.id WHERE p.verifier=$1 AND a.email_identity=$2 AND p.expected_revision=COALESCE(c.revision,0)"
    ).bind(verifier.as_slice()).bind(&identity).fetch_optional(&mut *precheck).await.map_err(|_| PasswordError::Unavailable)?;
    precheck
        .rollback()
        .await
        .map_err(|_| PasswordError::Unavailable)?;
    let initial = initial.ok_or(PasswordError::Denied)?;
    if !initial.valid(now(&observed)?) {
        return Err(PasswordError::Denied);
    }
    let password_hash = hash::hash_password(password).await?;
    let mut tx = begin(pool).await?;
    let account: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM verified_customer_accounts WHERE id=$1 AND email_identity=$2 FOR UPDATE",
    )
    .bind(initial.account_id)
    .bind(&identity)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| PasswordError::Unavailable)?;
    if account != Some(initial.account_id) {
        return Err(PasswordError::Denied);
    }
    let proof: Option<Proof> = sqlx::query_as(
        "SELECT account_id,expected_revision,created_at,expires_at,consumed_at,consumed_xid IS NULL AS xid_empty FROM customer_password_challenges WHERE verifier=$1 AND account_id=$2 FOR UPDATE"
    ).bind(verifier.as_slice()).bind(initial.account_id).fetch_optional(&mut *tx).await.map_err(|_| PasswordError::Unavailable)?;
    let proof = proof.ok_or(PasswordError::Denied)?;
    let current = current_revision(&mut tx, initial.account_id).await?;
    let captured = now(&observed)?;
    if !proof.same_source(&initial) || !proof.valid(captured) || current != proof.expected_revision
    {
        return Err(PasswordError::Denied);
    }
    let next = current.checked_add(1).ok_or(PasswordError::Unavailable)?;
    let consumed = sqlx::query("UPDATE customer_password_challenges SET consumed_at=$2 WHERE verifier=$1 AND account_id=$3 AND consumed_at IS NULL")
        .bind(verifier.as_slice()).bind(captured).bind(initial.account_id)
        .execute(&mut *tx).await.map_err(|_| PasswordError::Unavailable)?;
    if consumed.rows_affected() != 1 {
        return Err(PasswordError::Unavailable);
    }
    let changed = if current == 0 {
        sqlx::query("INSERT INTO customer_password_credentials(account_id,revision,password_hash,changed_at,challenge_verifier) VALUES ($1,$2,$3,$4,$5)")
            .bind(initial.account_id).bind(next).bind(&password_hash).bind(captured).bind(verifier.as_slice())
            .execute(&mut *tx).await
    } else {
        sqlx::query("UPDATE customer_password_credentials SET revision=$2,password_hash=$3,changed_at=$4,challenge_verifier=$5 WHERE account_id=$1 AND revision=$6")
            .bind(initial.account_id).bind(next).bind(&password_hash).bind(captured).bind(verifier.as_slice()).bind(current)
            .execute(&mut *tx).await
    }.map_err(|_| PasswordError::Unavailable)?;
    if changed.rows_affected() != 1 {
        return Err(PasswordError::Unavailable);
    }
    sqlx::query("SET CONSTRAINTS require_customer_password_change IMMEDIATE")
        .execute(&mut *tx)
        .await
        .map_err(|_| PasswordError::Unavailable)?;
    let stored: Option<Stored> = sqlx::query_as(
        "SELECT c.revision,c.password_hash,c.changed_at,c.challenge_verifier,c.changed_xid=pg_current_xact_id() AS credential_xid_matches,p.account_id,p.expected_revision,p.created_at,p.expires_at,p.consumed_at,p.consumed_xid=pg_current_xact_id() AS proof_xid_matches FROM customer_password_credentials c JOIN customer_password_challenges p ON p.verifier=c.challenge_verifier AND p.account_id=c.account_id WHERE c.account_id=$1"
    ).bind(initial.account_id).fetch_optional(&mut *tx).await.map_err(|_| PasswordError::Unavailable)?;
    let stored = stored.ok_or(PasswordError::Unavailable)?;
    if !stored.matches(&proof, next, &password_hash, &verifier, captured) {
        return Err(PasswordError::Unavailable);
    }
    if now(&observed)? >= proof.expires_at {
        return Err(PasswordError::Denied);
    }
    tx.commit().await.map_err(|_| PasswordError::Unavailable)?;
    Ok(next)
}

#[cfg(test)]
#[path = "../../tests/support/customer_password_replace_unit.rs"]
mod tests;
