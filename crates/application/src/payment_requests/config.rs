use super::RequestError;
use commerce_domain::payment_attempts::{CurrentConfiguration, PaymentMode, ProfileBinding};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use url::{Host, Url};
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PaymentMethod {
    Ideal,
    Bancontact,
    Creditcard,
    Paypal,
}
impl PaymentMethod {
    pub fn code(self) -> &'static str {
        match self {
            Self::Ideal => "ideal",
            Self::Bancontact => "bancontact",
            Self::Creditcard => "creditcard",
            Self::Paypal => "paypal",
        }
    }
}
/// Uninstalled trusted server policy; supported syntax is not actual profile/method eligibility.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigurationSnapshot {
    pub schema_version: u16,
    pub profile: ProfileBinding,
    pub method: PaymentMethod,
    pub merchant_origin: String,
    pub return_url: String,
    pub webhook_url: String,
}
pub struct Configuration {
    snapshot: ConfigurationSnapshot,
    return_url: Url,
    sha256: String,
    fingerprint_material: String,
}
impl Configuration {
    pub fn capture(snapshot: ConfigurationSnapshot) -> Result<Self, RequestError> {
        snapshot
            .profile
            .validate()
            .map_err(|_| RequestError::InvalidConfiguration)?;
        if snapshot.schema_version != 1
            || snapshot.merchant_origin.len() > 2048
            || snapshot.return_url.len() > 2048
            || snapshot.webhook_url.len() > 2048
            || snapshot.return_url == snapshot.webhook_url
        {
            return Err(RequestError::InvalidConfiguration);
        }
        let origin = Url::parse(&snapshot.merchant_origin)
            .map_err(|_| RequestError::InvalidConfiguration)?;
        if origin.scheme() != "https"
            || origin.origin().ascii_serialization() != snapshot.merchant_origin
            || !matches!(origin.host(),Some(Host::Domain(domain)) if domain.contains('.') && !domain.ends_with('.'))
        {
            return Err(RequestError::InvalidConfiguration);
        }
        let return_url = endpoint(&snapshot.return_url, &snapshot.merchant_origin)?;
        endpoint(&snapshot.webhook_url, &snapshot.merchant_origin)?;
        let mode = match snapshot.profile.mode {
            PaymentMode::Test => "test",
        };
        let canonical = Value::Array(
            [
                "piwas-mollie-test-configuration-v1",
                &snapshot.profile.id,
                &snapshot.profile.credential_fingerprint,
                mode,
                snapshot.method.code(),
                &snapshot.merchant_origin,
                &snapshot.return_url,
                &snapshot.webhook_url,
            ]
            .into_iter()
            .map(|value| Value::String(value.into()))
            .collect(),
        )
        .to_string();
        let sha256 = digest(canonical.as_bytes());
        Ok(Self {
            snapshot,
            return_url,
            sha256,
            fingerprint_material: canonical,
        })
    }
    pub fn identity(&self) -> CurrentConfiguration {
        CurrentConfiguration {
            profile: self.snapshot.profile.clone(),
            sha256: self.sha256.clone(),
        }
    }
    /// Exact versioned digest preimage for immutable relational custody; contains no raw key.
    pub fn fingerprint_material(&self) -> &str {
        &self.fingerprint_material
    }
    pub fn snapshot(&self) -> &ConfigurationSnapshot {
        &self.snapshot
    }
    pub(super) fn return_url(&self) -> Url {
        self.return_url.clone()
    }
    pub(super) fn webhook_url(&self) -> &str {
        &self.snapshot.webhook_url
    }
    pub(super) fn method(&self) -> PaymentMethod {
        self.snapshot.method
    }
}
fn endpoint(value: &str, origin: &str) -> Result<Url, RequestError> {
    let parsed = Url::parse(value).map_err(|_| RequestError::InvalidConfiguration)?;
    if parsed.as_str() != value
        || parsed.origin().ascii_serialization() != origin
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(RequestError::InvalidConfiguration);
    }
    Ok(parsed)
}
pub(super) fn digest(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}
