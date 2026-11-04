//! Frozen paid demand only; review stops grant no stock or retry authority.
use crate::order_stock::{Balance, Footprint, Member, Settlement, identifier};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
pub const MAX_ENVELOPE_BYTES: usize = 2_097_152;
pub const WINDOW_SECONDS: i64 = 60;
pub const ACTOR: &str = "late-payment-allocation";
pub const REVIEW_ACTOR: &str = "late-stock-review";
pub const EXPIRY_ACTOR: &str = "late-payment-expiry";
mod assessment;
mod receipts;
pub use assessment::{AssessedComponent, LateFootprint, ReviewFootprint, StageComponent};
pub use receipts::{LateAllocation, ReviewCase, ReviewReason};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PolicyError {
    InvalidSnapshot,
    InvalidClock,
    WindowExpired,
}
impl From<crate::order_stock::SettlementPolicyError> for PolicyError {
    fn from(_: crate::order_stock::SettlementPolicyError) -> Self {
        Self::InvalidSnapshot
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OriginalState {
    Active,
    Expired,
    Released,
    Consumed,
}
impl OriginalState {
    pub fn code(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Expired => "expired",
            Self::Released => "released",
            Self::Consumed => "consumed",
        }
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairReference {
    pub component_code: String,
    pub reserve_sequence: i64,
    pub reserve_digest: String,
    pub terminal_sequence: Option<i64>,
    pub terminal_digest: Option<String>,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OriginalMember {
    pub member: Member,
    pub state: OriginalState,
    pub pairs: Vec<PairReference>,
}
impl OriginalMember {
    pub fn validate(&self) -> Result<(), PolicyError> {
        Footprint::demand(std::slice::from_ref(&self.member))?;
        let mut codes = BTreeSet::new();
        if self.pairs.len() != self.member.components.len()
            || self.pairs.iter().any(|pair| {
                !codes.insert(&pair.component_code)
                    || pair.reserve_sequence <= 0
                    || !digest(&pair.reserve_digest)
                    || !self
                        .member
                        .components
                        .iter()
                        .any(|line| line.component_code == pair.component_code)
                    || match self.state {
                        OriginalState::Active => {
                            pair.terminal_sequence.is_some() || pair.terminal_digest.is_some()
                        }
                        _ => {
                            !pair
                                .terminal_sequence
                                .is_some_and(|sequence| sequence > pair.reserve_sequence)
                                || !pair.terminal_digest.as_deref().is_some_and(digest)
                        }
                    }
            })
        {
            return Err(PolicyError::InvalidSnapshot);
        }
        Ok(())
    }
}
pub type Demand = BTreeMap<String, i64>;
pub type Demands = (Demand, Demand);
fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub fn summarize(original: &[OriginalMember]) -> Result<Demands, PolicyError> {
    for member in original {
        member.validate()?;
    }
    let members: Vec<_> = original
        .iter()
        .map(|member| member.member.clone())
        .collect();
    let active: Vec<_> = original
        .iter()
        .filter(|member| member.state == OriginalState::Active)
        .map(|member| member.member.clone())
        .collect();
    Ok((
        Footprint::demand(&members)?,
        if active.is_empty() {
            BTreeMap::new()
        } else {
            Footprint::demand(&active)?
        },
    ))
}
#[derive(Clone, PartialEq, Eq)]
pub enum Outcome {
    Original(Settlement),
    Late(LateAllocation),
    NeedsReview(ReviewCase),
}
pub fn check_time(at: i64, created: i64, recognized: i64) -> Result<(), PolicyError> {
    if created < 0
        || recognized < created
        || recognized > crate::checkout_quotes::MAX_QUOTE_UNIX_SECONDS
    {
        return Err(PolicyError::InvalidSnapshot);
    }
    if at < created || at < recognized || at > crate::checkout_quotes::MAX_QUOTE_UNIX_SECONDS {
        return Err(PolicyError::InvalidClock);
    }
    Ok(())
}
pub fn check_window(at: i64, expires_at: i64) -> Result<(), PolicyError> {
    let created = expires_at
        .checked_sub(WINDOW_SECONDS)
        .ok_or(PolicyError::InvalidSnapshot)?;
    if created < 0 || expires_at > crate::checkout_quotes::MAX_QUOTE_UNIX_SECONDS {
        return Err(PolicyError::InvalidSnapshot);
    }
    if at < created {
        return Err(PolicyError::InvalidClock);
    }
    if at >= expires_at {
        return Err(PolicyError::WindowExpired);
    }
    Ok(())
}
