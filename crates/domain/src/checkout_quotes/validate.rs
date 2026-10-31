use super::*;
use crate::{
    catalogue_authoring::merchant_code,
    quotes::{Destination, PricedItem, QuoteRequest, calculate},
};
fn identifier(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
impl QuoteBinding {
    fn valid(&self) -> bool {
        identifier(&self.quote_id)
            && identifier(&self.basket_id)
            && self.basket_revision > 0
            && ["nl", "de", "fr", "en", "ar", "tr", "kmr"].contains(&self.locale.as_str())
            && self.created_at >= 0
            && self.expires_at <= MAX_QUOTE_UNIX_SECONDS
            && self.expires_at > self.created_at
            && self.expires_at - self.created_at <= MAX_QUOTE_LIFETIME_SECONDS
            && merchant_code(&self.policy_id)
            && self.policy_revision > 0
            && self.policy_review_revision > 0
    }
}
impl QuoteSnapshot {
    /// Decode adapters must also enforce the promised JSON object shapes and input size.
    /// Reconstruction rejects forged allocations, negative values and unsafe integer totals.
    pub fn totals(&self) -> Result<QuoteTotals, SnapshotError> {
        if self.schema_version != SNAPSHOT_VERSION
            || self.currency != "EUR"
            || !self.binding.valid()
            || self.lines.iter().any(|line| line.bom_revision <= 0)
        {
            return Err(SnapshotError::InvalidSnapshot);
        }
        let request = QuoteRequest {
            destination: Destination {
                country: self.binding.country.clone(),
                postcode: self.binding.postcode.clone(),
            },
            lines: self
                .lines
                .iter()
                .map(|line| crate::quotes::BasketLine {
                    sku: line.sku.clone(),
                    quantity: line.quantity,
                })
                .collect(),
        };
        request
            .validate()
            .map_err(|_| SnapshotError::InvalidSnapshot)?;
        let prices = self
            .lines
            .iter()
            .map(|line| {
                Ok(PricedItem {
                    sku: line.sku.clone(),
                    publication_revision: line.publication_revision,
                    quantity: line.quantity,
                    unit_gross: money(line.unit_gross_minor)?,
                    vat_basis_points: line.vat_basis_points,
                })
            })
            .collect::<Result<Vec<_>, SnapshotError>>()?;
        let calculated = calculate(
            &prices,
            money(self.shipping.gross_minor)?,
            self.shipping_vat_basis_points,
        )
        .map_err(|_| SnapshotError::InvalidSnapshot)?;
        if CapturedAmount::from(&calculated.shipping) != self.shipping
            || CapturedAmount::from(&calculated.total) != self.total
            || self
                .lines
                .iter()
                .zip(&calculated.lines)
                .any(|(stored, line)| {
                    stored.sku != line.sku || stored.amounts != CapturedAmount::from(&line.amounts)
                })
        {
            return Err(SnapshotError::InvalidSnapshot);
        }
        Ok(calculated)
    }
}
