use commerce_application::request_context::{Clock, ObservedClock};
use commerce_domain::email_identity::normalize_email;
use sqlx::PgPool;

use super::{
    CustomerCsrfToken, CustomerIdentity, CustomerSessionToken, IssuedCsrfSession, IssuedSession,
    LIFETIME, SessionError, begin, now,
};
use crate::{
    customer_credential_admission::{
        AdmissionError, AdmissionKey, AdmissionLimits, CredentialPurpose, admit,
    },
    customer_passwords::{PasswordError, hash::verify_password},
};

#[derive(sqlx::FromRow, PartialEq, Eq)]
struct Credential {
    account_id: i64,
    revision: i32,
    password_hash: String,
    changed_at: i64,
}

impl Credential {
    fn can_issue(&self, initial: &Self, verified_at: i64, issued_at: i64) -> bool {
        verified_at.min(self.changed_at) >= 0
            && verified_at.max(self.changed_at) <= issued_at
            && self.revision > 0
            && self == initial
    }
}

/// Verify outside a transaction, then issue only against the same locked credential.
pub async fn login(
    pool: &PgPool,
    email: &str,
    password: &str,
    clock: &dyn Clock,
    key: &AdmissionKey,
    limits: AdmissionLimits,
) -> Result<IssuedSession, SessionError> {
    let (session, _) = login_inner(pool, email, password, clock, key, limits, false).await?;
    Ok(session)
}

/// Trusted private issuance returning a fresh session-bound synchronizer once.
pub async fn login_with_csrf(
    pool: &PgPool,
    email: &str,
    password: &str,
    clock: &dyn Clock,
    key: &AdmissionKey,
    limits: AdmissionLimits,
) -> Result<IssuedCsrfSession, SessionError> {
    let (session, csrf) = login_inner(pool, email, password, clock, key, limits, true).await?;
    Ok(IssuedCsrfSession {
        session,
        csrf: csrf.ok_or(SessionError::Unavailable)?,
    })
}

async fn login_inner(
    pool: &PgPool,
    email: &str,
    password: &str,
    clock: &dyn Clock,
    key: &AdmissionKey,
    limits: AdmissionLimits,
    bind_csrf: bool,
) -> Result<(IssuedSession, Option<CustomerCsrfToken>), SessionError> {
    let observed = ObservedClock::new(clock, 0);
    admit(
        pool,
        CredentialPurpose::Login,
        email,
        key,
        limits,
        &observed,
    )
    .await
    .map_err(|error| match error {
        AdmissionError::Limited => SessionError::Denied,
        AdmissionError::Unavailable => SessionError::Unavailable,
    })?;
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
    if !current.can_issue(&initial, verified_at, created) {
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
    let csrf = if bind_csrf {
        let csrf = CustomerCsrfToken::random()?;
        let digest = csrf.digest();
        let inserted = sqlx::query("INSERT INTO customer_session_csrf(session_verifier,csrf_digest,created_xid) VALUES ($1,$2,NULL)")
            .bind(verifier.as_slice()).bind(digest.as_slice()).execute(&mut *tx)
            .await.map_err(|_| SessionError::Unavailable)?;
        let exact: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM customer_session_csrf WHERE session_verifier=$1 AND csrf_digest=$2 AND created_xid=pg_current_xact_id())")
            .bind(verifier.as_slice()).bind(digest.as_slice()).fetch_one(&mut *tx)
            .await.map_err(|_| SessionError::Unavailable)?;
        if inserted.rows_affected() != 1 || !exact {
            return Err(SessionError::Unavailable);
        }
        Some(csrf)
    } else {
        None
    };
    if now(&observed)? >= expires {
        return Err(SessionError::Denied);
    }
    tx.commit().await.map_err(|_| SessionError::Unavailable)?;
    Ok((
        IssuedSession {
            token,
            identity: CustomerIdentity {
                account_id: initial.account_id,
                credential_revision: current.revision,
                expires_at: expires,
            },
        },
        csrf,
    ))
}

#[cfg(test)]
mod tests {
    use super::Credential;

    #[test]
    fn issuance_requires_current_exact_credential_and_valid_chronology() {
        let verified = Credential {
            account_id: 7,
            revision: 2,
            password_hash: "verified fixture hash".into(),
            changed_at: 100,
        };
        assert!(verified.can_issue(&verified, 90, 100));
        assert!(!verified.can_issue(&verified, -1, 100));
        assert!(!verified.can_issue(&verified, 90, 99));
        let invalid_revision = Credential {
            account_id: 7,
            revision: 0,
            password_hash: verified.password_hash.clone(),
            changed_at: 100,
        };
        assert!(!invalid_revision.can_issue(&invalid_revision, 90, 100));
        let replaced = Credential {
            password_hash: "replaced fixture hash".into(),
            ..verified
        };
        assert!(!replaced.can_issue(&verified, 90, 100));
    }
}
