//! Private password change custody. This module creates no session or login authority.
pub(crate) mod hash;
mod issue;
mod replace;

use commerce_application::request_context::Clock;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Transaction};
use zeroize::Zeroizing;

pub use issue::issue;
pub use replace::replace;

const DOMAIN: &[u8] = b"piwas:customer-password-change:verifier:v1\0";
const LIFETIME: i64 = 900;

// Keep independent unit fixtures from exhausting the production hash admission limit.
#[cfg(test)]
static TEST_HASH_FIXTURE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Secret bearer for trusted delivery only. It is never stored or logged.
pub struct PasswordChangeToken(Zeroizing<[u8; 32]>);
impl PasswordChangeToken {
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
}

/// Denial does not reveal whether an account or proof exists.
#[derive(Debug, PartialEq, Eq)]
pub enum PasswordError {
    Denied,
    Unavailable,
}

fn now(clock: &dyn Clock) -> Result<i64, PasswordError> {
    clock
        .unix_seconds()
        .ok()
        .and_then(|value| i64::try_from(value).ok())
        .ok_or(PasswordError::Unavailable)
}

async fn begin(pool: &PgPool) -> Result<Transaction<'_, Postgres>, PasswordError> {
    let mut tx = pool.begin().await.map_err(|_| PasswordError::Unavailable)?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL READ COMMITTED")
        .execute(&mut *tx)
        .await
        .map_err(|_| PasswordError::Unavailable)?;
    sqlx::query(
        "SELECT set_config('lock_timeout','1s',true), set_config('statement_timeout','2s',true)",
    )
    .execute(&mut *tx)
    .await
    .map_err(|_| PasswordError::Unavailable)?;
    Ok(tx)
}

async fn current_revision(
    tx: &mut Transaction<'_, Postgres>,
    account: i64,
) -> Result<i32, PasswordError> {
    let revision: Option<i32> = sqlx::query_scalar(
        "SELECT revision FROM customer_password_credentials WHERE account_id=$1",
    )
    .bind(account)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| PasswordError::Unavailable)?;
    match revision {
        None => Ok(0),
        Some(value) if value > 0 => Ok(value),
        Some(_) => Err(PasswordError::Unavailable),
    }
}

#[cfg(test)]
#[path = "../../tests/support/customer_password_boundary_unit.rs"]
mod boundary_tests;
