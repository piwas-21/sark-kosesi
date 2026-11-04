//! Private guest-order proof custody and account linking; no transport is installed.
use commerce_application::request_context::{Clock, ObservedClock};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Transaction};
use zeroize::Zeroizing;

use super::{GuestCredential, GuestError, GuestSessionStore, access, valid_hex};
use crate::{
    checkout_orders,
    customer_sessions::{self, CustomerSessionToken, SessionError},
};

mod issue;
mod link;
mod read;
pub use issue::issue;
pub use link::link;
pub use read::{
    LinkedOrderCursor, LinkedOrderLine, LinkedOrderPage, LinkedOrderView, list_linked_orders,
    read_linked_order,
};

const DOMAIN: &[u8] = b"piwas:guest-order-link:verifier:v1\0";
const LIFETIME: i64 = 900;

/// Bearer proof; only its domain-separated verifier is stored.
pub struct GuestOrderLinkToken(Zeroizing<[u8; 32]>);
impl GuestOrderLinkToken {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(Zeroizing::new(bytes))
    }
    pub fn expose_bytes(&self) -> &[u8; 32] {
        &self.0
    }
    fn random() -> Result<Self, LinkError> {
        let mut token = Self::from_bytes([0; 32]);
        getrandom::fill(token.0.as_mut()).map_err(|_| LinkError::Unavailable)?;
        Ok(token)
    }
    fn verifier(&self) -> [u8; 32] {
        let mut hash = Sha256::new();
        hash.update(DOMAIN);
        hash.update(self.expose_bytes());
        hash.finalize().into()
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum LinkError {
    Denied,
    Unavailable,
}

pub struct LinkedOrder {
    pub order_id: String,
    pub account_id: i64,
    pub linked_at: i64,
}

impl From<SessionError> for LinkError {
    fn from(error: SessionError) -> Self {
        match error {
            SessionError::Denied => Self::Denied,
            SessionError::Unavailable => Self::Unavailable,
        }
    }
}
impl From<GuestError> for LinkError {
    fn from(error: GuestError) -> Self {
        match error {
            GuestError::Denied | GuestError::Csrf | GuestError::InvalidRequest => Self::Denied,
            _ => Self::Unavailable,
        }
    }
}
fn order_error(error: checkout_orders::OrderStoreError) -> LinkError {
    match error {
        checkout_orders::OrderStoreError::NotFound
        | checkout_orders::OrderStoreError::Denied
        | checkout_orders::OrderStoreError::InvalidRequest => LinkError::Denied,
        _ => LinkError::Unavailable,
    }
}

async fn require_unlinked_order(
    tx: &mut Transaction<'_, Postgres>,
    basket_id: &str,
    order_id: &str,
) -> Result<(), LinkError> {
    let locked: Option<String> = sqlx::query_scalar(
        "SELECT id FROM checkout_orders WHERE id=$1 AND basket_id=$2 FOR UPDATE",
    )
    .bind(order_id)
    .bind(basket_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| LinkError::Unavailable)?;
    if locked.as_deref() != Some(order_id) {
        return Err(LinkError::Denied);
    }
    let linked: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM customer_order_links WHERE order_id=$1)")
            .bind(order_id)
            .fetch_one(&mut **tx)
            .await
            .map_err(|_| LinkError::Unavailable)?;
    if linked {
        return Err(LinkError::Denied);
    }
    Ok(())
}
