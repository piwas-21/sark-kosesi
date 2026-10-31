//! Indicative server quotes, never payment authority or a stock reservation.
use crate::{Money, catalogue_authoring::merchant_code};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
mod policy;
mod totals;
pub use policy::{MarketPolicy, ProductRule, ShippingRule};
pub(crate) use totals::split;
pub use totals::{AmountBreakdown, PricedItem, QuoteLine, QuoteTotals, calculate};

pub const MAX_LINES: usize = 20;
pub const MAX_QUANTITY: u32 = 100;
pub const MAX_MINOR_UNITS: i64 = 9_007_199_254_740_991;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BasketLine {
    pub sku: String,
    pub quantity: u32,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Destination {
    pub country: String,
    pub postcode: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuoteRequest {
    pub lines: Vec<BasketLine>,
    pub destination: Destination,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuoteError {
    InvalidRequest,
    InvalidPolicy,
    MarketUnavailable,
    ProductUnavailable,
    InsufficientStock,
    ShippingUnavailable,
    AmountTooLarge,
}
impl Destination {
    pub fn validate(&self) -> Result<(), QuoteError> {
        if !valid_country(&self.country) || !valid_postcode(&self.postcode) {
            return Err(QuoteError::InvalidRequest);
        }
        Ok(())
    }
}
impl QuoteRequest {
    pub fn validate(&self) -> Result<(), QuoteError> {
        let mut seen = BTreeSet::new();
        self.destination.validate()?;
        if self.lines.is_empty()
            || self.lines.len() > MAX_LINES
            || !self.lines.iter().all(|line| {
                merchant_code(&line.sku)
                    && (1..=MAX_QUANTITY).contains(&line.quantity)
                    && seen.insert(&line.sku)
            })
        {
            return Err(QuoteError::InvalidRequest);
        }
        Ok(())
    }
}
pub(crate) fn valid_country(country: &str) -> bool {
    [
        "AT", "BE", "BG", "HR", "CY", "CZ", "DK", "EE", "FI", "FR", "DE", "GR", "HU", "IE", "IT",
        "LV", "LT", "LU", "MT", "NL", "PL", "PT", "RO", "SK", "SI", "ES", "SE",
    ]
    .contains(&country)
}
fn valid_postcode(postcode: &str) -> bool {
    !postcode.is_empty()
        && postcode.len() <= 16
        && postcode.trim() == postcode
        && postcode
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b' ' || b == b'-')
        && postcode.bytes().any(|b| b.is_ascii_alphanumeric())
}
fn valid_rate(rate: u16) -> bool {
    rate <= 10_000
}
fn bounded_money(money: Money) -> bool {
    money.minor_units() <= MAX_MINOR_UNITS
}
