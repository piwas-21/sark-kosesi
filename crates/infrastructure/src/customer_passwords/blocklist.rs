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

    /// Decode the canonical integrity preimage with a separately trusted digest.
    /// This checks shape and integrity, not the source or qualification of the corpus.
    pub fn from_canonical_bytes(bytes: &[u8], expected: [u8; 32]) -> Result<Self, PasswordError> {
        fn take<'a>(input: &mut &'a [u8], count: usize) -> Result<&'a [u8], PasswordError> {
            let (head, tail) = input
                .split_at_checked(count)
                .ok_or(PasswordError::Unavailable)?;
            *input = tail;
            Ok(head)
        }
        fn integer(input: &mut &[u8]) -> Result<[u8; 8], PasswordError> {
            let bytes = take(input, 8)?;
            Ok([
                bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
            ])
        }
        fn text(input: &mut &[u8]) -> Result<String, PasswordError> {
            let length = u64::from_be_bytes(integer(input)?);
            if length == 0 || length > 240 {
                return Err(PasswordError::Unavailable);
            }
            let length = length as usize; // At most 240 on every supported target.
            let value = std::str::from_utf8(take(input, length)?)
                .map_err(|_| PasswordError::Unavailable)?;
            Ok(value.to_owned())
        }
        let mut input = bytes
            .strip_prefix(DOMAIN)
            .ok_or(PasswordError::Unavailable)?;
        let version = text(&mut input)?;
        let provenance = text(&mut input)?;
        let qualification_ref = text(&mut input)?;
        let valid_from = i64::from_be_bytes(integer(&mut input)?);
        let valid_until = i64::from_be_bytes(integer(&mut input)?);
        let count = u64::from_be_bytes(integer(&mut input)?);
        if count == 0 || count > MAX_ENTRIES as u64 {
            return Err(PasswordError::Unavailable);
        }
        let count = count as usize; // At most 1,000,000 on every supported target.
        if count * 32 != input.len() {
            return Err(PasswordError::Unavailable);
        }
        let digests = input
            .chunks_exact(32)
            .map(|entry| {
                let mut digest = [0; 32];
                digest.copy_from_slice(entry);
                digest
            })
            .collect();
        Self::new(
            BlocklistManifest {
                version,
                provenance,
                qualification_ref,
                valid_from,
                valid_until,
                digests,
            },
            expected,
        )
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
