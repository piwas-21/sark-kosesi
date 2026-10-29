//! Audited activation of exact approved snapshots. No HTTP or production enablement.
use super::{
    factual_review::{self, FactBundle, FactError},
    image_review::{self, ImageReviewBundle, ImageReviewError},
    staff_access::{SessionToken, StaffError, lock_staff},
    translations::{self, TranslationError, TranslationRecord},
};
use commerce_application::{Locale, request_context::Clock, staff_access::Capability};
use commerce_domain::{
    catalogue_authoring::{CatalogueCommand, merchant_code},
    factual_review::FactDecision,
    image_review::ImageDecision,
    media::valid_command_id,
    publication::{ApprovedSelection, PublicationChoice},
    specifications::bounded_text,
    translations::TranslationDecision,
};
use sqlx::{PgPool, Postgres, Transaction};
mod read;
mod sources;
mod write;
pub(crate) use read::latest_record;
pub use read::publication_status;
pub(crate) use sources::{Sources, load as load_sources};
pub use write::record_publication;

pub struct PublicationRequest<'a> {
    pub sku: &'a str,
    pub locale: Locale,
    pub expected_revision: i32,
    pub effect_key: &'a str,
    pub choice: &'a PublicationChoice,
    pub evidence_ref: &'a str,
    pub reason: &'a str,
    pub confirmed: bool,
}
#[derive(Debug, PartialEq, Eq)]
pub struct PublicationReceipt {
    pub revision: i32,
    pub replayed: bool,
}
#[derive(Debug, PartialEq, Eq)]
pub struct PublicationRecord {
    pub revision: i32,
    pub choice: PublicationChoice,
    pub evidence_ref: String,
    pub actor: String,
    pub reason: String,
    pub evaluated_at: i64,
}
#[derive(Debug, PartialEq, Eq)]
pub struct PublicationStatus {
    pub record: PublicationRecord,
    pub eligible: bool,
    pub evaluated_at: i64,
}
#[derive(Debug)]
pub enum PublicationError {
    Staff(StaffError),
    Database(sqlx::Error),
    Facts(FactError),
    Image(ImageReviewError),
    Copy(TranslationError),
    InvalidRequest,
    InvalidStoredData,
    NotFound,
    Conflict,
    NotEligible,
}
impl From<StaffError> for PublicationError {
    fn from(e: StaffError) -> Self {
        Self::Staff(e)
    }
}
impl From<sqlx::Error> for PublicationError {
    fn from(e: sqlx::Error) -> Self {
        Self::Database(e)
    }
}
impl From<FactError> for PublicationError {
    fn from(e: FactError) -> Self {
        Self::Facts(e)
    }
}
impl From<ImageReviewError> for PublicationError {
    fn from(e: ImageReviewError) -> Self {
        Self::Image(e)
    }
}
impl From<TranslationError> for PublicationError {
    fn from(e: TranslationError) -> Self {
        Self::Copy(e)
    }
}
fn text(value: &str) -> bool {
    bounded_text(value, 500) && !value.contains('\0')
}
fn valid_selection_locale(choice: &PublicationChoice, locale: Locale) -> bool {
    match choice {
        PublicationChoice::Activate { selection } => {
            selection.valid()
                && (selection.source_review_revision.is_none() == (locale == Locale::Nl))
        }
        PublicationChoice::Withdraw {} => true,
    }
}
#[derive(sqlx::FromRow)]
struct Stored {
    sku: String,
    locale: String,
    revision: i32,
    choice: String,
    evidence_ref: String,
    actor: String,
    reason: String,
    evaluated_at: i64,
}
impl Stored {
    fn decode(self, sku: &str, locale: Locale) -> Result<PublicationRecord, PublicationError> {
        if self.sku != sku
            || self.locale != locale.internal()
            || self.revision <= 0
            || self.choice.len() > 8192
            || self.evaluated_at < 0
            || !valid_command_id(&self.actor)
            || !text(&self.evidence_ref)
            || !text(&self.reason)
        {
            return Err(PublicationError::InvalidStoredData);
        }
        let choice: PublicationChoice =
            serde_json::from_str(&self.choice).map_err(|_| PublicationError::InvalidStoredData)?;
        if !valid_selection_locale(&choice, locale) {
            return Err(PublicationError::InvalidStoredData);
        }
        Ok(PublicationRecord {
            revision: self.revision,
            choice,
            evidence_ref: self.evidence_ref,
            actor: self.actor,
            reason: self.reason,
            evaluated_at: self.evaluated_at,
        })
    }
}
pub(crate) async fn current(
    tx: &mut Transaction<'_, Postgres>,
    sku: &str,
    locale: Locale,
    choice: &PublicationChoice,
    now: i64,
) -> Result<bool, PublicationError> {
    let encoded = serde_json::to_string(choice).map_err(|_| PublicationError::InvalidStoredData)?;
    Ok(
        sqlx::query_scalar("SELECT publication_sources_current($1,$2,$3::jsonb,$4)")
            .bind(sku)
            .bind(locale.internal())
            .bind(encoded)
            .bind(now)
            .fetch_one(&mut **tx)
            .await?,
    )
}

#[cfg(test)]
#[path = "../../tests/support/publication_rows.rs"]
mod tests;
