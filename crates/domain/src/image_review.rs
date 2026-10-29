//! Image suitability attestations bind immutable private sources, never publication.
use crate::media::valid_command_id;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageTarget {
    pub asset_id: String,
    pub bom_revision: i32,
    pub specification_revision: i32,
    pub rights_revision: i32,
}
impl ImageTarget {
    pub fn valid(&self) -> bool {
        valid_command_id(&self.asset_id)
            && self.bom_revision > 0
            && self.specification_revision > 0
            && self.rights_revision > 0
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageDecision {
    Approved,
    Rejected,
}
impl ImageDecision {
    pub fn code(self) -> &'static str {
        match self {
            Self::Approved => "approved",
            Self::Rejected => "rejected",
        }
    }
}
