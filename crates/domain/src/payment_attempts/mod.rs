//! Test-payment identity and pure custody policy; no network or payment authority.
use crate::Currency;
use serde::{Deserialize, Serialize};
mod dispatch;
mod provider;
pub use dispatch::{DispatchPlan, DispatchState, bind_provider, plan_dispatch};
pub use provider::{PaymentStatus, ProviderPayment, match_provider};
pub const ATTEMPT_SCHEMA_VERSION: u16 = 1;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaymentError {
    InvalidSnapshot,
    InvalidClock,
    Expired,
    ConfigurationChanged,
    NotDispatched,
    IdentityConflict,
    ProviderMismatch,
}
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PaymentMode {
    Test,
}
// No Debug: identifiers correlate private orders and credential custody.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttemptSnapshot {
    pub schema_version: u16,
    pub id: String,
    pub order_id: String,
    pub basket_id: String,
    pub stock_group_id: String,
    pub profile_id: String,
    pub credential_fingerprint: String,
    pub mode: PaymentMode,
    pub currency: Currency,
    pub amount_minor: i64,
    pub request_sha256: String,
    pub created_at: i64,
    pub dispatch_before: i64,
}
impl AttemptSnapshot {
    pub fn validate(&self) -> Result<(), PaymentError> {
        if self.schema_version != ATTEMPT_SCHEMA_VERSION
            || ![
                &self.id,
                &self.order_id,
                &self.basket_id,
                &self.stock_group_id,
            ]
            .iter()
            .all(|v| hex(v, 32))
            || !opaque(&self.profile_id, "pfl_")
            || !hex(&self.credential_fingerprint, 64)
            || !hex(&self.request_sha256, 64)
            || !(1..=crate::quotes::MAX_MINOR_UNITS).contains(&self.amount_minor)
            || self.created_at < 0
            || self.dispatch_before <= self.created_at
            || self.dispatch_before > crate::checkout_quotes::MAX_QUOTE_UNIX_SECONDS
            || self.dispatch_before - self.created_at > 1800
        {
            return Err(PaymentError::InvalidSnapshot);
        }
        Ok(())
    }
    /// Stable within the stored profile/credential/request identity, never a new key on timeout.
    pub fn idempotency_key(&self) -> String {
        format!("piwas-test-payment-{}", self.id)
    }
}
fn hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|v| v.is_ascii_digit() || (b'a'..=b'f').contains(&v))
}
fn opaque(value: &str, prefix: &str) -> bool {
    value.starts_with(prefix)
        && value.len() > prefix.len()
        && value.len() <= 256
        && value.trim() == value
        && !value.chars().any(char::is_control)
}
