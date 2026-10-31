//! Immutable accepted order data, separate from payment and current stock state.
use serde::{Deserialize, Serialize};
mod contact;
mod policy;
mod validate;
pub use contact::{Address, BuyerContact};
pub use policy::{SellerPolicy, SellerTaxRegistration};
pub const ORDER_SNAPSHOT_VERSION: u16 = 1;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrderError {
    InvalidRequest,
    InvalidSnapshot,
    PolicyUnavailable,
}
// Customer/seller addresses and ownership deliberately have no Debug implementation.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrderIntent {
    pub quote_id: String,
    pub expected_basket_revision: i32,
    pub buyer: BuyerContact,
    pub shipping_address: Address,
    pub billing_address: Address,
    pub accepted_terms_version: String,
    pub confirmed: bool,
}
impl OrderIntent {
    pub fn validate(&self) -> Result<(), OrderError> {
        if !hex(&self.quote_id, 32)
            || self.expected_basket_revision <= 0
            || !self.confirmed
            || !crate::media::valid_command_id(&self.accepted_terms_version)
        {
            return Err(OrderError::InvalidRequest);
        }
        self.buyer.validate()?;
        self.shipping_address.validate()?;
        self.billing_address.validate()
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrderIdentity {
    pub id: String,
    pub basket_id: String,
    pub quote_id: String,
    pub stock_group_id: String,
    pub created_at: i64,
    pub hold_expires_at: i64,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProductSnapshot {
    pub sku: String,
    pub publication_revision: i32,
    pub bom_revision: i32,
    pub title: String,
    pub family_code: String,
    pub size_code: String,
    pub textile_code: String,
    pub fill: String,
    pub footprint: crate::specifications::DimensionsMm,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrderSnapshot {
    pub schema_version: u16,
    pub identity: OrderIdentity,
    pub quote: crate::checkout_quotes::QuoteSnapshot,
    pub buyer: BuyerContact,
    pub shipping_address: Address,
    pub billing_address: Address,
    pub seller: SellerPolicy,
    pub market: crate::quotes::MarketPolicy,
    pub products: Vec<ProductSnapshot>,
}
fn text(value: &str, limit: usize) -> bool {
    !value.is_empty()
        && value.trim() == value
        && value.chars().count() <= limit
        && !value.chars().any(char::is_control)
}
fn hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|v| v.is_ascii_digit() || (b'a'..=b'f').contains(&v))
}
