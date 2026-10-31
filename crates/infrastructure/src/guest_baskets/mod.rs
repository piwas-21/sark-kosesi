//! Maintained session transport with fresh PostgreSQL guest-ownership guards.
use commerce_application::{
    guest_access::{GuestLifetime, GuestLimits},
    request_context::{Clock, ContextError, Identifiers},
};
use commerce_domain::{
    guest_baskets::{BasketUpdate, valid_lines},
    quotes::BasketLine,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Transaction};
use std::{fmt, sync::Arc};
use subtle::ConstantTimeEq;
use tower_sessions::{
    Session, SessionStore,
    cookie::time::OffsetDateTime,
    session::{Id, Record},
    session_store,
};
mod access;
mod basket;
mod identity;
mod issue;
mod scope;
mod session_create;
pub(crate) use scope::{BasketScope, mutation_scope, read_scope};
mod store;
pub use basket::{read_basket, write_basket};
use identity::GuestIdentity;
pub use issue::{ensure_session, revoke_session};

#[derive(Debug)]
pub enum GuestError {
    InvalidRequest,
    InvalidStoredData,
    Denied,
    Csrf,
    Conflict,
    RateLimited,
    ClockUnavailable,
    EntropyUnavailable,
    SessionUnavailable,
    Database(sqlx::Error),
}
impl From<sqlx::Error> for GuestError {
    fn from(e: sqlx::Error) -> Self {
        Self::Database(e)
    }
}
impl From<ContextError> for GuestError {
    fn from(e: ContextError) -> Self {
        match e {
            ContextError::ClockUnavailable => Self::ClockUnavailable,
            ContextError::EntropyUnavailable => Self::EntropyUnavailable,
        }
    }
}
#[derive(Clone)]
pub struct GuestSessionStore {
    pool: PgPool,
    clock: Arc<dyn Clock>,
    identifiers: Arc<dyn Identifiers>,
    limits: GuestLimits,
}
impl fmt::Debug for GuestSessionStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("GuestSessionStore { redacted }")
    }
}
impl GuestSessionStore {
    pub fn new(
        pool: PgPool,
        clock: Arc<dyn Clock>,
        identifiers: Arc<dyn Identifiers>,
        limits: GuestLimits,
    ) -> Result<Self, GuestError> {
        if !limits.valid() {
            return Err(GuestError::InvalidRequest);
        }
        Ok(Self {
            pool,
            clock,
            identifiers,
            limits,
        })
    }
}
// No Debug/Serialize/Display: transport IDs are bearer credentials.
pub struct GuestCredential(Id);
impl GuestCredential {
    pub fn from_session(session: &Session) -> Result<Self, GuestError> {
        session.id().map(Self).ok_or(GuestError::Denied)
    }
    fn digest(&self) -> [u8; 32] {
        digest(self.0)
    }
}
fn digest(id: Id) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"piwas:guest-session:v1:");
    h.update(id.0.to_le_bytes());
    h.finalize().into()
}
fn now(clock: &dyn Clock) -> Result<i64, GuestError> {
    i64::try_from(clock.unix_seconds()?).map_err(|_| GuestError::ClockUnavailable)
}
fn valid_hex(value: &str, len: usize) -> bool {
    value.len() == len
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
async fn begin(pool: &PgPool) -> Result<Transaction<'static, Postgres>, GuestError> {
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
    Ok(tx)
}
fn backend(error: GuestError) -> session_store::Error {
    session_store::Error::Backend(
        if matches!(error, GuestError::RateLimited) {
            "guest_issue_rate_limited"
        } else {
            "guest_session_unavailable"
        }
        .into(),
    )
}
// The CSRF value is session-scoped response data, never a cookie, URL or log field.
pub struct BasketView {
    pub revision: i32,
    pub lines: Vec<BasketLine>,
    csrf_token: String,
}
impl BasketView {
    pub fn csrf_token(&self) -> &str {
        &self.csrf_token
    }
}
#[derive(Debug, PartialEq, Eq)]
pub struct BasketReceipt {
    pub revision: i32,
    pub replayed: bool,
}
