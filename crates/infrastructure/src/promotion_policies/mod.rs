//! Private reviewed promotion configuration. No coupon, redemption or payment execution.
use crate::staff_access::{SessionToken, StaffError, lock_staff};
use commerce_application::{
    request_context::{Clock, ObservedClock},
    staff_access::Capability,
};
use commerce_domain::{
    catalogue_authoring::merchant_code, media::valid_command_id,
    promotion_policies::PromotionPolicy,
};
use sqlx::{PgPool, Postgres, Transaction};
mod draft;
mod read;
mod review;
pub use draft::{DraftRequest, record_draft};
pub use read::{ApprovedPolicy, PolicyInspection, approved_policy, inspect_policy};
pub use review::{ReviewRequest, record_review};
#[derive(Debug)]
pub enum PolicyError {
    Staff(StaffError),
    InvalidRequest,
    InvalidStoredData,
    Conflict,
    NotFound,
    SelfReview,
    ClockUnavailable,
    Database(sqlx::Error),
}
impl From<StaffError> for PolicyError {
    fn from(v: StaffError) -> Self {
        Self::Staff(v)
    }
}
impl From<sqlx::Error> for PolicyError {
    fn from(v: sqlx::Error) -> Self {
        Self::Database(v)
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
fn text(s: &str) -> bool {
    !s.trim().is_empty() && s.chars().count() <= 500 && !s.chars().any(char::is_control)
}
fn now(clock: &dyn Clock) -> Result<i64, PolicyError> {
    i64::try_from(
        clock
            .unix_seconds()
            .map_err(|_| PolicyError::ClockUnavailable)?,
    )
    .map_err(|_| PolicyError::ClockUnavailable)
}
#[derive(sqlx::FromRow)]
struct StoredDraft {
    policy: Option<serde_json::Value>,
    actor: String,
    reason: String,
}
impl StoredDraft {
    fn decode(&self, id: &str, revision: i32) -> Result<PromotionPolicy, PolicyError> {
        let value = self.policy.as_ref().ok_or(PolicyError::InvalidStoredData)?;
        let promotion = &value["promotion"];
        if !value.is_object()
            || !promotion.is_object()
            || !value["limits"].is_object()
            || !promotion["discount"].is_object()
            || !promotion["products"].as_array().is_some_and(|products| {
                products.len() <= 20 && products.iter().all(serde_json::Value::is_object)
            })
        {
            return Err(PolicyError::InvalidStoredData);
        }
        let policy: PromotionPolicy =
            serde_json::from_value(value.clone()).map_err(|_| PolicyError::InvalidStoredData)?;
        let mut canonical = policy.clone();
        canonical.canonicalize();
        if policy.validate().is_err()
            || policy.promotion.id != id
            || policy.promotion.revision != revision
            || policy != canonical
            || !text(&self.actor)
            || !text(&self.reason)
        {
            return Err(PolicyError::InvalidStoredData);
        }
        Ok(policy)
    }
}
async fn load(
    tx: &mut Transaction<'_, Postgres>,
    id: &str,
    revision: i32,
) -> Result<StoredDraft, PolicyError> {
    sqlx::query_as("SELECT CASE WHEN octet_length(policy::text)<=32768 THEN policy END AS policy,left(actor,501) AS actor,left(reason,501) AS reason FROM promotion_policy_drafts WHERE id=$1 AND revision=$2").bind(id).bind(revision).fetch_optional(&mut **tx).await?.ok_or(PolicyError::NotFound)
}
async fn effect_lock(
    tx: &mut Transaction<'_, Postgres>,
    kind: &str,
    key: &str,
) -> Result<(), PolicyError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!("promotion-{kind}:{key}"))
        .execute(&mut **tx)
        .await?;
    Ok(())
}
async fn claim(tx: &mut Transaction<'_, Postgres>, id: &str) -> Result<i32, PolicyError> {
    let exists: Option<String> =
        sqlx::query_scalar("UPDATE promotion_policy_anchors SET id=id WHERE id=$1 RETURNING id")
            .bind(id)
            .fetch_optional(&mut **tx)
            .await?;
    exists.ok_or(PolicyError::NotFound)?;
    Ok(sqlx::query_scalar(
        "SELECT coalesce(max(revision),0) FROM promotion_policy_drafts WHERE id=$1",
    )
    .bind(id)
    .fetch_one(&mut **tx)
    .await?)
}
