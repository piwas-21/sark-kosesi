//! Deterministic single-promotion pricing. Quotes never reserve redemption budgets.
use crate::{Money, catalogue_authoring::merchant_code, quotes::*};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
mod allocation;
mod policy;
mod refund;
pub use allocation::apply;
pub use policy::{DiscountKind, EligibleProduct, Promotion};
pub use refund::allocate_refund;
pub const ALLOCATION_VERSION: u16 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromotionError {
    InvalidPolicy,
    InvalidQuote,
    Ineligible,
    AmountTooLarge,
    InvalidRefund,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DiscountedLine {
    pub original: QuoteLine,
    pub discount: Money,
    pub amounts: AmountBreakdown,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PromotionQuote {
    pub allocation_version: u16,
    pub promotion_id: String,
    pub promotion_revision: i32,
    pub discount: Money,
    pub lines: Vec<DiscountedLine>,
    pub shipping: AmountBreakdown,
    pub shipping_vat_basis_points: u16,
    pub total: AmountBreakdown,
}
fn money(minor: i64) -> Result<Money, PromotionError> {
    if !(0..=MAX_MINOR_UNITS).contains(&minor) {
        return Err(PromotionError::AmountTooLarge);
    }
    Money::eur(minor).map_err(|_| PromotionError::AmountTooLarge)
}
fn wide_money(minor: i128) -> Result<Money, PromotionError> {
    money(i64::try_from(minor).map_err(|_| PromotionError::AmountTooLarge)?)
}
fn add(a: Money, b: Money) -> Result<Money, PromotionError> {
    money(
        a.checked_add(b)
            .map_err(|_| PromotionError::AmountTooLarge)?
            .minor_units(),
    )
}
fn subtract(a: Money, b: Money) -> Result<Money, PromotionError> {
    money(
        a.minor_units()
            .checked_sub(b.minor_units())
            .ok_or(PromotionError::AmountTooLarge)?,
    )
}
