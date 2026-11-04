//! Private challenge custody. Possession alone grants no account or order authority.
#[path = "verified_customer_accounts.rs"]
pub mod verified_customer_accounts;

use commerce_application::request_context::Clock;
use commerce_domain::email_identity::normalize_email;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Transaction};

const PURPOSE: &str = "email-verification-v1";
const DOMAIN: &[u8] = b"piwas:email-verification:verifier:v1\0";
const LIFETIME: i64 = 900;

/// Secret bytes are exposed only to an explicitly trusted delivery integration.
/// The caller must protect the value; no Debug, Clone or serialization is provided.
pub struct VerificationToken([u8; 32]);
impl VerificationToken {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
    pub fn expose_bytes(&self) -> &[u8; 32] {
        &self.0
    }
    fn verifier(&self) -> [u8; 32] {
        let mut hash = Sha256::new();
        hash.update(DOMAIN);
        hash.update(self.0);
        hash.finalize().into()
    }
}

/// Denial never distinguishes absent, incorrect, expired, consumed or mismatched identity.
/// Operational errors retain no SQL details or submitted identity.
#[derive(Debug, PartialEq, Eq)]
pub enum VerificationError {
    Denied,
    Unavailable,
}
fn now(clock: &dyn Clock) -> Result<i64, VerificationError> {
    clock
        .unix_seconds()
        .ok()
        .and_then(|n| i64::try_from(n).ok())
        .ok_or(VerificationError::Unavailable)
}

/// Issues an OS-random bearer exactly once to the caller after durable insertion.
pub async fn issue(
    pool: &PgPool,
    email: &str,
    clock: &dyn Clock,
) -> Result<VerificationToken, VerificationError> {
    let identity = normalize_email(email).ok_or(VerificationError::Denied)?;
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| VerificationError::Unavailable)?;
    let token = VerificationToken(bytes);
    let mut tx = pool
        .begin()
        .await
        .map_err(|_| VerificationError::Unavailable)?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL READ COMMITTED")
        .execute(&mut *tx)
        .await
        .map_err(|_| VerificationError::Unavailable)?;
    sqlx::query(
        "SELECT set_config('lock_timeout','1s',true), set_config('statement_timeout','2s',true)",
    )
    .execute(&mut *tx)
    .await
    .map_err(|_| VerificationError::Unavailable)?;
    let created = now(clock)?;
    let expires = created
        .checked_add(LIFETIME)
        .ok_or(VerificationError::Unavailable)?;
    let inserted = sqlx::query("INSERT INTO email_verification_challenges(verifier,purpose,email_identity,created_at,expires_at) VALUES ($1,$2,$3,$4,$5)")
        .bind(token.verifier().as_slice()).bind(PURPOSE).bind(identity).bind(created).bind(expires)
        .execute(&mut *tx).await.map_err(|_| VerificationError::Unavailable)?;
    if inserted.rows_affected() != 1 {
        return Err(VerificationError::Unavailable);
    }
    let final_now = now(clock)?;
    if final_now < created || final_now >= expires {
        return Err(VerificationError::Unavailable);
    }
    tx.commit()
        .await
        .map_err(|_| VerificationError::Unavailable)?;
    Ok(token)
}

/// Atomically consumes a matching challenge. Failed attempts and replay return the same denial.
pub async fn consume(
    pool: &PgPool,
    email: &str,
    token: &VerificationToken,
    clock: &dyn Clock,
) -> Result<(), VerificationError> {
    let mut tx = begin(pool).await?;
    let guard = consume_in_transaction(&mut tx, email, token, clock).await?;
    guard.check(clock)?;
    tx.commit()
        .await
        .map_err(|_| VerificationError::Unavailable)?;
    Ok(())
}

async fn begin(pool: &PgPool) -> Result<Transaction<'_, Postgres>, VerificationError> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|_| VerificationError::Unavailable)?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL READ COMMITTED")
        .execute(&mut *tx)
        .await
        .map_err(|_| VerificationError::Unavailable)?;
    sqlx::query(
        "SELECT set_config('lock_timeout','1s',true), set_config('statement_timeout','2s',true)",
    )
    .execute(&mut *tx)
    .await
    .map_err(|_| VerificationError::Unavailable)?;
    Ok(tx)
}

// The guard is private to this module and its account child. The caller must check it
// after all dependent writes, immediately before committing the transaction.
struct Consumption {
    identity: String,
    verifier: [u8; 32],
    captured: i64,
    expires: i64,
}
impl Consumption {
    fn check(&self, clock: &dyn Clock) -> Result<(), VerificationError> {
        let final_now = now(clock)?;
        if final_now < self.captured {
            return Err(VerificationError::Unavailable);
        }
        if final_now >= self.expires {
            return Err(VerificationError::Denied);
        }
        Ok(())
    }
}

async fn consume_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    email: &str,
    token: &VerificationToken,
    clock: &dyn Clock,
) -> Result<Consumption, VerificationError> {
    let identity = normalize_email(email).ok_or(VerificationError::Denied)?;
    let started = now(clock)?;
    let verifier = token.verifier();
    let row: Option<(i64, i64, Option<i64>)> = sqlx::query_as("SELECT created_at,expires_at,consumed_at FROM email_verification_challenges WHERE verifier=$1 AND purpose=$2 AND email_identity=$3 FOR UPDATE")
        .bind(verifier.as_slice()).bind(PURPOSE).bind(&identity).fetch_optional(&mut **tx).await.map_err(|_| VerificationError::Unavailable)?;
    let Some((created, expires, consumed)) = row else {
        return Err(VerificationError::Denied);
    };
    let captured = now(clock)?;
    if captured < started {
        return Err(VerificationError::Unavailable);
    }
    if captured < created || captured >= expires || consumed.is_some() {
        return Err(VerificationError::Denied);
    }
    let updated =
        sqlx::query("UPDATE email_verification_challenges SET consumed_at=$2 WHERE verifier=$1")
            .bind(verifier.as_slice())
            .bind(captured)
            .execute(&mut **tx)
            .await
            .map_err(|_| VerificationError::Unavailable)?;
    if updated.rows_affected() != 1 {
        return Err(VerificationError::Unavailable);
    }
    Ok(Consumption {
        identity,
        verifier,
        captured,
        expires,
    })
}
