//! Staff-only immutable specification drafts; every read revalidates stored input.
use super::{
    bom,
    staff_access::{SessionToken, StaffError, lock_staff},
};
use commerce_application::{request_context::Clock, staff_access::Capability};
use commerce_domain::specifications::{SpecificationDraft, bounded_text};
use sqlx::{PgPool, Postgres, Transaction};

pub struct SpecificationRequest<'a> {
    pub sku: &'a str,
    pub bom_revision: i32,
    pub expected_revision: i32,
    pub effect_key: &'a str,
    pub draft: &'a SpecificationDraft,
    pub reason: &'a str,
    pub confirmed: bool,
}
#[derive(Debug, PartialEq, Eq)]
pub struct SpecificationReceipt {
    pub revision: i32,
    pub replayed: bool,
}
#[derive(Debug, PartialEq, Eq)]
pub struct SpecificationRecord {
    pub bom_revision: i32,
    pub draft: SpecificationDraft,
}
#[derive(Debug)]
pub enum SpecificationWriteError {
    Staff(StaffError),
    InvalidRequest,
    NotFound,
    Conflict,
    InvalidStoredData,
    Database(sqlx::Error),
}
impl From<StaffError> for SpecificationWriteError {
    fn from(value: StaffError) -> Self {
        Self::Staff(value)
    }
}
impl From<sqlx::Error> for SpecificationWriteError {
    fn from(value: sqlx::Error) -> Self {
        Self::Database(value)
    }
}
#[derive(sqlx::FromRow)]
struct StoredSpecification {
    sku: String,
    revision: i32,
    bom_revision: i32,
    payload: String,
    actor: String,
    reason: String,
}
fn valid_identity(sku: &str, revision: i32) -> bool {
    !sku.is_empty() && sku.len() <= 65 && revision > 0
}

pub async fn record_specification(
    pool: &PgPool,
    token: &SessionToken,
    clock: &dyn Clock,
    request: SpecificationRequest<'_>,
) -> Result<SpecificationReceipt, SpecificationWriteError> {
    let mut access = lock_staff(pool, token, clock, Capability::CatalogueManage).await?;
    if !request.confirmed
        || !valid_identity(request.sku, request.bom_revision)
        || !(0..i32::MAX).contains(&request.expected_revision)
        || request.effect_key.is_empty()
        || request.effect_key.len() > 128
        || !request
            .effect_key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        || !bounded_text(request.reason, 500)
    {
        return Err(SpecificationWriteError::InvalidRequest);
    }
    // Full drafts are bounded before serialization, including collection cardinality.
    let sealed = bom::load(&mut *access.tx, request.sku, request.bom_revision)
        .await?
        .ok_or(SpecificationWriteError::NotFound)?;
    request
        .draft
        .validate(&sealed)
        .map_err(|_| SpecificationWriteError::InvalidRequest)?;
    let payload =
        serde_json::to_value(request.draft).map_err(|_| SpecificationWriteError::InvalidRequest)?;
    let encoded = payload.to_string();
    if encoded.len() > 65536 {
        return Err(SpecificationWriteError::InvalidRequest);
    }
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!("catalogue-specification:{}", request.effect_key))
        .execute(&mut *access.tx)
        .await?;
    let prior=sqlx::query_as::<_,StoredSpecification>("SELECT sku,revision,bom_revision,payload::text,actor,reason FROM catalogue_specification_revisions WHERE effect_key=$1")
        .bind(request.effect_key).fetch_optional(&mut *access.tx).await?;
    if let Some(prior) = prior {
        let stored: serde_json::Value = serde_json::from_str(&prior.payload)
            .map_err(|_| SpecificationWriteError::InvalidStoredData)?;
        if prior.sku != request.sku
            || prior.revision - 1 != request.expected_revision
            || prior.bom_revision != request.bom_revision
            || stored != payload
            || prior.actor != access.actor
            || prior.reason != request.reason
        {
            return Err(SpecificationWriteError::Conflict);
        }
        access.check_time(clock)?;
        access.tx.rollback().await?;
        return Ok(SpecificationReceipt {
            revision: prior.revision,
            replayed: true,
        });
    }
    sqlx::query("SELECT sku FROM catalogue_variants WHERE sku=$1 FOR UPDATE")
        .bind(request.sku)
        .execute(&mut *access.tx)
        .await?;
    let (current,): (i32,) = sqlx::query_as(
        "SELECT coalesce(max(revision),0) FROM catalogue_specification_revisions WHERE sku=$1",
    )
    .bind(request.sku)
    .fetch_one(&mut *access.tx)
    .await?;
    if current != request.expected_revision {
        return Err(SpecificationWriteError::Conflict);
    }
    let revision = current
        .checked_add(1)
        .ok_or(SpecificationWriteError::InvalidRequest)?;
    sqlx::query("INSERT INTO catalogue_specification_revisions (sku,revision,bom_revision,effect_key,payload,actor,reason) VALUES ($1,$2,$3,$4,$5::jsonb,$6,$7)")
        .bind(request.sku).bind(revision).bind(request.bom_revision).bind(request.effect_key).bind(encoded)
        .bind(&access.actor).bind(request.reason).execute(&mut *access.tx).await?;
    access.check_time(clock)?;
    access.tx.commit().await?;
    Ok(SpecificationReceipt {
        revision,
        replayed: false,
    })
}

pub async fn specification(
    pool: &PgPool,
    token: &SessionToken,
    clock: &dyn Clock,
    sku: &str,
    revision: i32,
) -> Result<Option<SpecificationRecord>, SpecificationWriteError> {
    let mut access = lock_staff(pool, token, clock, Capability::CatalogueManage).await?;
    if !valid_identity(sku, revision) {
        return Err(SpecificationWriteError::InvalidRequest);
    }
    let record = load(&mut access.tx, sku, revision).await?;
    access.check_time(clock)?;
    access.tx.rollback().await?;
    Ok(record)
}

/// Caller owns staff authority and any required variant serialization lock.
pub(crate) async fn load(
    tx: &mut Transaction<'_, Postgres>,
    sku: &str,
    revision: i32,
) -> Result<Option<SpecificationRecord>, SpecificationWriteError> {
    let row:Option<(i32,String)>=sqlx::query_as("SELECT bom_revision,payload::text FROM catalogue_specification_revisions WHERE sku=$1 AND revision=$2")
        .bind(sku).bind(revision).fetch_optional(&mut **tx).await?;
    let Some((bom_revision, payload)) = row else {
        return Ok(None);
    };
    if payload.len() > 131072 {
        return Err(SpecificationWriteError::InvalidStoredData);
    }
    let draft: SpecificationDraft =
        serde_json::from_str(&payload).map_err(|_| SpecificationWriteError::InvalidStoredData)?;
    let sealed = bom::load(&mut **tx, sku, bom_revision)
        .await?
        .ok_or(SpecificationWriteError::InvalidStoredData)?;
    draft
        .validate(&sealed)
        .map_err(|_| SpecificationWriteError::InvalidStoredData)?;
    Ok(Some(SpecificationRecord {
        bom_revision,
        draft,
    }))
}
