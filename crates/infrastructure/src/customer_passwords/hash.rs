use super::PasswordError;
use argon2::{Algorithm, Argon2, Params, PasswordHash, PasswordHasher, PasswordVerifier, Version};
use std::time::Duration;
use tokio::sync::{Semaphore, SemaphorePermit};
use unicode_normalization::UnicodeNormalization;
use zeroize::Zeroizing;

const MAX_PASSWORD_BYTES: usize = 256;
const MIN_PASSWORD_CHARACTERS: usize = 15;
const PHC_PREFIX: &str = "$argon2id$v=19$m=19456,t=2,p=1$";
// Canonical encoding of a zero salt and digest. This dummy never grants authority.
const DUMMY_PHC: &str = "$argon2id$v=19$m=19456,t=2,p=1$AAAAAAAAAAAAAAAAAAAAAA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
static PASSWORD_WORK: Semaphore = Semaphore::const_new(2);

async fn acquire_capacity(work: &Semaphore) -> Result<SemaphorePermit<'_>, PasswordError> {
    tokio::time::timeout(Duration::from_secs(2), work.acquire())
        .await
        .map_err(|_| PasswordError::Unavailable)?
        .map_err(|_| PasswordError::Unavailable)
}

fn normalize(password: &str) -> Result<Zeroizing<String>, PasswordError> {
    if password.len() > MAX_PASSWORD_BYTES {
        return Err(PasswordError::Denied);
    }
    let normalized = Zeroizing::new(password.nfc().collect::<String>());
    if normalized.len() > MAX_PASSWORD_BYTES || normalized.chars().count() < MIN_PASSWORD_CHARACTERS
    {
        return Err(PasswordError::Denied);
    }
    Ok(normalized)
}

async fn run<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, PasswordError> + Send + 'static,
) -> Result<T, PasswordError> {
    let permit = acquire_capacity(&PASSWORD_WORK).await?;
    tokio::task::spawn_blocking(move || {
        // Retain the permit even if the awaiting caller is cancelled.
        let _permit = permit;
        work()
    })
    .await
    .map_err(|_| PasswordError::Unavailable)?
}

pub(super) async fn hash_password(password: &str) -> Result<String, PasswordError> {
    let normalized = normalize(password)?;
    run(move || {
        let mut salt = [0; 16];
        getrandom::fill(&mut salt).map_err(|_| PasswordError::Unavailable)?;
        let params = Params::new(19_456, 2, 1, Some(32)).map_err(|_| PasswordError::Unavailable)?;
        Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
            .hash_password_with_salt(normalized.as_bytes(), &salt)
            .map(|hash| hash.to_string())
            .map_err(|_| PasswordError::Unavailable)
    })
    .await
}

pub(crate) async fn verify_password(
    password: &str,
    phc: Option<&str>,
) -> Result<bool, PasswordError> {
    let normalized = normalize(password)?;
    let has_credential = phc.is_some();
    let encoded = phc.unwrap_or(DUMMY_PHC);
    if encoded.len() > 97 || !encoded.starts_with(PHC_PREFIX) {
        return Err(PasswordError::Unavailable);
    }
    let encoded = Zeroizing::new(encoded.to_owned());
    run(move || {
        let parsed = PasswordHash::new(&encoded).map_err(|_| PasswordError::Unavailable)?;
        if parsed.salt.as_ref().map(|salt| salt.len()) != Some(16)
            || parsed.hash.as_ref().map(|digest| digest.len()) != Some(32)
        {
            return Err(PasswordError::Unavailable);
        }
        match Argon2::default().verify_password(normalized.as_bytes(), &parsed) {
            Ok(()) => Ok(has_credential),
            Err(argon2::password_hash::Error::PasswordInvalid) => Ok(false),
            Err(_) => Err(PasswordError::Unavailable),
        }
    })
    .await
}

#[cfg(test)]
#[path = "../../tests/support/customer_password_hash_unit.rs"]
mod tests;
