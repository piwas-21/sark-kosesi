//! Transaction-scoped authority from persisted sessions/grants. No issuing endpoint.
use commerce_application::{
    request_context::Clock,
    staff_access::{Capability, SessionState, check_session},
};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Transaction};

// Intentionally no Debug/Serialize: a bearer credential is never response/log data.
pub struct SessionToken([u8; 32]);
impl SessionToken {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
    pub fn digest(&self) -> [u8; 32] {
        Sha256::digest(self.0).into()
    }
}
#[derive(Debug)]
pub enum StaffError {
    InvalidRequest,
    Denied,
    ClockUnavailable,
    Database(sqlx::Error),
    Stock(super::stock::StockWriteError),
}
impl From<sqlx::Error> for StaffError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}
impl From<super::stock::StockWriteError> for StaffError {
    fn from(error: super::stock::StockWriteError) -> Self {
        Self::Stock(error)
    }
}
pub(super) struct StaffTransaction {
    pub tx: Transaction<'static, Postgres>,
    pub actor: String,
    state: SessionState,
}
impl StaffTransaction {
    pub fn check_time(&self, clock: &dyn Clock) -> Result<(), StaffError> {
        self.authorized_at(clock).map(|_| ())
    }
    pub fn authorized_at(&self, clock: &dyn Clock) -> Result<i64, StaffError> {
        let seconds = clock
            .unix_seconds()
            .map_err(|_| StaffError::ClockUnavailable)?;
        let now = i64::try_from(seconds).map_err(|_| StaffError::ClockUnavailable)?;
        check_session(self.state, now).map_err(|_| StaffError::Denied)?;
        Ok(now)
    }
}
#[derive(sqlx::FromRow)]
struct StaffRow {
    staff_id: String,
    enabled: bool,
    created_at: i64,
    expires_at: i64,
    mfa_verified_at: Option<i64>,
    revoked: bool,
}
pub(super) async fn lock_staff(
    pool: &PgPool,
    token: &SessionToken,
    clock: &dyn Clock,
    capability: Capability,
) -> Result<StaffTransaction, StaffError> {
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
    // SHARE blocks disabling, revocation and grant deletion through the operation's
    // commit. KEY SHARE would incorrectly permit changes to non-key security fields.
    let authority=sqlx::query_as::<_,StaffRow>("SELECT s.staff_id,a.enabled,s.created_at,s.expires_at,s.mfa_verified_at,s.revoked_at IS NOT NULL AS revoked
        FROM staff_sessions s JOIN staff_accounts a ON a.id=s.staff_id
        JOIN staff_capabilities g ON g.staff_id=a.id AND g.capability=$2
        WHERE s.token_hash=$1 FOR SHARE OF s,a,g")
        .bind(token.digest().as_slice()).bind(capability.code()).fetch_optional(&mut *tx).await?;
    // Keep the absence of server authority an explicit, instrumented decision.
    let Some(row) = authority else {
        return Err(StaffError::Denied);
    };
    let access = StaffTransaction {
        tx,
        actor: row.staff_id,
        state: SessionState {
            enabled: row.enabled,
            revoked: row.revoked,
            granted: true,
            created_at: row.created_at,
            expires_at: row.expires_at,
            mfa_verified_at: row.mfa_verified_at,
        },
    };
    // Evaluate time after any authority-row wait, immediately before the operation.
    access.check_time(clock)?;
    Ok(access)
}
