//! Guarded append-only draft prices; publication is a separate validation step.
use super::staff_access::{SessionToken, StaffError, lock_staff};
use commerce_application::{request_context::Clock, staff_access::Capability};
use commerce_domain::Money;
use sqlx::PgPool;

pub struct PriceRevisionRequest<'a> {
    pub sku: &'a str,
    pub expected_revision: i32,
    pub effect_key: &'a str,
    pub price: Money,
    pub evidence_ref: &'a str,
    pub reason: &'a str,
    pub confirmed: bool,
}
#[derive(Debug, PartialEq, Eq)]
pub struct PriceRevisionReceipt {
    pub revision: i32,
    pub price: Money,
    pub replayed: bool,
}
#[derive(Debug)]
pub enum PriceRevisionError {
    Staff(StaffError),
    InvalidRequest,
    NotFound,
    Conflict,
    Database(sqlx::Error),
}
impl From<StaffError> for PriceRevisionError {
    fn from(value: StaffError) -> Self {
        Self::Staff(value)
    }
}
impl From<sqlx::Error> for PriceRevisionError {
    fn from(value: sqlx::Error) -> Self {
        Self::Database(value)
    }
}
#[derive(sqlx::FromRow)]
struct StoredPrice {
    sku: String,
    revision: i32,
    minor_units: i64,
    evidence_ref: String,
    actor: String,
    reason: String,
}
fn bounded_text(value: &str, maximum: usize) -> bool {
    !value.trim().is_empty() && value.chars().count() <= maximum
}
/// No actor, role or publication status can be supplied by the caller.
pub async fn record_price_revision(
    pool: &PgPool,
    token: &SessionToken,
    clock: &dyn Clock,
    request: PriceRevisionRequest<'_>,
) -> Result<PriceRevisionReceipt, PriceRevisionError> {
    let mut access = lock_staff(pool, token, clock, Capability::CatalogueManage).await?;
    if !request.confirmed
        || request.expected_revision < 0
        || request.expected_revision == i32::MAX
        || request.sku.is_empty()
        || request.sku.len() > 65
        || request.effect_key.is_empty()
        || request.effect_key.len() > 128
        || !request
            .effect_key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        || !bounded_text(request.evidence_ref, 512)
        || !bounded_text(request.reason, 500)
    {
        return Err(PriceRevisionError::InvalidRequest);
    }
    // Global effect serialization precedes SKU locks: cross-SKU retries cannot
    // race the uniqueness check. Hash collisions only serialize unrelated work.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("catalogue-price:{}", request.effect_key))
        .execute(&mut *access.tx)
        .await?;
    let prior = sqlx::query_as::<_, StoredPrice>("SELECT sku,revision,minor_units,evidence_ref,actor,reason FROM catalogue_price_revisions WHERE effect_key=$1")
        .bind(request.effect_key).fetch_optional(&mut *access.tx).await?;
    if let Some(prior) = prior {
        if prior.sku != request.sku
            || prior.revision - 1 != request.expected_revision
            || prior.minor_units != request.price.minor_units()
            || prior.evidence_ref != request.evidence_ref
            || prior.actor != access.actor
            || prior.reason != request.reason
        {
            return Err(PriceRevisionError::Conflict);
        }
        access.check_time(clock)?;
        access.tx.rollback().await?;
        return Ok(PriceRevisionReceipt {
            revision: prior.revision,
            price: request.price,
            replayed: true,
        });
    }
    let variant: Option<(String,)> =
        sqlx::query_as("SELECT sku FROM catalogue_variants WHERE sku=$1 FOR UPDATE")
            .bind(request.sku)
            .fetch_optional(&mut *access.tx)
            .await?;
    if variant.is_none() {
        return Err(PriceRevisionError::NotFound);
    }
    let (current,): (i32,) = sqlx::query_as(
        "SELECT coalesce(max(revision),0) FROM catalogue_price_revisions WHERE sku=$1",
    )
    .bind(request.sku)
    .fetch_one(&mut *access.tx)
    .await?;
    if current != request.expected_revision {
        return Err(PriceRevisionError::Conflict);
    }
    let revision = current
        .checked_add(1)
        .ok_or(PriceRevisionError::InvalidRequest)?;
    sqlx::query("INSERT INTO catalogue_price_revisions (sku,revision,effect_key,minor_units,evidence_ref,actor,reason) VALUES ($1,$2,$3,$4,$5,$6,$7)")
        .bind(request.sku).bind(revision).bind(request.effect_key).bind(request.price.minor_units())
        .bind(request.evidence_ref).bind(&access.actor).bind(request.reason).execute(&mut *access.tx).await?;
    // Authority may have expired while waiting for either effect or SKU locks.
    access.check_time(clock)?;
    access.tx.commit().await?;
    Ok(PriceRevisionReceipt {
        revision,
        price: request.price,
        replayed: false,
    })
}
