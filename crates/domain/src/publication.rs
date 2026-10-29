//! Explicit approved-version selection, never draft fallback or automatic publication.
use crate::{factual_review::FactTarget, image_review::ImageTarget};
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovedSelection {
    pub facts: FactTarget,
    pub factual_review_revision: i32,
    pub image: ImageTarget,
    pub image_review_revision: i32,
    pub translation_revision: i32,
    pub translation_review_revision: i32,
    pub source_review_revision: Option<i32>,
    pub family_slug: String,
}
impl ApprovedSelection {
    pub fn valid(&self) -> bool {
        self.facts.valid()
            && self.image.valid()
            && self.facts.bom_revision == self.image.bom_revision
            && self.facts.specification_revision == self.image.specification_revision
            && self.factual_review_revision > 0
            && self.image_review_revision > 0
            && self.translation_revision > 0
            && self.translation_review_revision > 0
            && self
                .source_review_revision
                .is_none_or(|revision| revision > 0)
            && valid_slug(&self.family_slug)
    }
}
pub fn valid_slug(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 100
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !value.starts_with('-')
        && !value.ends_with('-')
        && !value.contains("--")
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum PublicationChoice {
    Activate { selection: ApprovedSelection },
    Withdraw {},
}
impl PublicationChoice {
    pub fn valid(&self) -> bool {
        match self {
            Self::Activate { selection } => selection.valid(),
            Self::Withdraw {} => true,
        }
    }
}
