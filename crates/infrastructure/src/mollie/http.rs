use super::*;
use reqwest::{
    Client, Response, StatusCode,
    header::{ACCEPT, AUTHORIZATION, CONTENT_ENCODING, CONTENT_TYPE, HeaderMap},
};
use std::{sync::Arc, time::Duration};
use url::Url;
pub(super) const MAX_RESPONSE: usize = 65536;
pub(super) struct Http {
    client: Client,
    base: Url,
}
impl Http {
    pub(super) fn build(
        base: Url,
        authorization: reqwest::header::HeaderValue,
        total: Duration,
    ) -> Result<Self, MollieError> {
        let roots =
            rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let tls = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .map_err(|_| MollieError::Unavailable)?
        .with_root_certificates(roots)
        .with_no_client_auth();
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, authorization);
        headers.insert(
            ACCEPT,
            reqwest::header::HeaderValue::from_static("application/hal+json, application/json"),
        );
        let client = Client::builder()
            .default_headers(headers)
            .user_agent("PiwasCommerceTest/1")
            .tls_backend_preconfigured(tls)
            .https_only(base.scheme() == "https")
            .http1_only()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .no_proxy()
            .referer(false)
            .no_gzip()
            .no_brotli()
            .no_deflate()
            .no_zstd()
            .connect_timeout(Duration::from_secs(3))
            .read_timeout(Duration::from_secs(5))
            .timeout(total)
            .build()
            .map_err(|_| MollieError::Unavailable)?;
        Ok(Self { client, base })
    }
    fn path(&self, segments: &[&str]) -> Result<Url, MollieError> {
        let mut url = self.base.clone();
        url.path_segments_mut()
            .map_err(|_| MollieError::InvalidConfiguration)?
            .pop_if_empty()
            .extend(segments);
        Ok(url)
    }
    pub(super) async fn get(&self, segments: &[&str]) -> Result<Vec<u8>, MollieError> {
        let response = self
            .client
            .get(self.path(segments)?)
            .send()
            .await
            .map_err(|_| MollieError::Unavailable)?;
        read(response, StatusCode::OK).await
    }
    pub(super) async fn create(&self, key: &str, body: &str) -> Result<Vec<u8>, MollieError> {
        let response = self
            .client
            .post(self.path(&["payments"])?)
            .header("Idempotency-Key", key)
            .header(CONTENT_TYPE, "application/json")
            .body(body.to_owned())
            .send()
            .await
            .map_err(|_| MollieError::Unavailable)?;
        read(response, StatusCode::CREATED).await
    }
}
async fn read(mut response: Response, expected: StatusCode) -> Result<Vec<u8>, MollieError> {
    if response.status() != expected {
        return Err(MollieError::Unavailable);
    }
    let mime = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next());
    if !mime.is_some_and(|v| {
        v.eq_ignore_ascii_case("application/json") || v.eq_ignore_ascii_case("application/hal+json")
    }) || response
        .headers()
        .get(CONTENT_ENCODING)
        .is_some_and(|v| v != "identity")
    {
        return Err(MollieError::InvalidResponse);
    }
    if response
        .content_length()
        .is_some_and(|v| v > MAX_RESPONSE as u64)
    {
        return Err(MollieError::ResponseTooLarge);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| MollieError::Unavailable)?
    {
        if chunk.len() > MAX_RESPONSE - body.len() {
            return Err(MollieError::ResponseTooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}
