//! Closed classic callback form input; incoming facts never establish payment state.
use axum::http::{
    HeaderMap,
    header::{CONTENT_ENCODING, CONTENT_TYPE},
};
use commerce_domain::payment_attempts::callback_inbox::valid_provider_id;

pub(super) const MAX_BODY: usize = 1024;

#[derive(Debug, PartialEq, Eq)]
pub(super) enum FormError {
    InvalidBody,
    UnsupportedMediaType,
    TooLarge,
}

pub(super) fn parse(headers: &HeaderMap, body: &[u8]) -> Result<String, FormError> {
    if body.len() > MAX_BODY {
        return Err(FormError::TooLarge);
    }
    media_type(headers)?;
    // Exactly one field also excludes duplicate encoded names and empty segments.
    if body.contains(&b'&') {
        return Err(FormError::InvalidBody);
    }
    let separator = body
        .iter()
        .position(|byte| *byte == b'=')
        .ok_or(FormError::InvalidBody)?;
    if decode(&body[..separator])? != "id" {
        return Err(FormError::InvalidBody);
    }
    let value = decode(&body[separator + 1..])?;
    if !valid_provider_id(&value) {
        return Err(FormError::InvalidBody);
    }
    Ok(value)
}

fn media_type(headers: &HeaderMap) -> Result<(), FormError> {
    if headers.contains_key(CONTENT_ENCODING) {
        return Err(FormError::UnsupportedMediaType);
    }
    let mut values = headers.get_all(CONTENT_TYPE).iter();
    let value = values.next().ok_or(FormError::UnsupportedMediaType)?;
    if values.next().is_some() || value.as_bytes().len() > 128 {
        return Err(FormError::UnsupportedMediaType);
    }
    let value = value
        .to_str()
        .map_err(|_| FormError::UnsupportedMediaType)?;
    let (media, charset) = value
        .split_once(';')
        .map_or((value, None), |(media, charset)| (media, Some(charset)));
    if !ows(media).eq_ignore_ascii_case("application/x-www-form-urlencoded") {
        return Err(FormError::UnsupportedMediaType);
    }
    let Some(charset) = charset else {
        return Ok(());
    };
    let (name, value) = charset
        .split_once('=')
        .ok_or(FormError::UnsupportedMediaType)?;
    if !ows(name).eq_ignore_ascii_case("charset")
        || !(ows(value).eq_ignore_ascii_case("utf-8")
            || ows(value).eq_ignore_ascii_case("\"utf-8\""))
    {
        return Err(FormError::UnsupportedMediaType);
    }
    Ok(())
}

fn ows(value: &str) -> &str {
    value.trim_matches([' ', '\t'])
}

fn decode(raw: &[u8]) -> Result<String, FormError> {
    let mut decoded = Vec::with_capacity(raw.len());
    let mut bytes = raw.iter().copied();
    while let Some(byte) = bytes.next() {
        decoded.push(match byte {
            b'+' => b' ',
            b'%' => {
                let high = bytes.next().and_then(hex).ok_or(FormError::InvalidBody)?;
                let low = bytes.next().and_then(hex).ok_or(FormError::InvalidBody)?;
                high * 16 + low
            }
            byte => byte,
        });
    }
    String::from_utf8(decoded).map_err(|_| FormError::InvalidBody)
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
#[path = "../../tests/support/callback_form.rs"]
mod tests;
