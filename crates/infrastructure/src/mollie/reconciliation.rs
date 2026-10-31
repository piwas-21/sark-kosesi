//! GET-only authenticated test-profile client; no payment-create capability.
use super::*;
use url::Url;
pub struct ReconciliationClient {
    pub(super) http: http::Http,
    pub(super) profile: ProfileBinding,
}
pub struct VerifiedReconciliationClient {
    pub(super) inner: ReconciliationClient,
}
impl ReconciliationClient {
    pub fn new(credential: TestCredential) -> Result<Self, MollieError> {
        let base = Url::parse("https://api.mollie.com/v2/")
            .map_err(|_| MollieError::InvalidConfiguration)?;
        Ok(Self {
            http: http::Http::build(
                base,
                credential.authorization,
                std::time::Duration::from_secs(10),
            )?,
            profile: credential.binding,
        })
    }
    pub async fn verify_profile(self) -> Result<VerifiedReconciliationClient, MollieError> {
        let body = self.http.get(&["profiles", "me"]).await?;
        wire::profile(&body, &self.profile.id)?;
        Ok(VerifiedReconciliationClient { inner: self })
    }
}
impl VerifiedReconciliationClient {
    pub fn profile_id(&self) -> &str {
        &self.inner.profile.id
    }
    pub async fn search_page(
        &self,
        attempt: &AttemptSnapshot,
        from: Option<&SearchCursor>,
    ) -> Result<VerifiedSearchPage, MollieError> {
        search_page(&self.inner.http, &self.inner.profile, attempt, from).await
    }
    pub async fn fetch(
        &self,
        attempt: &AttemptSnapshot,
        id: &str,
    ) -> Result<VerifiedPayment, MollieError> {
        fetch(&self.inner.http, &self.inner.profile, attempt, id).await
    }
}
pub(super) fn check(
    profile: &ProfileBinding,
    attempt: &AttemptSnapshot,
) -> Result<(), MollieError> {
    attempt
        .validate()
        .map_err(|_| MollieError::PaymentMismatch)?;
    if attempt.binding.profile.id != profile.id {
        return Err(MollieError::PaymentMismatch);
    }
    Ok(())
}
pub(super) async fn search_page(
    http: &http::Http,
    profile: &ProfileBinding,
    attempt: &AttemptSnapshot,
    from: Option<&SearchCursor>,
) -> Result<VerifiedSearchPage, MollieError> {
    check(profile, attempt)?;
    let body = http
        .payment_page(from.map(SearchCursor::provider_id))
        .await?;
    let page = search::decode(&body, attempt, from)?;
    Ok(VerifiedSearchPage::capture(
        page,
        attempt,
        from,
        &profile.credential_fingerprint,
    ))
}
pub(super) async fn fetch(
    http: &http::Http,
    profile: &ProfileBinding,
    attempt: &AttemptSnapshot,
    id: &str,
) -> Result<VerifiedPayment, MollieError> {
    check(profile, attempt)?;
    if !provider_id(id) {
        return Err(MollieError::PaymentMismatch);
    }
    let body = http.get(&["payments", id]).await?;
    let (payment, checkout_url) = wire::payment(&body, attempt, Some(id))?;
    Ok(VerifiedPayment {
        payment,
        checkout_url,
        credential_fingerprint: profile.credential_fingerprint.clone(),
    })
}
