//! Exact immutable facts under review; an attestation never publishes a product.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FactTarget {
    pub bom_revision: i32,
    pub specification_revision: i32,
    pub price_revision: i32,
}
impl FactTarget {
    pub fn valid(self) -> bool {
        self.bom_revision > 0 && self.specification_revision > 0 && self.price_revision > 0
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FactDecision {
    Approved,
    Rejected,
}
impl FactDecision {
    pub fn code(self) -> &'static str {
        match self {
            Self::Approved => "approved",
            Self::Rejected => "rejected",
        }
    }
}
