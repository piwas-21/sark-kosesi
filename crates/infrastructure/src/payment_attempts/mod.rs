//! Private test-payment custody. No HTTP, provider transport or production activation.
use crate::{
    checkout_orders,
    guest_baskets::{self, GuestCredential, GuestError, GuestSessionStore},
    holds, market_policies, quotes,
};
use commerce_application::{
    Locale,
    payment_requests::{Configuration, FrozenAttempt, RequestError},
};
use commerce_domain::{
    checkout_orders::OrderSnapshot,
    media::valid_command_id,
    payment_attempts::{AttemptBinding, AttemptSnapshot, PreparationIntent},
};
use sqlx::{Postgres, Transaction};
mod dispatch;
mod eligibility;
pub(crate) mod evidence;
pub mod identities;
pub mod observations;
mod prepare;
mod read;
pub mod reconciliation;
pub mod searches;
mod stored;
pub use dispatch::{DispatchOutcome, DispatchPermit, start_dispatch};
pub use prepare::{PreparationReceipt, prepare_attempt};
pub use read::owned_attempt;
/// Nested boundary errors are already static/redacted; raw database errors are discarded.
#[derive(Debug, PartialEq, Eq)]
pub enum PaymentStoreError {
    Boundary(checkout_orders::OrderStoreError),
    InvalidRequest,
    Conflict,
    NotFound,
    Unavailable,
    InvalidStoredData,
    ClockUnavailable,
    Database,
}
impl From<GuestError> for PaymentStoreError {
    fn from(value: GuestError) -> Self {
        Self::Boundary(value.into())
    }
}
impl From<checkout_orders::OrderStoreError> for PaymentStoreError {
    fn from(value: checkout_orders::OrderStoreError) -> Self {
        Self::Boundary(value)
    }
}
impl From<sqlx::Error> for PaymentStoreError {
    fn from(_: sqlx::Error) -> Self {
        Self::Database
    }
}
impl From<quotes::QuoteFailure> for PaymentStoreError {
    fn from(value: quotes::QuoteFailure) -> Self {
        Self::Boundary(value.into())
    }
}
impl From<market_policies::MarketError> for PaymentStoreError {
    fn from(value: market_policies::MarketError) -> Self {
        Self::Boundary(value.into())
    }
}
impl From<holds::HoldWriteError> for PaymentStoreError {
    fn from(value: holds::HoldWriteError) -> Self {
        Self::Boundary(value.into())
    }
}
impl From<RequestError> for PaymentStoreError {
    fn from(value: RequestError) -> Self {
        match value {
            RequestError::InvalidConfiguration => Self::Unavailable,
            _ => Self::InvalidStoredData,
        }
    }
}
fn identifier(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|v| v.is_ascii_digit() || (b'a'..=b'f').contains(&v))
}
