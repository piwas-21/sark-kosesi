use super::*;
use std::time::Duration;
use url::Url;
/// Unverified client has no payment operation. Profile verification performs no DB work.
pub struct TestClient {
    http: http::Http,
    configuration: Configuration,
}
/// Only the API-key's authenticated expected profile grants transport access.
pub struct VerifiedTestClient {
    inner: TestClient,
}
impl TestClient {
    pub fn new(
        credential: TestCredential,
        configuration: Configuration,
    ) -> Result<Self, MollieError> {
        let base = Url::parse("https://api.mollie.com/v2/")
            .map_err(|_| MollieError::InvalidConfiguration)?;
        Self::build(credential, configuration, base, Duration::from_secs(10))
    }
    fn build(
        credential: TestCredential,
        configuration: Configuration,
        base: Url,
        total: Duration,
    ) -> Result<Self, MollieError> {
        if configuration.snapshot().profile != credential.binding {
            return Err(MollieError::InvalidConfiguration);
        }
        Ok(Self {
            http: http::Http::build(base, credential.authorization, total)?,
            configuration,
        })
    }
    pub fn configuration(&self) -> &Configuration {
        &self.configuration
    }
    pub async fn verify_profile(self) -> Result<VerifiedTestClient, MollieError> {
        let body = self.http.get(&["profiles", "me"]).await?;
        wire::profile(&body, &self.configuration.snapshot().profile.id)?;
        Ok(VerifiedTestClient { inner: self })
    }
    #[cfg(test)]
    pub(super) fn local(
        credential: TestCredential,
        configuration: Configuration,
        base: Url,
        total: Duration,
    ) -> Result<Self, MollieError> {
        if base.scheme() != "http"
            || base.host_str() != Some("127.0.0.1")
            || base.path() != "/v2/"
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
        {
            return Err(MollieError::InvalidConfiguration);
        }
        Self::build(credential, configuration, base, total)
    }
}
impl VerifiedTestClient {
    pub fn configuration(&self) -> &Configuration {
        &self.inner.configuration
    }
    /// One bounded list page supplies identity hints only. Every candidate needs GET.
    /// An empty/end page never proves absence or permits a replacement charge.
    pub async fn search_page(
        &self,
        attempt: &AttemptSnapshot,
        from: Option<&SearchCursor>,
    ) -> Result<VerifiedSearchPage, MollieError> {
        self.check_profile(attempt)?;
        let body = self
            .inner
            .http
            .payment_page(from.map(SearchCursor::provider_id))
            .await?;
        let page = search::decode(&body, attempt, from)?;
        Ok(VerifiedSearchPage::capture(
            page,
            attempt,
            from,
            &self
                .inner
                .configuration
                .snapshot()
                .profile
                .credential_fingerprint,
        ))
    }
    fn check_profile(&self, attempt: &AttemptSnapshot) -> Result<(), MollieError> {
        attempt
            .validate()
            .map_err(|_| MollieError::PaymentMismatch)?;
        if attempt.binding.profile.id != self.inner.configuration.snapshot().profile.id {
            return Err(MollieError::PaymentMismatch);
        }
        Ok(())
    }
    /// Consumes the sole durable permit before one non-retrying POST, outside every DB TX.
    /// Any unusable create reply remains uncertain; it never grants another initial POST.
    pub async fn create(
        &self,
        permit: DispatchPermit,
        clock: &dyn Clock,
    ) -> Result<CreationHint, MollieError> {
        let frozen = permit
            .into_request(clock, &self.inner.configuration)
            .map_err(MollieError::Custody)?;
        let body = self
            .inner
            .http
            .create(&frozen.snapshot().idempotency_key(), frozen.body())
            .await
            .map_err(|_| MollieError::CreateUncertain)?;
        let (payment, _) = wire::payment(&body, frozen.snapshot(), None)
            .map_err(|_| MollieError::CreateUncertain)?;
        Ok(CreationHint {
            provider_id: payment.id,
        })
    }
    /// GET can use a rotated TEST key independently verified against the SAME profile.
    /// No attempt/hold deadline: real late-paid evidence remains reconcilable.
    pub async fn fetch(
        &self,
        attempt: &AttemptSnapshot,
        id: &str,
    ) -> Result<VerifiedPayment, MollieError> {
        self.check_profile(attempt)?;
        if !provider_id(id) {
            return Err(MollieError::PaymentMismatch);
        }
        let body = self.inner.http.get(&["payments", id]).await?;
        let (payment, checkout_url) = wire::payment(&body, attempt, Some(id))?;
        Ok(VerifiedPayment {
            payment,
            checkout_url,
            credential_fingerprint: self
                .inner
                .configuration
                .snapshot()
                .profile
                .credential_fingerprint
                .clone(),
        })
    }
}
