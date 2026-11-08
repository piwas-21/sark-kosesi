//! Trusted private guest order composition. No HTTP/provider/dispatch or production activation.
use crate::{
    guest_baskets::{self, BasketScope, GuestCredential, GuestError, GuestSessionStore},
    holds, market_policies, quotes,
};
use commerce_application::Locale;
use commerce_domain::{
    checkout_orders::*,
    checkout_quotes::{QuoteBinding, QuoteSnapshot},
    media::valid_command_id,
};
use sqlx::{Postgres, Transaction};
mod create;
mod read;
mod redemption;
mod stored;
pub(crate) use create::products as product_snapshots;
pub use read::{owned_order, owned_verified_promoted_order};
pub(crate) use stored::{
    by_id as snapshot_in_transaction, guest_by_id as guest_snapshot_in_transaction,
};

/// Seller policy is trusted server configuration; no caller/customer total is accepted.
/// This private primitive grants no provider, public-route or fulfilment permission.
pub async fn create_order(
    store: &GuestSessionStore,
    key: &GuestCredential,
    csrf: &str,
    effect: &str,
    intent: &OrderIntent,
    seller: &SellerPolicy,
) -> Result<OrderReceipt, OrderStoreError> {
    create::create(store, key, csrf, None, effect, intent, seller).await
}

/// Accept an account-bound promoted quote while retaining both authorities.
pub async fn create_verified_promoted_order(
    store: &GuestSessionStore,
    key: &GuestCredential,
    csrf: &str,
    customer: &crate::customer_sessions::CustomerSessionToken,
    effect: &str,
    intent: &OrderIntent,
    seller: &SellerPolicy,
) -> Result<OrderReceipt, OrderStoreError> {
    create::create(store, key, csrf, Some(customer), effect, intent, seller).await
}

/// Failure classes contain no database detail or private request/snapshot material.
#[derive(Debug, PartialEq, Eq)]
pub enum OrderStoreError {
    InvalidRequest,
    Denied,
    Csrf,
    Conflict,
    RateLimited,
    NotFound,
    QuoteNotFound,
    QuoteExpired,
    QuoteChanged,
    Unavailable,
    InvalidStoredData,
    ClockUnavailable,
    EntropyUnavailable,
    Database,
}
impl From<sqlx::Error> for OrderStoreError {
    fn from(_: sqlx::Error) -> Self {
        Self::Database
    }
}
impl From<GuestError> for OrderStoreError {
    fn from(e: GuestError) -> Self {
        match e {
            GuestError::InvalidRequest => Self::InvalidRequest,
            GuestError::Denied => Self::Denied,
            GuestError::Csrf => Self::Csrf,
            GuestError::Conflict => Self::Conflict,
            GuestError::RateLimited => Self::RateLimited,
            GuestError::ClockUnavailable => Self::ClockUnavailable,
            GuestError::EntropyUnavailable => Self::EntropyUnavailable,
            GuestError::InvalidStoredData => Self::InvalidStoredData,
            _ => Self::Database,
        }
    }
}
impl From<crate::checkout_quotes::QuoteStoreError> for OrderStoreError {
    fn from(e: crate::checkout_quotes::QuoteStoreError) -> Self {
        use crate::checkout_quotes::QuoteStoreError as E;
        match e {
            E::NotFound => Self::QuoteNotFound,
            E::Expired => Self::QuoteExpired,
            E::ClockUnavailable => Self::ClockUnavailable,
            E::Guest(e) => e.into(),
            E::Database(_) => Self::Database,
            _ => Self::InvalidStoredData,
        }
    }
}
impl From<commerce_domain::checkout_quotes::SnapshotError> for OrderStoreError {
    fn from(e: commerce_domain::checkout_quotes::SnapshotError) -> Self {
        use commerce_domain::checkout_quotes::SnapshotError as E;
        match e {
            E::Expired => Self::QuoteExpired,
            E::InvalidClock => Self::ClockUnavailable,
            E::InvalidSnapshot => Self::InvalidStoredData,
        }
    }
}
impl From<quotes::QuoteFailure> for OrderStoreError {
    fn from(e: quotes::QuoteFailure) -> Self {
        match e {
            quotes::QuoteFailure::ClockUnavailable => Self::ClockUnavailable,
            quotes::QuoteFailure::Database(_) => Self::Database,
            quotes::QuoteFailure::InvalidStoredData => Self::InvalidStoredData,
            _ => Self::Unavailable,
        }
    }
}
impl From<market_policies::MarketError> for OrderStoreError {
    fn from(e: market_policies::MarketError) -> Self {
        match e {
            market_policies::MarketError::Database(_) => Self::Database,
            market_policies::MarketError::InvalidStoredData => Self::InvalidStoredData,
            _ => Self::Unavailable,
        }
    }
}
impl From<holds::HoldWriteError> for OrderStoreError {
    fn from(e: holds::HoldWriteError) -> Self {
        match e {
            holds::HoldWriteError::ClockUnavailable => Self::ClockUnavailable,
            holds::HoldWriteError::Database(_) => Self::Database,
            holds::HoldWriteError::IdempotencyConflict => Self::Conflict,
            _ => Self::Unavailable,
        }
    }
}
pub struct OrderReceipt {
    pub snapshot: OrderSnapshot,
    pub replayed: bool,
}
fn identifier(s: &str) -> bool {
    s.len() == 32
        && s.bytes()
            .all(|v| v.is_ascii_digit() || (b'a'..=b'f').contains(&v))
}

fn check_session(
    session: Option<&crate::customer_sessions::HeldSession>,
    at: i64,
) -> Result<(), OrderStoreError> {
    if let Some(session) = session {
        session.check_at(at).map_err(|_| OrderStoreError::Denied)?;
    }
    Ok(())
}

fn check_promoted(
    p: &commerce_domain::checkout_quotes::PromotedQuoteSnapshot,
    policy: &commerce_domain::promotion_policies::PromotionPolicy,
    account: Option<i64>,
    at: i64,
) -> Result<(), OrderStoreError> {
    if account.is_some() {
        p.check_at_verified(policy, p.promotion.review_revision, at)?;
    } else {
        p.check_at(policy, p.promotion.review_revision, at)?;
    }
    Ok(())
}
