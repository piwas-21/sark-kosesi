//! An owned indicative promotion snapshot; it grants no review or spending authority.
use super::{CapturedAmount, QuoteSnapshot, SnapshotError};
use crate::{
    promotion_policies::PromotionPolicy,
    promotions::{self, PromotionQuote},
};
use serde::{Deserialize, Serialize};

const PROMOTED_SNAPSHOT_VERSION: u16 = 2;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromotionReference {
    pub id: String,
    pub revision: i32,
    pub review_revision: i32,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromotedLine {
    pub sku: String,
    pub discount_minor: i64,
    pub amounts: CapturedAmount,
}
/// The caller must independently establish ownership and supply the exact trusted
/// historical policy and review identity. This envelope itself proves neither.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromotedQuoteSnapshot {
    pub schema_version: u16,
    pub original: QuoteSnapshot,
    pub promotion: PromotionReference,
    pub allocation_version: u16,
    pub discount_minor: i64,
    pub lines: Vec<PromotedLine>,
    pub shipping: CapturedAmount,
    pub total: CapturedAmount,
}
impl PromotedQuoteSnapshot {
    /// Capture server-derived amounts against a separately trusted policy source.
    pub fn capture(
        original: QuoteSnapshot,
        policy: &PromotionPolicy,
        review_revision: i32,
    ) -> Result<Self, SnapshotError> {
        if policy.limits.per_customer_redemptions.is_some() {
            return Err(SnapshotError::InvalidSnapshot);
        }
        Self::capture_verified(original, policy, review_revision)
    }
    /// Capture for a caller that holds separately verified customer authority.
    pub fn capture_verified(
        original: QuoteSnapshot,
        policy: &PromotionPolicy,
        review_revision: i32,
    ) -> Result<Self, SnapshotError> {
        if review_revision <= 0
            || policy.validate().is_err()
            || original.binding.expires_at > policy.promotion.expires_at
        {
            return Err(SnapshotError::InvalidSnapshot);
        }
        original.check_at(original.binding.created_at)?;
        let priced = promotions::apply(
            &original.totals()?,
            &policy.promotion,
            &original.binding.country,
            original.binding.created_at,
        )
        .map_err(|_| SnapshotError::InvalidSnapshot)?;
        Ok(Self::from_priced(original, &priced, review_revision))
    }
    fn from_priced(original: QuoteSnapshot, priced: &PromotionQuote, review_revision: i32) -> Self {
        Self {
            schema_version: PROMOTED_SNAPSHOT_VERSION,
            original,
            promotion: PromotionReference {
                id: priced.promotion_id.clone(),
                revision: priced.promotion_revision,
                review_revision,
            },
            allocation_version: priced.allocation_version,
            discount_minor: priced.discount.minor_units(),
            lines: priced
                .lines
                .iter()
                .map(|line| PromotedLine {
                    sku: line.original.sku.clone(),
                    discount_minor: line.discount.minor_units(),
                    amounts: CapturedAmount::from(&line.amounts),
                })
                .collect(),
            shipping: CapturedAmount::from(&priced.shipping),
            total: CapturedAmount::from(&priced.total),
        }
    }
    /// Historical revalidation only. Callers must obtain this exact source from
    /// their trusted history; the numeric review reference does not assert approval.
    pub fn validate(
        &self,
        policy: &PromotionPolicy,
        review_revision: i32,
    ) -> Result<(), SnapshotError> {
        let expected = Self::capture(self.original.clone(), policy, review_revision)?;
        if *self != expected {
            return Err(SnapshotError::InvalidSnapshot);
        }
        Ok(())
    }
    /// Revalidate an envelope whose customer authority is established separately.
    pub fn validate_verified(
        &self,
        policy: &PromotionPolicy,
        review_revision: i32,
    ) -> Result<(), SnapshotError> {
        let expected = Self::capture_verified(self.original.clone(), policy, review_revision)?;
        if *self != expected {
            return Err(SnapshotError::InvalidSnapshot);
        }
        Ok(())
    }
    /// Check the captured time window, not live review, stock or budget authority.
    pub fn check_at(
        &self,
        policy: &PromotionPolicy,
        review_revision: i32,
        at: i64,
    ) -> Result<(), SnapshotError> {
        self.validate(policy, review_revision)?;
        self.check_window(policy, at)
    }
    pub fn check_at_verified(
        &self,
        policy: &PromotionPolicy,
        review_revision: i32,
        at: i64,
    ) -> Result<(), SnapshotError> {
        self.validate_verified(policy, review_revision)?;
        self.check_window(policy, at)
    }
    fn check_window(&self, policy: &PromotionPolicy, at: i64) -> Result<(), SnapshotError> {
        self.original.check_at(at)?;
        policy
            .permits(&self.original.binding.country, at)
            .map_err(|_| SnapshotError::Expired)?;
        Ok(())
    }
}
