//! Explicit test-key transport. No environment activation, worker or payment transition.
use crate::payment_attempts::{DispatchPermit, PaymentStoreError};
use commerce_application::{payment_requests::Configuration, request_context::Clock};
use commerce_domain::payment_attempts::{
    AttemptSnapshot, ProfileBinding, ProviderPayment, match_provider,
};
mod client;
mod credential;
mod http;
mod wire;
pub use client::{TestClient, VerifiedTestClient};
pub use credential::TestCredential;
#[derive(Debug, PartialEq, Eq)]
pub enum MollieError {
    InvalidCredential,
    InvalidConfiguration,
    Unavailable,
    InvalidResponse,
    ResponseTooLarge,
    ProfileMismatch,
    PaymentMismatch,
    CreateUncertain,
    Custody(PaymentStoreError),
}
/// A create reply is only an identity hint. Fetch-and-verify is still required.
pub struct CreationHint {
    provider_id: String,
}
impl CreationHint {
    pub fn provider_id(&self) -> &str {
        &self.provider_id
    }
}
/// Minimal projection from an authenticated, exact-profile/amount/metadata GET.
/// Retains late-paid evidence; grants no order, stock or fulfilment effect.
pub struct VerifiedPayment {
    payment: ProviderPayment,
    checkout_url: Option<String>,
    credential_fingerprint: String,
}
impl VerifiedPayment {
    pub fn payment(&self) -> &ProviderPayment {
        &self.payment
    }
    pub fn checkout_url(&self) -> Option<&str> {
        self.checkout_url.as_deref()
    }
    pub fn credential_fingerprint(&self) -> &str {
        &self.credential_fingerprint
    }
}
fn provider_id(value: &str) -> bool {
    value.starts_with("tr_")
        && value.len() > 3
        && value.len() <= 256
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

#[cfg(test)]
#[path = "../../tests/support/mollie_transport.rs"]
mod tests;
