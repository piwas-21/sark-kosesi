//! Trusted immutable offline snapshot. Construction checks integrity, not provenance.
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use super::{PasswordError, hash};

const DOMAIN: &[u8] = b"piwas:password-blocklist:snapshot:v1\0";
const MAX_ENTRIES: usize = 1_000_000;
const MAX_WINDOW_SECONDS: i64 = 90 * 24 * 60 * 60;
const MAX_TIME: i64 = 9_007_199_254_740_991;

/// Metadata and sorted exact SHA-256 digests of NFC-normalized UTF-8 passwords.
/// The caller must obtain the expected digest and approval from a trusted source.
pub struct BlocklistManifest {
    pub version: String,
    pub provenance: String,
    pub qualification_ref: String,
    pub valid_from: i64,
    pub valid_until: i64,
    pub digests: Vec<[u8; 32]>,
}
impl BlocklistManifest {
    /// Canonical SHA-256 for independently reviewing or transporting a manifest.
    /// Computing this value does not establish trust or qualify its contents.
    pub fn integrity_digest(&self) -> [u8; 32] {
        let mut digest = Sha256::new();
        digest.update(DOMAIN);
        for value in [&self.version, &self.provenance, &self.qualification_ref] {
            digest.update((value.len() as u64).to_be_bytes());
            digest.update(value.as_bytes());
        }
        digest.update(self.valid_from.to_be_bytes());
        digest.update(self.valid_until.to_be_bytes());
        digest.update((self.digests.len() as u64).to_be_bytes());
        for entry in &self.digests {
            digest.update(entry);
        }
        digest.finalize().into()
    }
}

/// Validated immutable snapshot; possession does not authenticate its source.
pub struct PasswordBlocklist(BlocklistManifest);
impl PasswordBlocklist {
    pub fn new(manifest: BlocklistManifest, expected: [u8; 32]) -> Result<Self, PasswordError> {
        fn text_valid(value: &str) -> bool {
            !value.is_empty()
                && value.len() <= 240
                && value.trim() == value
                && !value.chars().any(char::is_control)
        }
        if !text_valid(&manifest.version)
            || !text_valid(&manifest.provenance)
            || !text_valid(&manifest.qualification_ref)
            || manifest.valid_from < 0
            || manifest.valid_until > MAX_TIME
            || manifest
                .valid_until
                .checked_sub(manifest.valid_from)
                .is_none_or(|v| v <= 0 || v > MAX_WINDOW_SECONDS)
            || manifest.digests.is_empty()
            || manifest.digests.len() > MAX_ENTRIES
            || !manifest.digests.windows(2).all(|p| p[0] < p[1])
            || manifest.integrity_digest() != expected
        {
            return Err(PasswordError::Unavailable);
        }
        Ok(Self(manifest))
    }

    pub(super) fn require_current(&self, at: i64) -> Result<(), PasswordError> {
        if at < self.0.valid_from || at >= self.0.valid_until {
            return Err(PasswordError::Unavailable);
        }
        Ok(())
    }

    pub(super) fn check(&self, password: &str, at: i64) -> Result<(), PasswordError> {
        self.require_current(at)?;
        let normalized: Zeroizing<String> = hash::normalize(password)?;
        let digest = Zeroizing::new(<[u8; 32]>::from(Sha256::digest(normalized.as_bytes())));
        if self.0.digests.binary_search(&*digest).is_ok() {
            return Err(PasswordError::Denied);
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../tests/support/customer_password_blocklist_unit.rs"]
mod tests;
