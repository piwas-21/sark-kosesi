//! Independent attestations over exact validated private facts. No publication.
use super::{
    catalogue_authoring::{self, DefinitionError, DefinitionEvent},
    specifications::{self, SpecificationRecord, SpecificationWriteError},
    staff_access::{SessionToken, StaffError, lock_staff},
};
use commerce_application::{request_context::Clock, staff_access::Capability};
use commerce_domain::{
    Money,
    catalogue_authoring::merchant_code,
    factual_review::{FactDecision, FactTarget},
    media::valid_command_id,
    specifications::bounded_text,
};
use sqlx::{PgPool, Postgres, Transaction};
mod read;
mod write;
pub use read::facts_for_review;
pub use write::record_factual_review;

pub struct FactReviewRequest<'a> {
    pub sku: &'a str,
    pub target: FactTarget,
    pub expected_review_revision: i32,
    pub effect_key: &'a str,
    pub decision: FactDecision,
    pub evidence_ref: &'a str,
    pub qualification_ref: &'a str,
    pub reason: &'a str,
    pub confirmed: bool,
}
#[derive(Debug, PartialEq, Eq)]
pub struct FactReceipt {
    pub revision: i32,
    pub replayed: bool,
}
#[derive(Debug, PartialEq, Eq)]
pub struct FactBundle {
    pub sku: String,
    pub target: FactTarget,
    pub identity: DefinitionEvent,
    pub bom: DefinitionEvent,
    pub specification: SpecificationRecord,
    pub specification_actor: String,
    pub specification_reason: String,
    pub price: Money,
    pub price_actor: String,
    pub price_evidence_ref: String,
    pub price_reason: String,
    pub review: Option<FactReview>,
}
impl FactBundle {
    fn authored_by(&self, actor: &str) -> bool {
        [
            &self.identity.actor,
            &self.bom.actor,
            &self.specification_actor,
            &self.price_actor,
        ]
        .iter()
        .any(|author| author.as_str() == actor)
    }
}
#[derive(Debug, PartialEq, Eq)]
pub struct FactReview {
    pub revision: i32,
    pub decision: FactDecision,
    pub evidence_ref: String,
    pub qualification_ref: String,
    pub actor: String,
    pub reason: String,
}
#[derive(Debug)]
pub enum FactError {
    Staff(StaffError),
    Database(sqlx::Error),
    Specification(SpecificationWriteError),
    Definition(DefinitionError),
    InvalidRequest,
    InvalidStoredData,
    NotFound,
    Conflict,
    SelfReview,
}
impl From<StaffError> for FactError {
    fn from(error: StaffError) -> Self {
        Self::Staff(error)
    }
}
impl From<sqlx::Error> for FactError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}
impl From<SpecificationWriteError> for FactError {
    fn from(error: SpecificationWriteError) -> Self {
        Self::Specification(error)
    }
}
impl From<DefinitionError> for FactError {
    fn from(error: DefinitionError) -> Self {
        Self::Definition(error)
    }
}
fn text(value: &str) -> bool {
    bounded_text(value, 500) && !value.contains('\0')
}
#[derive(sqlx::FromRow)]
struct StoredReview {
    sku: String,
    bom_revision: i32,
    specification_revision: i32,
    price_revision: i32,
    revision: i32,
    decision: String,
    evidence_ref: String,
    qualification_ref: String,
    actor: String,
    reason: String,
}
impl StoredReview {
    fn target(&self) -> FactTarget {
        FactTarget {
            bom_revision: self.bom_revision,
            specification_revision: self.specification_revision,
            price_revision: self.price_revision,
        }
    }
    fn validated(self, bundle: &FactBundle) -> Result<FactReview, FactError> {
        if self.sku != bundle.sku
            || self.target() != bundle.target
            || self.revision <= 0
            || !valid_command_id(&self.actor)
            || bundle.authored_by(&self.actor)
            || !text(&self.evidence_ref)
            || !text(&self.qualification_ref)
            || !text(&self.reason)
        {
            return Err(FactError::InvalidStoredData);
        }
        let decision = match self.decision.as_str() {
            "approved" => FactDecision::Approved,
            "rejected" => FactDecision::Rejected,
            _ => return Err(FactError::InvalidStoredData),
        };
        Ok(FactReview {
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
#[path = "../../tests/support/fact_reviews.rs"]
mod tests;
