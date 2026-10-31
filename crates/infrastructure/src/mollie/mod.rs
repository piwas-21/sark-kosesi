//! Explicit test-key transport. No environment activation, worker or payment transition.
use crate::payment_attempts::{DispatchPermit, PaymentStoreError};
use commerce_application::{payment_requests::Configuration, request_context::Clock};
use commerce_domain::payment_attempts::{
    AttemptSnapshot, ProfileBinding, ProviderPayment, match_provider, searches::SearchIntent,
};
mod client;
mod credential;
mod http;
mod reconciliation;
mod search;
pub use reconciliation::{ReconciliationClient, VerifiedReconciliationClient};
mod wire;
pub use client::{TestClient, VerifiedTestClient};
pub use credential::TestCredential;
pub use search::{SearchCursor, SearchPage};
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
/// Opaque authenticated list evidence; candidate IDs still require separate GETs.
pub struct VerifiedSearchPage {
    page: SearchPage,
    attempt: AttemptSnapshot,
    from: Option<String>,
    credential_fingerprint: String,
}
impl VerifiedSearchPage {
    fn capture(
        page: SearchPage,
        attempt: &AttemptSnapshot,
        from: Option<&SearchCursor>,
        fingerprint: &str,
    ) -> Self {
        Self {
            page,
            attempt: attempt.clone(),
            from: from.map(|v| v.provider_id().into()),
            credential_fingerprint: fingerprint.into(),
        }
    }
    pub fn candidates(&self) -> &[CreationHint] {
        self.page.candidates()
    }
    pub fn next_cursor(&self) -> Option<&SearchCursor> {
        self.page.next_cursor()
    }
    pub(crate) fn attempt(&self) -> &AttemptSnapshot {
        &self.attempt
    }
    pub(crate) fn intent(&self, previous_page_id: Option<&str>) -> SearchIntent {
        SearchIntent {
            previous_page_id: previous_page_id.map(str::to_owned),
            from: self.from.clone(),
            next: self.next_cursor().map(|v| v.provider_id().into()),
            candidates: self
                .candidates()
                .iter()
                .map(|v| v.provider_id().into())
                .collect(),
            credential_fingerprint: self.credential_fingerprint.clone(),
        }
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
