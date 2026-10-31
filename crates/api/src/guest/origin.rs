use axum::http::HeaderMap;
#[derive(Clone)]
pub struct GuestOrigin(String);
impl GuestOrigin {
    /// Only a canonical HTTPS origin: no path, credentials, query or fragment.
    pub fn parse(value: &str) -> Result<Self, &'static str> {
        let url = url::Url::parse(value).map_err(|_| "GUEST_ORIGIN must be an HTTPS origin")?;
        if url.scheme() != "https" || url.origin().ascii_serialization() != value {
            return Err("GUEST_ORIGIN must be a canonical HTTPS origin");
        }
        Ok(Self(value.into()))
    }
    pub(super) fn accepts(&self, headers: &HeaderMap) -> bool {
        single_header(headers, "origin") == Some(self.0.as_str())
    }
}
pub(super) fn single_header<'a>(headers: &'a HeaderMap, key: &str) -> Option<&'a str> {
    let mut values = headers.get_all(key).iter();
    let first = values.next()?.to_str().ok()?;
    if values.next().is_some() {
        return None;
    }
    Some(first)
}
