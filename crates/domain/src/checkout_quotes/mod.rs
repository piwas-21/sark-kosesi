//! Immutable indicative quote data. Ownership, current sources and checkout remain use cases.
use crate::{
    Money,
    quotes::{AmountBreakdown, QuoteTotals},
};
use serde::{Deserialize, Serialize};
mod validate;
pub const SNAPSHOT_VERSION: u16 = 1;
pub const MAX_QUOTE_LIFETIME_SECONDS: i64 = 300;
// Timestamp fields cross JSON as exact JavaScript integers.
pub const MAX_QUOTE_UNIX_SECONDS: i64 = 9_007_199_254_740_991;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapshotError {
    InvalidSnapshot,
    InvalidClock,
    Expired,
}
// No Debug: even an indicative destination and its owner are private response data.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuoteBinding {
    pub quote_id: String,
    pub basket_id: String,
    pub basket_revision: i32,
    pub locale: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub country: String,
    pub postcode: String,
    pub policy_id: String,
    pub policy_revision: i32,
    pub policy_review_revision: i32,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapturedAmount {
    pub gross_minor: i64,
    pub net_minor: i64,
    pub vat_minor: i64,
}
impl From<&AmountBreakdown> for CapturedAmount {
    fn from(value: &AmountBreakdown) -> Self {
        Self {
            gross_minor: value.gross.minor_units(),
            net_minor: value.net.minor_units(),
            vat_minor: value.vat.minor_units(),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapturedLine {
    pub sku: String,
    pub publication_revision: i32,
    pub bom_revision: i32,
    pub quantity: u32,
    pub unit_gross_minor: i64,
    pub vat_basis_points: u16,
    pub amounts: CapturedAmount,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuoteSnapshot {
    pub schema_version: u16,
    pub binding: QuoteBinding,
    pub currency: String,
    pub lines: Vec<CapturedLine>,
    pub shipping: CapturedAmount,
    pub shipping_vat_basis_points: u16,
    pub total: CapturedAmount,
}
impl QuoteSnapshot {
    /// Only trusted server evaluation supplies amounts and exact BOM references.
    /// Validation proves internal consistency, not a current price/stock/payment permission.
    pub fn capture(
        binding: QuoteBinding,
        totals: &QuoteTotals,
        boms: &[(String, i32)],
    ) -> Result<Self, SnapshotError> {
        if boms.len() != totals.lines.len() {
            return Err(SnapshotError::InvalidSnapshot);
        }
        let lines = totals
            .lines
            .iter()
            .map(|line| {
                let (_, bom_revision) = boms
                    .iter()
                    .find(|(sku, _)| sku == &line.sku)
                    .ok_or(SnapshotError::InvalidSnapshot)?;
                Ok(CapturedLine {
                    sku: line.sku.clone(),
                    publication_revision: line.publication_revision,
                    bom_revision: *bom_revision,
                    quantity: line.quantity,
                    unit_gross_minor: line.unit_gross.minor_units(),
                    vat_basis_points: line.vat_basis_points,
                    amounts: CapturedAmount::from(&line.amounts),
                })
            })
            .collect::<Result<Vec<_>, SnapshotError>>()?;
        let result = Self {
            schema_version: SNAPSHOT_VERSION,
            binding,
            currency: "EUR".into(),
            lines,
            shipping: CapturedAmount::from(&totals.shipping),
            shipping_vat_basis_points: totals.shipping_vat_basis_points,
            total: CapturedAmount::from(&totals.total),
        };
        result.totals()?;
        Ok(result)
    }
    pub fn check_at(&self, at: i64) -> Result<(), SnapshotError> {
        self.totals()?;
        if at < self.binding.created_at {
            return Err(SnapshotError::InvalidClock);
        }
        if at >= self.binding.expires_at {
            return Err(SnapshotError::Expired);
        }
        Ok(())
    }
}
fn money(value: i64) -> Result<Money, SnapshotError> {
    Money::eur(value).map_err(|_| SnapshotError::InvalidSnapshot)
}
