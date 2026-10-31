//! Retained source context for a caller which persists a quote before final validation.
use super::*;
pub(crate) struct QuoteProof {
    pub(crate) original: ServerQuote,
    pub(crate) promoted: Option<commerce_domain::promotions::PromotionQuote>,
    pub(super) policy: MarketPolicy,
    pub(super) promotion: Option<commerce_domain::promotions::Promotion>,
    pub(super) products: Vec<published_catalogue::EligibleProduct>,
    pub(super) checked_at: i64,
}
impl QuoteProof {
    pub(crate) fn check_at(&mut self, at: i64) -> Result<(), QuoteFailure> {
        if at < self.checked_at {
            return Err(QuoteFailure::ClockUnavailable);
        }
        self.policy.permits(&self.policy.destination, at)?;
        if let Some(promotion) = &self.promotion {
            promotion
                .permits(&self.policy.destination.country, at)
                .map_err(QuoteFailure::Promotion)?;
        }
        if self
            .products
            .iter()
            .any(|product| !product.rights_active(at))
        {
            return Err(QuoteError::ProductUnavailable.into());
        }
        self.checked_at = at;
        Ok(())
    }
    pub(crate) fn bom_revisions(&self) -> Vec<(String, i32)> {
        self.products
            .iter()
            .map(|product| (product.product.sku.clone(), product.bom_revision))
            .collect()
    }
}
