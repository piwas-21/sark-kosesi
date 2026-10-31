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
    InvalidRequest,
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
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileBinding {
    pub id: String,
    pub credential_fingerprint: String,
    pub mode: PaymentMode,
}
impl ProfileBinding {
    pub fn validate(&self) -> Result<(), PaymentError> {
        if !opaque(&self.id, "pfl_") || !hex(&self.credential_fingerprint, 64) {
            return Err(PaymentError::InvalidSnapshot);
        }
        Ok(())
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparationIntent {
    pub order_id: String,
    pub confirmed: bool,
}
impl PreparationIntent {
    pub fn validate(&self) -> Result<(), PaymentError> {
        if !hex(&self.order_id, 32) || !self.confirmed {
            return Err(PaymentError::InvalidRequest);
        }
        Ok(())
    }
}
/// Fresh validated server configuration, supplied by the application at dispatch.
pub struct CurrentConfiguration {
    pub profile: ProfileBinding,
    pub sha256: String,
}
// No Debug: identifiers correlate private orders and credential custody.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttemptBinding {
    pub id: String,
    pub order_id: String,
    pub basket_id: String,
    pub stock_group_id: String,
    pub profile: ProfileBinding,
    pub configuration_sha256: String,
    pub currency: Currency,
    pub amount_minor: i64,
    pub created_at: i64,
    pub dispatch_before: i64,
}
impl AttemptBinding {
    pub fn validate(&self) -> Result<(), PaymentError> {
        if ![
            &self.id,
            &self.order_id,
            &self.basket_id,
            &self.stock_group_id,
        ]
        .iter()
        .all(|v| hex(v, 32))
            || self.profile.validate().is_err()
            || !hex(&self.configuration_sha256, 64)
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
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttemptSnapshot {
    pub schema_version: u16,
    pub binding: AttemptBinding,
    pub request_sha256: String,
}
impl AttemptSnapshot {
    pub fn validate(&self) -> Result<(), PaymentError> {
        if self.schema_version != ATTEMPT_SCHEMA_VERSION || !hex(&self.request_sha256, 64) {
            return Err(PaymentError::InvalidSnapshot);
        }
        self.binding.validate()
    }
    /// Stable for the validated frozen attempt; never replace this key on timeout.
    pub fn idempotency_key(&self) -> String {
        format!("piwas-test-payment-{}", self.binding.id)
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
