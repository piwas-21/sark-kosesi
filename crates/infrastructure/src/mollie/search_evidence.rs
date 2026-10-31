//! Opaque authenticated list evidence; no verified financial fact or absence proof.
use super::*;
use commerce_domain::payment_attempts::searches::SearchIntent;
pub struct VerifiedSearchPage {
    page: SearchPage,
    attempt: AttemptSnapshot,
    from: Option<String>,
    credential_fingerprint: String,
}
impl VerifiedSearchPage {
    pub(super) fn capture(
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
