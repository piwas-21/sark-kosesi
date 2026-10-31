//! Authenticated callback-ID lookup; metadata is only an original-attempt hint.
use super::*;
use crate::payment_attempts::identifier;
use commerce_domain::payment_attempts::ATTEMPT_SCHEMA_VERSION;

/// No Debug/serialization or constructor. Never accepts callback/redirect status,
/// arbitrary URLs, raw JSON or a caller-selected original attempt as evidence.
pub struct CallbackPayment {
    payment: ProviderPayment,
    credential_fingerprint: String,
}
impl CallbackPayment {
    pub fn payment_id(&self) -> &str {
        &self.payment.id
    }
    /// Lookup only: financial matching still requires the frozen original attempt.
    pub fn attempt_id_hint(&self) -> &str {
        &self.payment.attempt_id
    }
    pub(crate) fn bind(&self, original: &AttemptSnapshot) -> Result<VerifiedPayment, MollieError> {
        match_provider(original, &self.payment).map_err(|_| MollieError::PaymentMismatch)?;
        Ok(VerifiedPayment {
            payment: self.payment.clone(),
            checkout_url: None,
            credential_fingerprint: self.credential_fingerprint.clone(),
        })
    }
}
impl VerifiedReconciliationClient {
    /// One bounded GET under the authenticated expected test profile. This never
    /// selects current payment state, creates a payment, or begins a DB transaction.
    pub async fn fetch_callback(&self, id: &str) -> Result<CallbackPayment, MollieError> {
        if !provider_id(id) {
            return Err(MollieError::PaymentMismatch);
        }
        let body = self.inner.http.get(&["payments", id]).await?;
        let (payment, _) = wire::unbound_payment(&body, Some(id))?;
        if payment.resource != "payment"
            || payment.profile_id != self.profile_id()
            || payment.metadata_schema_version != ATTEMPT_SCHEMA_VERSION
            || !identifier(&payment.attempt_id)
            || !identifier(&payment.order_id)
        {
            return Err(MollieError::PaymentMismatch);
        }
        Ok(CallbackPayment {
            payment,
            credential_fingerprint: self.inner.profile.credential_fingerprint.clone(),
        })
    }
}
