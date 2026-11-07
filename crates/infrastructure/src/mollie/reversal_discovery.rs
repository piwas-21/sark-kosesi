//! Payment-scoped candidate identities only; exact observations require another GET.
use super::*;
use commerce_domain::payment_attempts::{PaymentMode, PaymentStatus};
use serde::Deserialize;
use serde_json::{Value, value::RawValue};
use std::collections::HashSet;
mod pagination;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReversalKind {
    Refund,
    Chargeback,
}
impl ReversalKind {
    fn collection(self) -> &'static str {
        match self {
            Self::Refund => "refunds",
            Self::Chargeback => "chargebacks",
        }
    }
    fn resource(self) -> &'static str {
        match self {
            Self::Refund => "refund",
            Self::Chargeback => "chargeback",
        }
    }
    fn prefix(self) -> &'static str {
        match self {
            Self::Refund => "re_",
            Self::Chargeback => "chb_",
        }
    }
}
/// In-memory continuation bound to the full frozen attempt and authenticated key.
/// There is deliberately no public constructor or serialization.
pub struct ReversalCursor {
    from: String,
    attempt: AttemptSnapshot,
    payment_id: String,
    kind: ReversalKind,
    credential_fingerprint: String,
}
impl ReversalCursor {
    pub(crate) fn from_storage(
        proof: crate::payment_attempts::reversal_pages::CursorProof,
    ) -> Self {
        let receipt = proof.receipt();
        // The proof is produced only after the store checks the nonterminal tip.
        Self {
            from: proof.from().into(),
            attempt: proof.attempt().clone(),
            payment_id: receipt.payment_id.clone(),
            kind: receipt.kind,
            credential_fingerprint: receipt.credential_fingerprint.clone(),
        }
    }
}
/// Candidates are hints, and neither an end link nor an empty page proves absence.
pub struct ReversalPage {
    attempt: AttemptSnapshot,
    payment_id: String,
    kind: ReversalKind,
    requested_from: Option<String>,
    credential_fingerprint: String,
    candidates: Vec<String>,
    next: Option<ReversalCursor>,
}
impl ReversalPage {
    pub fn attempt(&self) -> &AttemptSnapshot {
        &self.attempt
    }
    pub fn payment_id(&self) -> &str {
        &self.payment_id
    }
    pub fn kind(&self) -> ReversalKind {
        self.kind
    }
    pub fn requested_from(&self) -> Option<&str> {
        self.requested_from.as_deref()
    }
    pub fn credential_fingerprint(&self) -> &str {
        &self.credential_fingerprint
    }
    pub fn candidates(&self) -> &[String] {
        &self.candidates
    }
    pub fn next_cursor(&self) -> Option<&ReversalCursor> {
        self.next.as_ref()
    }
    pub fn next_from(&self) -> Option<&str> {
        self.next.as_ref().map(|cursor| cursor.from.as_str())
    }
}
#[derive(Deserialize)]
struct Page<'a> {
    count: usize,
    #[serde(rename = "_embedded", borrow)]
    embedded: Embedded<'a>,
    #[serde(rename = "_links", borrow)]
    links: Links<'a>,
}
#[derive(Deserialize)]
struct Embedded<'a> {
    #[serde(default, borrow)]
    refunds: Option<Vec<&'a RawValue>>,
    #[serde(default, borrow)]
    chargebacks: Option<Vec<&'a RawValue>>,
}
#[derive(Deserialize)]
struct Links<'a> {
    #[serde(borrow)]
    next: &'a RawValue,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Item {
    resource: String,
    id: String,
    payment_id: String,
    #[serde(default, deserialize_with = "reversal::optional_mode")]
    mode: Option<PaymentMode>,
}
impl VerifiedReconciliationClient {
    /// Fetch one test-profile page after independently verifying the paid parent.
    pub async fn reversal_page(
        &self,
        attempt: &AttemptSnapshot,
        payment_id: &str,
        kind: ReversalKind,
        from: Option<&ReversalCursor>,
    ) -> Result<ReversalPage, MollieError> {
        if !provider_id(payment_id)
            || from.is_some_and(|v| {
                v.attempt != *attempt
                    || v.payment_id != payment_id
                    || v.kind != kind
                    || v.credential_fingerprint != self.inner.profile.credential_fingerprint
            })
        {
            return Err(MollieError::PaymentMismatch);
        }
        let parent = self.fetch(attempt, payment_id).await?;
        if !matches!(parent.payment.status, PaymentStatus::Paid) {
            return Err(MollieError::PaymentMismatch);
        }
        let body = self
            .inner
            .http
            .reversal_page(payment_id, kind.collection(), from.map(|v| v.from.as_str()))
            .await?;
        decode(
            &body,
            attempt,
            payment_id,
            kind,
            from,
            &parent.credential_fingerprint,
        )
    }
}
fn decode(
    body: &[u8],
    attempt: &AttemptSnapshot,
    payment_id: &str,
    kind: ReversalKind,
    from: Option<&ReversalCursor>,
    fingerprint: &str,
) -> Result<ReversalPage, MollieError> {
    let value = wire::json(body)?;
    let embedded = value
        .get("_embedded")
        .and_then(Value::as_object)
        .ok_or(MollieError::InvalidResponse)?;
    let entries = embedded
        .get(kind.collection())
        .and_then(Value::as_array)
        .ok_or(MollieError::InvalidResponse)?;
    let next_value = value
        .get("_links")
        .and_then(Value::as_object)
        .and_then(|v| v.get("next"))
        .ok_or(MollieError::InvalidResponse)?;
    if !entries.iter().all(Value::is_object) || !(next_value.is_object() || next_value.is_null()) {
        return Err(MollieError::InvalidResponse);
    }
    let page: Page<'_> = serde_json::from_slice(body).map_err(|_| MollieError::InvalidResponse)?;
    let items = match kind {
        ReversalKind::Refund => page.embedded.refunds,
        ReversalKind::Chargeback => page.embedded.chargebacks,
    }
    .ok_or(MollieError::InvalidResponse)?;
    if page.count != items.len() || page.count > 10 {
        return Err(MollieError::InvalidResponse);
    }
    let offset = pagination::next(page.links.next, payment_id, kind)?;
    if from.is_some_and(|v| offset.as_ref().is_some_and(|n| *n == v.from)) {
        return Err(MollieError::InvalidResponse);
    }
    let mut seen = HashSet::new();
    let mut candidates = Vec::new();
    for raw in items {
        let item: Item =
            serde_json::from_str(raw.get()).map_err(|_| MollieError::InvalidResponse)?;
        if item.resource != kind.resource()
            || item.payment_id != payment_id
            || !reversal::child_id(&item.id, kind.prefix())
            || !seen.insert(item.id.clone())
            || (kind == ReversalKind::Refund && item.mode.is_none())
            || item
                .mode
                .is_some_and(|mode| !matches!(mode, PaymentMode::Test))
        {
            return Err(MollieError::PaymentMismatch);
        }
        candidates.push(item.id);
    }
    Ok(ReversalPage {
        attempt: attempt.clone(),
        payment_id: payment_id.into(),
        kind,
        requested_from: from.map(|cursor| cursor.from.clone()),
        credential_fingerprint: fingerprint.into(),
        candidates,
        next: offset.map(|from| ReversalCursor {
            from,
            attempt: attempt.clone(),
            payment_id: payment_id.into(),
            kind,
            credential_fingerprint: fingerprint.into(),
        }),
    })
}
