//! Durable owned indicative prices. Checkout must freshly revalidate before reserving/charging.
use crate::{
    guest_baskets::{self, BasketScope, GuestCredential, GuestError, GuestSessionStore},
    market_policies::{self, MarketError},
    promotion_policies::{self, PolicyError},
    quotes,
};
use commerce_application::Locale;
use commerce_domain::{
    checkout_quotes::{MAX_QUOTE_LIFETIME_SECONDS, QuoteBinding, QuoteSnapshot, SnapshotError},
    media::valid_command_id,
    quotes::Destination,
};
use serde::{Deserialize, Serialize};
use sqlx::{Postgres, Transaction};
mod decode;
pub(crate) use decode::decode_snapshot;
pub(crate) use stored::by_id as snapshot_in_transaction;
mod issue;
mod promoted;
mod stored;
pub use issue::{issue_quote, owned_quote};
pub use promoted::{PromotedQuoteReceipt, issue_promoted_quote, owned_promoted_quote};
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuestQuoteRequest {
    pub expected_basket_revision: i32,
    pub effect_key: String,
    pub destination: Destination,
}
impl GuestQuoteRequest {
    fn validate(&self) -> Result<(), QuoteStoreError> {
        if self.expected_basket_revision <= 0 || !valid_command_id(&self.effect_key) {
            return Err(QuoteStoreError::InvalidRequest);
        }
        self.destination
            .validate()
            .map_err(|_| QuoteStoreError::InvalidRequest)
    }
}
#[derive(Debug)]
pub enum QuoteStoreError {
    Guest(GuestError),
    Market(MarketError),
    Promotion(PolicyError),
    Evaluation(quotes::QuoteFailure),
    InvalidRequest,
    InvalidStoredData,
    NotFound,
    Conflict,
    Expired,
    ClockUnavailable,
    RateLimited,
    Database(sqlx::Error),
}
impl From<GuestError> for QuoteStoreError {
    fn from(value: GuestError) -> Self {
        Self::Guest(value)
    }
}
impl From<MarketError> for QuoteStoreError {
    fn from(value: MarketError) -> Self {
        Self::Market(value)
    }
}
impl From<PolicyError> for QuoteStoreError {
    fn from(value: PolicyError) -> Self {
        match value {
            PolicyError::InvalidStoredData => Self::InvalidStoredData,
            other => Self::Promotion(other),
        }
    }
}
impl From<quotes::QuoteFailure> for QuoteStoreError {
    fn from(value: quotes::QuoteFailure) -> Self {
        Self::Evaluation(value)
    }
}
impl From<sqlx::Error> for QuoteStoreError {
    fn from(value: sqlx::Error) -> Self {
        Self::Database(value)
    }
}
impl From<SnapshotError> for QuoteStoreError {
    fn from(value: SnapshotError) -> Self {
        match value {
            SnapshotError::Expired => Self::Expired,
            SnapshotError::InvalidClock => Self::ClockUnavailable,
            SnapshotError::InvalidSnapshot => Self::InvalidStoredData,
        }
    }
}
pub struct QuoteReceipt {
    pub snapshot: QuoteSnapshot,
    pub replayed: bool,
}
fn identifier(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
