//! Bounded loopback-only callback forwarding; no database or provider access.
use reqwest::{
    Client,
    header::{AUTHORIZATION, CONTENT_TYPE, HeaderValue},
};
use std::time::Duration;
use url::{Host, Url};
pub const CALLBACK_PATH: &str = "/api/callbacks/mollie";
const MAX_BODY: usize = 1024;
const DEADLINE: Duration = Duration::from_millis(1500);
#[derive(Debug, PartialEq, Eq)]
pub enum ForwardError {
    InvalidConfiguration,
    InvalidBody,
    Unavailable,
}
/// No Debug: the server authorization value remains private and sensitive.
pub struct CallbackForwarder {
    client: Client,
    target: Url,
    authorization: HeaderValue,
}
impl CallbackForwarder {
    pub fn new(origin: &str, token: &str) -> Result<Self, ForwardError> {
        let mut target = endpoint(origin)?;
        target.set_path(CALLBACK_PATH);
        if !(32..=128).contains(&token.len())
            || !token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(ForwardError::InvalidConfiguration);
        }
        let mut authorization = HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|_| ForwardError::InvalidConfiguration)?;
        authorization.set_sensitive(true);
        let roots =
            rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let tls = rustls::ClientConfig::builder_with_provider(std::sync::Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .map_err(|_| ForwardError::Unavailable)?
        .with_root_certificates(roots)
        .with_no_client_auth();
        let client = Client::builder()
            .tls_backend_preconfigured(tls)
            .http1_only()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .no_proxy()
            .referer(false)
            .no_gzip()
            .no_brotli()
            .no_deflate()
            .no_zstd()
            .connect_timeout(DEADLINE)
            .read_timeout(DEADLINE)
            .timeout(DEADLINE)
            .build()
            .map_err(|_| ForwardError::Unavailable)?;
        Ok(Self {
            client,
            target,
            authorization,
        })
    }
    /// The caller has already applied the closed form policy. Fresh server-owned
    /// headers are built here; no incoming header map enters this boundary.
    pub async fn forward(&self, body: &[u8]) -> Result<(), ForwardError> {
        if body.is_empty() || body.len() > MAX_BODY {
            return Err(ForwardError::InvalidBody);
        }
        let mut response = self
            .client
            .post(self.target.clone())
            .header(
                CONTENT_TYPE,
                HeaderValue::from_static("application/x-www-form-urlencoded"),
            )
            .header(AUTHORIZATION, self.authorization.clone())
            .body(body.to_vec())
            .send()
            .await
            .map_err(|_| ForwardError::Unavailable)?;
        if response.status() != reqwest::StatusCode::OK {
            return Err(ForwardError::Unavailable);
        }
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| ForwardError::Unavailable)?
        {
            if !chunk.is_empty() {
                return Err(ForwardError::Unavailable);
            }
        }
        Ok(())
    }
}
fn endpoint(raw: &str) -> Result<Url, ForwardError> {
    let url = Url::parse(raw).map_err(|_| ForwardError::InvalidConfiguration)?;
    let loopback = match url.host() {
        Some(Host::Ipv4(ip)) => ip.is_loopback(),
        Some(Host::Ipv6(ip)) => ip.is_loopback(),
        _ => false,
    };
    if !loopback
        || url.scheme() != "http"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
        || url.port_or_known_default() == Some(0)
        || (raw != url.origin().ascii_serialization() && raw != url.as_str())
    {
        return Err(ForwardError::InvalidConfiguration);
    }
    Ok(url)
}

#[cfg(test)]
#[path = "../tests/support/callback_forwarder.rs"]
mod tests;
