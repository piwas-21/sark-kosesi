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
#[derive(Clone, PartialEq, Eq)]
pub struct OrderSnapshot {
    pub schema_version: u16,
    pub identity: OrderIdentity,
    pub quote: crate::checkout_quotes::QuoteSnapshot,
    pub promoted_quote: Option<crate::checkout_quotes::PromotedQuoteSnapshot>,
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

use crate::checkout_quotes::{PromotedQuoteSnapshot, QuoteSnapshot};
use serde::{Deserializer, Serializer};

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum WireQuote {
    Original(QuoteSnapshot),
    Promoted(PromotedQuoteSnapshot),
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireOrder {
    schema_version: u16,
    identity: OrderIdentity,
    quote: WireQuote,
    buyer: BuyerContact,
    shipping_address: Address,
    billing_address: Address,
    seller: SellerPolicy,
    market: crate::quotes::MarketPolicy,
    products: Vec<ProductSnapshot>,
}
impl Serialize for OrderSnapshot {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let quote = match &self.promoted_quote {
            Some(p) => WireQuote::Promoted(p.clone()),
            None => WireQuote::Original(self.quote.clone()),
        };
        WireOrder {
            schema_version: self.schema_version,
            identity: self.identity.clone(),
            quote,
            buyer: self.buyer.clone(),
            shipping_address: self.shipping_address.clone(),
            billing_address: self.billing_address.clone(),
            seller: self.seller.clone(),
            market: self.market.clone(),
            products: self.products.clone(),
        }
        .serialize(serializer)
    }
}
impl<'de> Deserialize<'de> for OrderSnapshot {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = WireOrder::deserialize(deserializer)?;
        let (quote, promoted_quote) = match wire.quote {
            WireQuote::Original(q) => (q, None),
            WireQuote::Promoted(p) => (p.original.clone(), Some(p)),
        };
        Ok(Self {
            schema_version: wire.schema_version,
            identity: wire.identity,
            quote,
            promoted_quote,
            buyer: wire.buyer,
            shipping_address: wire.shipping_address,
            billing_address: wire.billing_address,
            seller: wire.seller,
            market: wire.market,
            products: wire.products,
        })
    }
}
