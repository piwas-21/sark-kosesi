use super::*;
use commerce_domain::payment_attempts::PaymentMode;
use reqwest::header::HeaderValue;
use sha2::{Digest, Sha256};
/// No Debug/Clone/serialization/raw-key getter. Only sensitive HTTP authorization survives.
pub struct TestCredential {
    pub(super) binding: ProfileBinding,
    pub(super) authorization: HeaderValue,
}
impl TestCredential {
    pub fn capture(raw_key: &str, profile_id: &str) -> Result<Self, MollieError> {
        let suffix = raw_key
            .strip_prefix("test_")
            .ok_or(MollieError::InvalidCredential)?;
        if !(20..=128).contains(&suffix.len()) || !suffix.bytes().all(|v| v.is_ascii_alphanumeric())
        {
            return Err(MollieError::InvalidCredential);
        }
        let mut digest = Sha256::new();
        digest.update(b"piwas-mollie-test-credential-v1\0");
        digest.update(raw_key.as_bytes());
        let binding = ProfileBinding {
            id: profile_id.into(),
            credential_fingerprint: format!("{:x}", digest.finalize()),
            mode: PaymentMode::Test,
        };
        binding
            .validate()
            .map_err(|_| MollieError::InvalidCredential)?;
        let mut authorization = HeaderValue::from_str(&format!("Bearer {raw_key}"))
            .map_err(|_| MollieError::InvalidCredential)?;
        authorization.set_sensitive(true);
        Ok(Self {
            binding,
            authorization,
        })
    }
    pub fn binding(&self) -> &ProfileBinding {
        &self.binding
    }
}
