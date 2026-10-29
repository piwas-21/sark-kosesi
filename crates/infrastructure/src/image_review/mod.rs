//! Independent SKU/image suitability; rights and actual merchant acceptance stay separate.
use super::{
    catalogue_authoring::{self, DefinitionError, DefinitionEvent},
    image_ingestion::{self, ImageError, ImageReceipt, PrivateRendition},
    media::{self, MediaError},
    specifications::{self, SpecificationRecord, SpecificationWriteError},
    staff_access::{SessionToken, StaffError, lock_staff},
};
use commerce_application::{request_context::Clock, staff_access::Capability};
use commerce_domain::{
    catalogue_authoring::merchant_code,
    image_review::{ImageDecision, ImageTarget},
    media::{MediaAssetDraft, MediaRightsChange, valid_command_id},
    specifications::bounded_text,
};
use sqlx::{PgPool, Postgres, Transaction};
mod read;
mod write;
pub(crate) use read::load as load_bundle;
pub use read::{image_for_review, image_review_bundle};
pub use write::record_image_review;

pub struct ImageReviewRequest<'a> {
    pub sku: &'a str,
    pub target: &'a ImageTarget,
    pub expected_revision: i32,
    pub effect_key: &'a str,
    pub decision: ImageDecision,
    pub evidence_ref: &'a str,
    pub qualification_ref: &'a str,
    pub reason: &'a str,
    pub confirmed: bool,
}
#[derive(Debug, PartialEq, Eq)]
pub struct ImageReviewReceipt {
    pub revision: i32,
    pub replayed: bool,
}
#[derive(Debug, PartialEq, Eq)]
pub struct ImageReviewBundle {
    pub sku: String,
    pub target: ImageTarget,
    pub identity: DefinitionEvent,
    pub bom: DefinitionEvent,
    pub specification: SpecificationRecord,
    pub specification_actor: String,
    pub asset: MediaAssetDraft,
    pub registration_actor: String,
    pub ingestion_actor: String,
    pub rights_actor: String,
    pub rights: MediaRightsChange,
    pub image: ImageReceipt,
    pub review: Option<ImageReview>,
}
impl ImageReviewBundle {
    fn authored_by(&self, actor: &str) -> bool {
        [
            &self.identity.actor,
            &self.bom.actor,
            &self.specification_actor,
            &self.registration_actor,
            &self.ingestion_actor,
            &self.rights_actor,
        ]
        .iter()
        .any(|author| author.as_str() == actor)
    }
}
#[derive(Debug, PartialEq, Eq)]
pub struct ImageReview {
    pub revision: i32,
    pub decision: ImageDecision,
    pub evidence_ref: String,
    pub qualification_ref: String,
    pub actor: String,
    pub reason: String,
}
#[derive(Debug)]
pub enum ImageReviewError {
    Staff(StaffError),
    Database(sqlx::Error),
    Definition(DefinitionError),
    Specification(SpecificationWriteError),
    Media(MediaError),
    Image(ImageError),
    InvalidRequest,
    InvalidStoredData,
    NotFound,
    Conflict,
    SelfReview,
}
macro_rules! convert {
    ($source:ty, $variant:ident) => {
        impl From<$source> for ImageReviewError {
            fn from(error: $source) -> Self {
                Self::$variant(error)
            }
        }
    };
}
convert!(StaffError, Staff);
convert!(sqlx::Error, Database);
convert!(DefinitionError, Definition);
convert!(SpecificationWriteError, Specification);
convert!(MediaError, Media);
convert!(ImageError, Image);
fn text(value: &str) -> bool {
    bounded_text(value, 500) && !value.contains('\0')
}
#[derive(sqlx::FromRow)]
struct StoredReview {
    sku: String,
    asset_id: String,
    bom_revision: i32,
    specification_revision: i32,
    rights_revision: i32,
    revision: i32,
    decision: String,
    evidence_ref: String,
    qualification_ref: String,
    actor: String,
    reason: String,
}
impl StoredReview {
    fn target(&self) -> ImageTarget {
        ImageTarget {
            asset_id: self.asset_id.clone(),
            bom_revision: self.bom_revision,
            specification_revision: self.specification_revision,
            rights_revision: self.rights_revision,
        }
    }
    fn validated(self, bundle: &ImageReviewBundle) -> Result<ImageReview, ImageReviewError> {
        if self.sku != bundle.sku
            || self.target() != bundle.target
            || self.revision <= 0
            || !valid_command_id(&self.actor)
            || bundle.authored_by(&self.actor)
            || !text(&self.evidence_ref)
            || !text(&self.qualification_ref)
            || !text(&self.reason)
        {
            return Err(ImageReviewError::InvalidStoredData);
        }
        let decision = match self.decision.as_str() {
            "approved" => ImageDecision::Approved,
            "rejected" => ImageDecision::Rejected,
            _ => return Err(ImageReviewError::InvalidStoredData),
        };
        Ok(ImageReview {
            revision: self.revision,
            decision,
            evidence_ref: self.evidence_ref,
            qualification_ref: self.qualification_ref,
            actor: self.actor,
            reason: self.reason,
        })
    }
}

#[cfg(test)]
#[path = "../../tests/support/image_review_rows.rs"]
mod tests;
