//! Independently reviewed promotion terms; limits are not a redemption or payment grant.
use crate::promotions::{Promotion, PromotionError};
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RedemptionLimits {
    /// Lifetime across policy revisions, never a fresh allowance per revision.
    pub total_discount_minor: i64,
    pub total_redemptions: u32,
    /// Requires a separately verified customer identity; a guest cookie is insufficient.
    pub per_customer_redemptions: Option<u32>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromotionPolicy {
    pub promotion: Promotion,
    pub limits: RedemptionLimits,
}
impl PromotionPolicy {
    pub fn validate(&self) -> Result<(), PromotionError> {
        self.promotion.validate()?;
        let limits = &self.limits;
        if self.promotion.expires_at > 9_007_199_254_740_991
            || !(1..=crate::quotes::MAX_MINOR_UNITS).contains(&limits.total_discount_minor)
            || !(1..=1_000_000).contains(&limits.total_redemptions)
            || limits
                .per_customer_redemptions
                .is_some_and(|n| n == 0 || n > limits.total_redemptions)
        {
            return Err(PromotionError::InvalidPolicy);
        }
        Ok(())
    }
    pub fn permits(&self, country: &str, at: i64) -> Result<(), PromotionError> {
        self.validate()?;
        self.promotion.permits(country, at)
    }
    pub fn canonicalize(&mut self) {
        self.promotion.countries.sort();
        self.promotion.products.sort_by(|a, b| a.sku.cmp(&b.sku));
    }
}
