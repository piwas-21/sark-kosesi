//! Private customer session custody; no transport or public endpoint is installed.
mod lifecycle;
mod login;

use commerce_application::request_context::Clock;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Transaction};
use std::time::Duration;
use zeroize::Zeroizing;

pub use lifecycle::{authenticate, logout, rotate};
pub use login::login;

const DOMAIN: &[u8] = b"piwas:customer-session:verifier:v1\0";
const LIFETIME: i64 = 43_200;

/// Opaque bearer. Only its domain-separated verifier is persisted.
pub struct CustomerSessionToken(Zeroizing<[u8; 32]>);
impl CustomerSessionToken {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(Zeroizing::new(bytes))
    }
    pub fn expose_bytes(&self) -> &[u8; 32] {
        &self.0
    }
    fn verifier(&self) -> [u8; 32] {
        let mut hash = Sha256::new();
        hash.update(DOMAIN);
        hash.update(self.expose_bytes());
        hash.finalize().into()
    }
    fn random() -> Result<Self, SessionError> {
        let mut token = Self::from_bytes([0; 32]);
        getrandom::fill(token.0.as_mut()).map_err(|_| SessionError::Unavailable)?;
        Ok(token)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum SessionError {
    Denied,
    Unavailable,
}

pub struct CustomerIdentity {
    pub account_id: i64,
    pub credential_revision: i32,
    pub expires_at: i64,
}

pub struct IssuedSession {
    pub token: CustomerSessionToken,
    pub identity: CustomerIdentity,
}

fn now(clock: &dyn Clock) -> Result<i64, SessionError> {
    clock
        .unix_seconds()
        .ok()
        .and_then(|v| i64::try_from(v).ok())
        .ok_or(SessionError::Unavailable)
}

async fn begin(pool: &PgPool) -> Result<Transaction<'_, Postgres>, SessionError> {
    let mut tx = tokio::time::timeout(Duration::from_secs(2), pool.begin())
        .await
        .map_err(|_| SessionError::Unavailable)?
        .map_err(|_| SessionError::Unavailable)?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL READ COMMITTED")
        .execute(&mut *tx)
        .await
        .map_err(|_| SessionError::Unavailable)?;
    sqlx::query(
        "SELECT set_config('lock_timeout','1s',true), set_config('statement_timeout','2s',true)",
    )
    .execute(&mut *tx)
    .await
    .map_err(|_| SessionError::Unavailable)?;
    Ok(tx)
}

#[derive(sqlx::FromRow)]
struct SessionRow {
    account_id: i64,
    credential_revision: i32,
    created_at: i64,
    expires_at: i64,
    rotated_from: Option<Vec<u8>>,
    revoked_at: Option<i64>,
    revocation_kind: Option<String>,
    created_xid: String,
    revoked_xid_empty: bool,
}
impl SessionRow {
    fn active(&self, at: i64) -> bool {
        self.account_id > 0
            && self.credential_revision > 0
            && self.created_at >= 0
            && self.expires_at > self.created_at
            && self
                .expires_at
                .checked_sub(self.created_at)
                .is_some_and(|v| v <= LIFETIME)
            && self.rotated_from.as_ref().is_none_or(|v| v.len() == 32)
            && !self.created_xid.is_empty()
            && self.revoked_xid_empty
            && self.revoked_at.is_none()
            && self.revocation_kind.is_none()
            && at >= self.created_at
            && at < self.expires_at
    }
    fn identity(&self) -> CustomerIdentity {
        CustomerIdentity {
            account_id: self.account_id,
            credential_revision: self.credential_revision,
            expires_at: self.expires_at,
        }
    }
}
