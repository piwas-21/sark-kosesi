//! Payment-scoped candidate identities only; exact observations require another GET.
use super::*;
use commerce_domain::payment_attempts::{PaymentMode, PaymentStatus};
use serde::Deserialize;
use serde_json::{Value, value::RawValue};
use std::collections::HashSet;
use url::Url;

#[derive(Clone, Copy, PartialEq, Eq)]
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
/// Candidates are hints, and neither an end link nor an empty page proves absence.
pub struct ReversalPage {
    candidates: Vec<String>,
    next: Option<ReversalCursor>,
}
impl ReversalPage {
    pub fn candidates(&self) -> &[String] {
        &self.candidates
    }
    pub fn next_cursor(&self) -> Option<&ReversalCursor> {
        self.next.as_ref()
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
struct Link {
    href: String,
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
    let offset = next(page.links.next, payment_id, kind)?;
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
fn next(
    raw: &RawValue,
    payment_id: &str,
    kind: ReversalKind,
) -> Result<Option<String>, MollieError> {
    if raw.get() == "null" {
        return Ok(None);
    }
    let link: Link = serde_json::from_str(raw.get()).map_err(|_| MollieError::InvalidResponse)?;
    if link.href.len() > 2048 {
        return Err(MollieError::InvalidResponse);
    }
    let url = Url::parse(&link.href).map_err(|_| MollieError::InvalidResponse)?;
    if url.as_str() != link.href
        || url.scheme() != "https"
        || url.host_str() != Some("api.mollie.com")
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(MollieError::InvalidResponse);
    }
    let parts = url
        .path_segments()
        .ok_or(MollieError::InvalidResponse)?
        .collect::<Vec<_>>();
    if parts.len() != 4
        || decode_segment(parts[0]).as_deref() != Some("v2")
        || decode_segment(parts[1]).as_deref() != Some("payments")
        || decode_segment(parts[2]).as_deref() != Some(payment_id)
        || decode_segment(parts[3]).as_deref() != Some(kind.collection())
    {
        return Err(MollieError::InvalidResponse);
    }
    let mut from = None;
    let mut limit = false;
    let query = url.query().ok_or(MollieError::InvalidResponse)?;
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').ok_or(MollieError::InvalidResponse)?;
        let key = decode_segment(key).ok_or(MollieError::InvalidResponse)?;
        // Form query spaces are allowed inside an opaque ID. Invalid percent escapes
        // and non-UTF-8 bytes cannot silently turn into a different cursor.
        let value = decode_segment(&value.replace('+', " ")).ok_or(MollieError::InvalidResponse)?;
        match key.as_str() {
            "from" if from.is_none() && reversal::child_id(&value, kind.prefix()) => {
                from = Some(value)
            }
            "limit" if !limit && value == "10" => limit = true,
            _ => return Err(MollieError::InvalidResponse),
        }
    }
    from.map(Some).ok_or(MollieError::InvalidResponse)
}
fn decode_segment(segment: &str) -> Option<String> {
    let bytes = segment.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}
