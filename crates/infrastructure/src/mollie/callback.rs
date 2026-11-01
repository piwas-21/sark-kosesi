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

/// Authenticated fetched evidence bound to an opaque durable pre-I/O operation.
pub struct CallbackLookupPayment {
    operation: commerce_domain::payment_attempts::callbacks::LookupOperationSnapshot,
    fetched: CallbackPayment,
    started_at: i64,
}
impl CallbackLookupPayment {
    pub(crate) fn operation(
        &self,
    ) -> &commerce_domain::payment_attempts::callbacks::LookupOperationSnapshot {
        &self.operation
    }
    pub(crate) fn snapshot(
        &self,
        id: String,
        recorded_at: i64,
    ) -> commerce_domain::payment_attempts::callbacks::LookupSnapshot {
        commerce_domain::payment_attempts::callbacks::LookupSnapshot {
            schema_version: 1,
            id,
            operation: self.operation.clone(),
            started_at: self.started_at,
            recorded_at,
            payment: self.fetched.payment.clone(),
            credential_fingerprint: self.fetched.credential_fingerprint.clone(),
        }
    }
    pub(crate) fn matches(
        &self,
        snapshot: &commerce_domain::payment_attempts::callbacks::LookupSnapshot,
    ) -> bool {
        self.operation == snapshot.operation
            && self.started_at == snapshot.started_at
            && self.fetched.payment == snapshot.payment
            && self.fetched.credential_fingerprint == snapshot.credential_fingerprint
    }
    pub(crate) fn copy_payment(&self) -> CallbackPayment {
        CallbackPayment {
            payment: self.fetched.payment.clone(),
            credential_fingerprint: self.fetched.credential_fingerprint.clone(),
        }
    }
}
impl VerifiedReconciliationClient {
    pub async fn fetch_lookup(
        &self,
        permit: crate::payment_attempts::callbacks::lookup::LookupFetchPermit,
    ) -> Result<CallbackLookupPayment, MollieError> {
        let (snapshot, started_at) = permit.into_parts();
        if snapshot.profile_id != self.profile_id() {
            return Err(MollieError::ProfileMismatch);
        }
        let fetched = self.fetch_callback(&snapshot.provider_id).await?;
        Ok(CallbackLookupPayment {
            operation: snapshot,
            fetched,
            started_at,
        })
    }
}
impl CallbackPayment {
    /// Called only by the durable custody decoder after exact operation/header/event
    /// checks. A caller's JSON or domain projection cannot access this constructor.
    pub(crate) fn from_custody(
        snapshot: &commerce_domain::payment_attempts::callbacks::LookupSnapshot,
    ) -> Result<Self, MollieError> {
        snapshot
            .validate()
            .map_err(|_| MollieError::PaymentMismatch)?;
        Ok(Self {
            payment: snapshot.payment.clone(),
            credential_fingerprint: snapshot.credential_fingerprint.clone(),
        })
    }
}

#[cfg(test)]
#[path = "../../tests/support/mollie_callback_capabilities.rs"]
mod capability_tests;
