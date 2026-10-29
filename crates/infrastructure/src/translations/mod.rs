//! Versioned copy and independent review attestations; no publication side effects.
use super::staff_access::{SessionToken, StaffError, lock_staff};
use commerce_application::{Locale, request_context::Clock, staff_access::Capability};
use commerce_domain::{
    media::valid_command_id,
    specifications::bounded_text,
    translations::{TranslationDecision, TranslationDraft},
};
use sqlx::{PgPool, Postgres, Transaction};
mod read;
mod review;
mod write;
pub(crate) use read::assembled as load_bundle;
pub use read::{translation, translation_for_review};
pub use review::record_review;
pub use write::record_translation;

pub struct TranslationRequest<'a> {
    pub sku: &'a str,
    pub locale: Locale,
    pub specification_revision: i32,
    pub source_revision: Option<i32>,
    pub expected_revision: i32,
    pub effect_key: &'a str,
    pub draft: &'a TranslationDraft,
    pub reason: &'a str,
    pub confirmed: bool,
}
pub struct TranslationReviewRequest<'a> {
    pub sku: &'a str,
    pub locale: Locale,
    pub translation_revision: i32,
    pub expected_review_revision: i32,
    pub effect_key: &'a str,
    pub decision: TranslationDecision,
    pub evidence_ref: &'a str,
    pub qualification_ref: &'a str,
    pub reason: &'a str,
    pub confirmed: bool,
}
#[derive(Debug, PartialEq, Eq)]
pub struct TranslationReceipt {
    pub revision: i32,
    pub replayed: bool,
}
#[derive(Debug, PartialEq, Eq)]
pub struct TranslationRecord {
    pub sku: String,
    pub locale: Locale,
    pub revision: i32,
    pub specification_revision: i32,
    pub source_revision: Option<i32>,
    pub draft: TranslationDraft,
    pub actor: String,
    pub reason: String,
    pub review: Option<TranslationReview>,
}
#[derive(Debug, PartialEq, Eq)]
pub struct TranslationReview {
    pub revision: i32,
    pub decision: TranslationDecision,
    pub actor: String,
    pub evidence_ref: String,
    pub qualification_ref: String,
    pub reason: String,
}
#[derive(Debug)]
pub enum TranslationError {
    Staff(StaffError),
    Database(sqlx::Error),
    InvalidRequest,
    InvalidStoredData,
    NotFound,
    Conflict,
    SelfReview,
}
impl From<StaffError> for TranslationError {
    fn from(value: StaffError) -> Self {
        Self::Staff(value)
    }
}
impl From<sqlx::Error> for TranslationError {
    fn from(value: sqlx::Error) -> Self {
        Self::Database(value)
    }
}
#[derive(sqlx::FromRow)]
struct StoredDraft {
    sku: String,
    locale: String,
    revision: i32,
    specification_revision: i32,
    source_revision: Option<i32>,
    payload: String,
    actor: String,
    reason: String,
}
#[derive(sqlx::FromRow)]
struct StoredReview {
    sku: String,
    locale: String,
    translation_revision: i32,
    revision: i32,
    decision: String,
    evidence_ref: String,
    qualification_ref: String,
    actor: String,
    reason: String,
}
fn valid_sku(sku: &str) -> bool {
    sku.len() <= 65 && valid_command_id(sku)
}
fn text(value: &str) -> bool {
    bounded_text(value, 500) && !value.contains('\0')
}
fn valid_source(locale: Locale, source: Option<i32>) -> bool {
    match (locale, source) {
        (Locale::Nl, None) => true,
        (Locale::Nl, Some(_)) | (_, None) => false,
        (_, Some(revision)) => revision > 0,
    }
}
async fn effect_lock(
    tx: &mut Transaction<'_, Postgres>,
    kind: &str,
    key: &str,
) -> Result<(), TranslationError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!("catalogue-translation-{kind}:{key}"))
        .execute(&mut **tx)
        .await?;
    Ok(())
}
async fn lock_variant(
    tx: &mut Transaction<'_, Postgres>,
    sku: &str,
) -> Result<(), TranslationError> {
    let row: Option<(String,)> = sqlx::query_as(
        "UPDATE catalogue_variants SET stock_policy=stock_policy WHERE sku=$1 RETURNING sku",
    )
    .bind(sku)
    .fetch_optional(&mut **tx)
    .await?;
    row.ok_or(TranslationError::NotFound)?;
    Ok(())
}
/// Caller holds the variant serialization lock used by specification/copy changes.
async fn current_source(
    tx: &mut Transaction<'_, Postgres>,
    sku: &str,
    locale: Locale,
    specification: i32,
    source: Option<i32>,
) -> Result<bool, TranslationError> {
    let (current_specification, current_source, source_specification): (Option<i32>,Option<i32>,Option<i32>) = sqlx::query_as("SELECT (SELECT max(revision) FROM catalogue_specification_revisions WHERE sku=$1),(SELECT max(revision) FROM catalogue_translation_revisions WHERE sku=$1 AND locale='nl'),(SELECT specification_revision FROM catalogue_translation_revisions WHERE sku=$1 AND locale='nl' ORDER BY revision DESC LIMIT 1)")
        .bind(sku).fetch_one(&mut **tx).await?;
    if current_specification != Some(specification)
        || (locale != Locale::Nl
            && (current_source != source || source_specification != Some(specification)))
    {
        return Ok(false);
    }
    if locale != Locale::Nl {
        read::validate_source_copy(
            tx,
            sku,
            source.ok_or(TranslationError::InvalidStoredData)?,
            specification,
        )
        .await?;
    }
    Ok(true)
}
