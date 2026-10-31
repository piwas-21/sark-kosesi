//! Private reviewed exact-destination policies. No installed readiness or HTTP writes.
use crate::staff_access::{SessionToken, StaffError, lock_staff};
use commerce_application::{request_context::Clock, staff_access::Capability};
use commerce_domain::{catalogue_authoring::merchant_code, media::valid_command_id, quotes::*};
use sqlx::{PgPool, Postgres, Transaction};
mod draft;
mod read;
mod review;
pub use draft::{DraftRequest, record_draft};
pub use read::{PolicyInspection, inspect_policy, reviewed_promotion_quote, reviewed_quote};
pub use review::{ReviewRequest, record_review};

#[derive(Debug)]
pub enum MarketError {
    Staff(StaffError),
    InvalidRequest,
    InvalidStoredData,
    Conflict,
    NotFound,
    SelfReview,
    Database(sqlx::Error),
    Quote(crate::quotes::QuoteFailure),
}
impl From<StaffError> for MarketError {
    fn from(value: StaffError) -> Self {
        Self::Staff(value)
    }
}
impl From<sqlx::Error> for MarketError {
    fn from(value: sqlx::Error) -> Self {
        Self::Database(value)
    }
}
#[derive(Debug, PartialEq, Eq)]
pub struct Receipt {
    pub revision: i32,
    pub replayed: bool,
}
#[derive(Clone, Copy)]
pub struct Audit<'a> {
    pub expected_revision: i32,
    pub effect_key: &'a str,
    pub reason: &'a str,
    pub confirmed: bool,
}
impl Audit<'_> {
    fn valid(self) -> bool {
        self.confirmed
            && (0..i32::MAX).contains(&self.expected_revision)
            && valid_command_id(self.effect_key)
            && text(self.reason)
    }
}
fn text(value: &str) -> bool {
    !value.trim().is_empty() && value.chars().count() <= 500 && !value.chars().any(char::is_control)
}
#[derive(sqlx::FromRow)]
struct StoredDraft {
    policy: serde_json::Value,
    actor: String,
    reason: String,
}
impl StoredDraft {
    fn decode(&self, id: &str, revision: i32) -> Result<MarketPolicy, MarketError> {
        let p: MarketPolicy = serde_json::from_value(self.policy.clone())
            .map_err(|_| MarketError::InvalidStoredData)?;
        if p.validate().is_err() || p.id != id || p.revision != revision || !text(&self.reason) {
            return Err(MarketError::InvalidStoredData);
        }
        Ok(p)
    }
}
async fn load(
    tx: &mut Transaction<'_, Postgres>,
    id: &str,
    revision: i32,
) -> Result<StoredDraft, MarketError> {
    sqlx::query_as(
        "SELECT policy,actor,reason FROM market_policy_drafts WHERE id=$1 AND revision=$2",
    )
    .bind(id)
    .bind(revision)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(MarketError::NotFound)
}
async fn effect_lock(
    tx: &mut Transaction<'_, Postgres>,
    kind: &str,
    key: &str,
) -> Result<(), MarketError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!("market-{kind}:{key}"))
        .execute(&mut **tx)
        .await?;
    Ok(())
}
async fn claim(tx: &mut Transaction<'_, Postgres>, id: &str) -> Result<i32, MarketError> {
    let exists: Option<String> =
        sqlx::query_scalar("UPDATE market_policy_anchors SET id=id WHERE id=$1 RETURNING id")
            .bind(id)
            .fetch_optional(&mut **tx)
            .await?;
    exists.ok_or(MarketError::NotFound)?;
    Ok(
        sqlx::query_scalar(
            "SELECT coalesce(max(revision),0) FROM market_policy_drafts WHERE id=$1",
        )
        .bind(id)
        .fetch_one(&mut **tx)
        .await?,
    )
}
