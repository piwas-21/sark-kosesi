use super::*;
use serde::Deserialize;
use serde_json::{Value, value::RawValue};
use std::collections::HashSet;
use url::Url;
const PAGE_LIMIT: usize = 10;
/// Opaque bounded provider offset, never a caller-selected URL or payment authority.
pub struct SearchCursor {
    from: String,
}
impl SearchCursor {
    pub fn capture(from: &str) -> Result<Self, MollieError> {
        if !provider_id(from) {
            return Err(MollieError::InvalidResponse);
        }
        Ok(Self { from: from.into() })
    }
    pub fn provider_id(&self) -> &str {
        &self.from
    }
}
/// A list page is only a set of possible identities for separate authenticated GETs.
/// No next cursor means only that this response supplied no next link, never absence.
pub struct SearchPage {
    candidates: Vec<CreationHint>,
    next: Option<SearchCursor>,
}
impl SearchPage {
    pub fn candidates(&self) -> &[CreationHint] {
        &self.candidates
    }
    pub fn next_cursor(&self) -> Option<&SearchCursor> {
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
    #[serde(borrow)]
    payments: Vec<&'a RawValue>,
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
struct Selector<'a> {
    resource: String,
    id: String,
    #[serde(borrow)]
    metadata: Option<&'a RawValue>,
}
#[derive(Deserialize)]
struct MetadataProbe {
    #[serde(rename = "schema_version")]
    _schema_version: Option<Value>,
    order_id: Option<Value>,
    attempt_id: Option<Value>,
}
pub(super) fn decode(
    body: &[u8],
    attempt: &AttemptSnapshot,
    from: Option<&SearchCursor>,
) -> Result<SearchPage, MollieError> {
    wire::json(body)?;
    let page: Page<'_> = serde_json::from_slice(body).map_err(|_| MollieError::InvalidResponse)?;
    if page.count != page.embedded.payments.len() || page.count > PAGE_LIMIT {
        return Err(MollieError::InvalidResponse);
    }
    let next = next(page.links.next)?;
    if from.is_some_and(|from| next.as_ref().is_some_and(|n| n.from == from.from)) {
        return Err(MollieError::InvalidResponse);
    }
    let mut ids = HashSet::new();
    let mut candidates = Vec::new();
    for raw in page.embedded.payments {
        let selector: Selector<'_> =
            serde_json::from_str(raw.get()).map_err(|_| MollieError::InvalidResponse)?;
        if selector.resource != "payment" || !provider_id(&selector.id) || !ids.insert(selector.id)
        {
            return Err(MollieError::InvalidResponse);
        }
        if associated(selector.metadata, attempt)? {
            // Decode original bytes: duplicate critical fields must never be deduplicated.
            let (payment, _) = wire::payment(raw.get().as_bytes(), attempt, None)?;
            candidates.push(CreationHint {
                provider_id: payment.id,
            });
        }
    }
    Ok(SearchPage { candidates, next })
}
fn associated(metadata: Option<&RawValue>, attempt: &AttemptSnapshot) -> Result<bool, MollieError> {
    let Some(raw) = metadata else {
        return Ok(false);
    };
    let value: Value = serde_json::from_str(raw.get()).map_err(|_| MollieError::InvalidResponse)?;
    if !value.is_object() {
        return Ok(false);
    }
    // Unrelated profile payments may have arbitrary metadata. Known identifiers still
    // reject duplicates, and either matching identifier requires full exact validation.
    let probe: MetadataProbe =
        serde_json::from_str(raw.get()).map_err(|_| MollieError::InvalidResponse)?;
    Ok(
        probe.order_id.as_ref().and_then(Value::as_str) == Some(&attempt.binding.order_id)
            || probe.attempt_id.as_ref().and_then(Value::as_str) == Some(&attempt.binding.id),
    )
}
fn next(raw: &RawValue) -> Result<Option<SearchCursor>, MollieError> {
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
        || url.path() != "/v2/payments"
    {
        return Err(MollieError::InvalidResponse);
    }
    let mut from = None;
    let mut limit = false;
    let mut sort = false;
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "from" if from.is_none() => from = Some(SearchCursor::capture(&value)?),
            "limit" if !limit && value == "10" => limit = true,
            "sort" if !sort && value == "desc" => sort = true,
            _ => return Err(MollieError::InvalidResponse),
        }
    }
    if !limit || from.is_none() {
        return Err(MollieError::InvalidResponse);
    }
    Ok(from)
}
