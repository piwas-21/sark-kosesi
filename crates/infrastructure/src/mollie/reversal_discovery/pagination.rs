//! Validate provider pagination offsets for the fixed reversal endpoint.
use super::{MollieError, RawValue, ReversalKind, reversal};
use serde::Deserialize;
use url::Url;

#[derive(Deserialize)]
struct Link {
    href: String,
}
pub(super) fn next(
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
